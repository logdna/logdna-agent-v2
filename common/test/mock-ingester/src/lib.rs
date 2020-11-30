#[macro_use]
extern crate log;

use futures::{future, Stream, StreamExt, TryFutureExt, TryStreamExt};
use hyper::service::Service;
use hyper::{Body, Request, Response};
use rustls::internal::pemfile;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::convert::From;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::{fs, io};
use tokio::macros::support::{Future, Pin};
use tokio::sync::Mutex;

use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::server::TlsStream;
use tokio_rustls::TlsAcceptor;

const ROOT: &str = "/logs/agent";

#[derive(Serialize, Deserialize)]
struct IngestBody {
    lines: Vec<Line>,
}

#[derive(Serialize, Deserialize)]
struct Line {
    line: Option<String>,
    file: Option<String>,
}

#[derive(Debug)]
pub struct Svc {
    files: Arc<Mutex<HashMap<String, (AtomicUsize, File)>>>,
}

impl Unpin for Svc {}

impl Service<Request<Body>> for Svc {
    type Response = Response<Body>;
    type Error = hyper::Error;
    #[allow(clippy::type_complexity)]
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Ok(()).into()
    }

    fn call(&mut self, req: Request<Body>) -> Self::Future {
        info!("Received {:?}", req);
        let files = self.files.clone();
        Box::pin(async move {
            let rsp = Response::builder();

            let uri = req.uri();
            if uri.path() != ROOT {
                return Ok(rsp.status(404).body(Body::empty()).unwrap());
            }

            let encoding = {
                &req.headers()
                    .get("content-encoding")
                    .map(|e| e.to_str().unwrap().to_string())
                    .clone()
            };

            let mut bytes = Vec::new();
            let mut body = req.into_body();
            match encoding {
                Some(encoding) if encoding == "gzip" => {
                    let mut decoder = async_compression::stream::GzipDecoder::new(
                        body.map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e)),
                    );
                    while let Some(Ok(chunk)) = decoder.next().await {
                        bytes.extend_from_slice(&chunk);
                    }
                }
                _ => {
                    while let Some(Ok(chunk)) = body.next().await {
                        bytes.extend_from_slice(&chunk);
                    }
                }
            }

            let ingest_body: IngestBody = match serde_json::from_slice(&bytes) {
                Ok(lines) => lines,
                Err(e) => {
                    error!("{}", e);
                    return Ok(rsp.status(500).body(Body::empty()).unwrap());
                }
            };

            for line in ingest_body.lines {
                if let Some(mut raw_line) = line.line {
                    if !raw_line.ends_with('\n') {
                        raw_line.push('\n')
                    }

                    let orig_file_name = line
                        .file
                        .unwrap_or_else(|| " unknown".into());

                    let file_name = orig_file_name.replace("/", "-").clone();
                    let file_name = &file_name[1..];

                    let mut files = files.lock().await;
                    let (ref line_count, ref mut file) =
                        files.entry(orig_file_name.into())
                          .or_insert_with(|| {
                            info!("creating {}", file_name);
                            (AtomicUsize::new(0),
                             OpenOptions::new()
                                .create(true)
                                .append(true)
                                .open(&file_name)
                                .unwrap())
                        });

                    line_count.fetch_add(1, Ordering::Relaxed);
                    file.write_all(raw_line.as_bytes()).unwrap();
                }
            }

            Ok(rsp.status(200).body(Body::empty()).unwrap())
        })
    }
}

pub struct MakeSvc {
    files: Arc<Mutex<HashMap<String, (AtomicUsize, File)>>>,
}

impl MakeSvc {
    pub fn new() -> Self {
        MakeSvc {
            files: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl Default for MakeSvc {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Service<T> for MakeSvc {
    type Response = Svc;
    type Error = std::io::Error;
    type Future = future::Ready<Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Ok(()).into()
    }

    fn call(&mut self, _: T) -> Self::Future {
        future::ok(Svc {
            files: self.files.clone(),
        })
    }
}

pub async fn http_ingester(addr: SocketAddr) -> std::result::Result<(), hyper::Error> {
    hyper::Server::bind(&addr).serve(MakeSvc::new()).await
}

fn error(err: String) -> io::Error {
    io::Error::new(io::ErrorKind::Other, err)
}

pub fn https_ingester(
    addr: SocketAddr,
    server_cert: Vec<rustls::Certificate>,
    private_key: rustls::PrivateKey,
) -> (
    impl Future<Output = std::result::Result<(), hyper::Error>>,
    Arc<Mutex<HashMap<String, (AtomicUsize, File)>>>,
    impl FnOnce() -> (),
) {
    info!("creating https_ingester");
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let mk_svc = MakeSvc::new();
    let received = mk_svc.files.clone();
    (
        async move {
            // Build TLS configuration.
            let tls_cfg = {
                // Do not use client certificate authentication.
                let mut cfg = rustls::ServerConfig::new(rustls::NoClientAuth::new());
                // Select a certificate to use.
                cfg.set_single_cert(server_cert, private_key)
                    .map_err(|e| error(format!("{}", e)))
                    .unwrap();
                // Configure ALPN to accept HTTP/2, HTTP/1.1 in that order.
                cfg.set_protocols(&[b"h2".to_vec(), b"http/1.1".to_vec()]);
                Arc::new(cfg)
            };

            // Create a TCP listener via tokio.
            let mut tcp = TcpListener::bind(&addr)
                .await
                .unwrap_or_else(|_| panic!("Couldn't bind to {:?}", addr));
            let tls_acceptor = TlsAcceptor::from(tls_cfg);
            // Prepare a long-running future stream to accept and serve cients.
            let incoming_tls_stream = tcp
                .incoming()
                .map_err(|e| error(format!("Incoming failed: {:?}", e)))
                .and_then(move |s| {
                    tls_acceptor.accept(s).map_err(|e| {
                        println!("[!] Voluntary server halt due to client-connection error...");
                        // Errors could be handled here, instead of server aborting.
                        // Ok(None)
                        error(format!("TLS Error: {:?}", e))
                    })
                })
                .boxed();
            hyper::Server::builder(HyperAcceptor {
                acceptor: incoming_tls_stream,
            })
            .serve(mk_svc)
            .with_graceful_shutdown(async {
                rx.await.ok();
            })
            .await
        },
        received,
        move || tx.send(()).expect("Couldn't terminate server"),
    )
}

struct HyperAcceptor<'a> {
    acceptor: Pin<Box<dyn Stream<Item = Result<TlsStream<TcpStream>, io::Error>> + 'a>>,
}

impl hyper::server::accept::Accept for HyperAcceptor<'_> {
    type Conn = TlsStream<TcpStream>;
    type Error = io::Error;

    fn poll_accept(
        mut self: Pin<&mut Self>,
        cx: &mut Context,
    ) -> Poll<Option<Result<Self::Conn, Self::Error>>> {
        Pin::new(&mut self.acceptor).poll_next(cx)
    }
}

// Load public certificate from file.
pub fn load_certs(filename: &str) -> io::Result<Vec<rustls::Certificate>> {
    // Open certificate file.
    let certfile = fs::File::open(filename)
        .map_err(|e| error(format!("failed to open {}: {}", filename, e)))?;
    let mut reader = io::BufReader::new(certfile);

    // Load and return certificate.
    pemfile::certs(&mut reader).map_err(|_| error("failed to load certificate".into()))
}

// Load private key from file.
pub fn load_private_key(filename: &str) -> io::Result<rustls::PrivateKey> {
    // Open keyfile.
    let keyfile = fs::File::open(filename)
        .map_err(|e| error(format!("failed to open {}: {}", filename, e)))?;
    let mut reader = io::BufReader::new(keyfile);

    // Load and return a single private key.
    let keys = pemfile::rsa_private_keys(&mut reader)
        .map_err(|_| error("failed to load private key".into()))?;
    if keys.len() != 1 {
        return Err(error("expected a single private key".into()));
    }
    Ok(keys[0].clone())
}

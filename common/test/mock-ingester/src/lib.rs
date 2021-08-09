#[macro_use]
extern crate log;

use futures::{future, Stream, StreamExt, TryFutureExt, TryStreamExt};
use hyper::service::Service;
use hyper::{Body, Request, Response};
use rustls::internal::pemfile;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::convert::From;
use std::net::SocketAddr;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::{fs, io};
use thiserror::Error;
use tokio::macros::support::{Future, Pin};
use tokio::sync::Mutex;

use tokio::io::AsyncReadExt;
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::server::TlsStream;
use tokio_rustls::TlsAcceptor;
use tokio_stream::wrappers::TcpListenerStream;

const ROOT: &str = "/logs/agent";

pub type FileLineCounter = Arc<Mutex<HashMap<String, FileInfo>>>;
pub type ProcessFn = Box<dyn Fn(&IngestBody) + Send + Sync>;

#[derive(Debug)]
pub struct FileInfo {
    pub tags: Option<String>,
    pub values: Vec<String>,
    pub lines: usize,
    pub annotation: Option<HashMap<String, String>>,
    pub label: Option<HashMap<String, String>>,
}

#[derive(Debug, Error)]
pub enum IngestError {
    #[error(transparent)]
    HyperError(#[from] hyper::Error),
    #[error(transparent)]
    IoError(#[from] std::io::Error),
}

#[derive(Serialize, Deserialize, Debug)]
pub struct IngestBody {
    pub lines: Vec<Line>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Line {
    pub line: Option<String>,
    tags: Option<String>,
    pub file: Option<String>,
    annotation: Option<HashMap<String, String>>,
    label: Option<HashMap<String, String>>,
}

// #[derive(Debug)]
pub struct Svc {
    files: FileLineCounter,
    process_fn: Arc<ProcessFn>,
}

impl Unpin for Svc {}

impl Service<Request<Body>> for Svc {
    type Response = Response<Body>;
    type Error = IngestError;
    #[allow(clippy::type_complexity)]
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Ok(()).into()
    }

    fn call(&mut self, req: Request<Body>) -> Self::Future {
        info!("Received {:?}", req);
        let files = self.files.clone();
        let process_fn = self.process_fn.clone();
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
            let params: HashMap<String, String> = req
                .uri()
                .query()
                .map(|v| {
                    url::form_urlencoded::parse(v.as_bytes())
                        .into_owned()
                        .collect()
                })
                .unwrap_or_else(HashMap::new);

            let body = req.into_body();
            let mut body = tokio_util::io::StreamReader::new(
                body.map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e)),
            );
            match encoding {
                Some(encoding) if encoding == "gzip" => {
                    let mut decoder = async_compression::tokio::bufread::GzipDecoder::new(body);
                    decoder.read_to_end(&mut bytes).await?;
                }
                _ => {
                    body.read_to_end(&mut bytes).await?;
                }
            }

            let ingest_body: IngestBody = match serde_json::from_slice(&bytes) {
                Ok(lines) => lines,
                Err(e) => {
                    panic!(
                        "Ingest body could not be parsed: {}\n{}",
                        e,
                        std::str::from_utf8(&bytes).unwrap(),
                    );
                }
            };

            process_fn(&ingest_body);

            for line in ingest_body.lines {
                if let Some(mut raw_line) = line.line {
                    if !raw_line.ends_with('\n') {
                        raw_line.push('\n')
                    }

                    let tags = params.get("tags").map(String::from);
                    let orig_file_name = line.file.unwrap_or_else(|| " unknown".into());
                    let file_name = orig_file_name.replace("/", "-").clone();

                    let mut files = files.lock().await;
                    let file_info = files.entry(orig_file_name).or_insert_with(move || {
                        info!("creating {}", file_name);
                        FileInfo {
                            tags,
                            values: Vec::new(),
                            lines: 0,
                            annotation: None,
                            label: None,
                        }
                    });

                    file_info.annotation = line.annotation.clone();
                    file_info.label = line.label.clone();
                    file_info.lines += 1;
                    file_info.values.push(raw_line);
                }
            }

            Ok(rsp.status(200).body(Body::empty()).unwrap())
        })
    }
}

pub struct MakeSvc {
    files: FileLineCounter,
    process_fn: Arc<ProcessFn>,
}

impl MakeSvc {
    pub fn new(process_fn: ProcessFn) -> Self {
        MakeSvc {
            files: Arc::new(Mutex::new(HashMap::new())),
            process_fn: Arc::new(process_fn),
        }
    }
}

impl Default for MakeSvc {
    fn default() -> Self {
        Self::new(Box::new(|_| {}))
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
            process_fn: self.process_fn.clone(),
        })
    }
}

pub fn http_ingester(
    addr: SocketAddr,
) -> (
    impl Future<Output = std::result::Result<(), IngestError>>,
    FileLineCounter,
    impl FnOnce(),
) {
    http_ingester_with_processors(addr, Box::new(|_| {}))
}

pub fn http_ingester_with_processors(
    addr: SocketAddr,
    process_fn: ProcessFn,
) -> (
    impl Future<Output = std::result::Result<(), IngestError>>,
    FileLineCounter,
    impl FnOnce(),
) {
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let mk_svc = MakeSvc::new(process_fn);
    let received = mk_svc.files.clone();
    (
        async move {
            // Create a TCP listener via tokio.
            let tcp = TcpListener::bind(&addr)
                .await
                .unwrap_or_else(|_| panic!("Couldn't bind to {:?}", addr));
            // Prepare a long-running future stream to accept and serve cients.
            let incoming_stream = TcpListenerStream::new(tcp)
                .map_err(|e| error(format!("Incoming failed: {:?}", e)))
                .boxed();
            hyper::server::Server::builder(HyperAcceptor {
                acceptor: incoming_stream,
            })
            .serve(mk_svc)
            .with_graceful_shutdown(async {
                rx.await.ok();
            })
            .await
            .map_err(IngestError::HyperError)
        },
        received,
        move || tx.send(()).expect("Couldn't terminate server"),
    )
}

fn error(err: String) -> io::Error {
    io::Error::new(io::ErrorKind::Other, err)
}

pub fn https_ingester(
    addr: SocketAddr,
    server_cert: Vec<rustls::Certificate>,
    private_key: rustls::PrivateKey,
) -> (
    impl Future<Output = std::result::Result<(), IngestError>>,
    FileLineCounter,
    impl FnOnce(),
) {
    info!("creating https_ingester");
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let mk_svc = MakeSvc::new(Box::new(|_| {}));
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
            let tcp = TcpListener::bind(&addr)
                .await
                .unwrap_or_else(|_| panic!("Couldn't bind to {:?}", addr));
            info!("ingester listening at {:?}", addr);
            let tls_acceptor = TlsAcceptor::from(tls_cfg);
            // Prepare a long-running future stream to accept and serve cients.
            //
            let incoming_tls_stream = TcpListenerStream::new(tcp)
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
            hyper::server::Server::builder(HyperTlsAcceptor {
                acceptor: incoming_tls_stream,
            })
            .serve(mk_svc)
            .with_graceful_shutdown(async {
                rx.await.ok();
            })
            .await
            .map_err(IngestError::HyperError)
        },
        received,
        move || tx.send(()).expect("Couldn't terminate server"),
    )
}

struct HyperTlsAcceptor<'a> {
    acceptor: Pin<Box<dyn Stream<Item = Result<TlsStream<TcpStream>, io::Error>> + 'a>>,
}

impl hyper::server::accept::Accept for HyperTlsAcceptor<'_> {
    type Conn = TlsStream<TcpStream>;
    type Error = io::Error;

    fn poll_accept(
        mut self: Pin<&mut Self>,
        cx: &mut Context,
    ) -> Poll<Option<Result<Self::Conn, Self::Error>>> {
        Pin::new(&mut self.acceptor).poll_next(cx)
    }
}

struct HyperAcceptor<'a> {
    acceptor: Pin<Box<dyn Stream<Item = Result<TcpStream, io::Error>> + 'a>>,
}

impl hyper::server::accept::Accept for HyperAcceptor<'_> {
    type Conn = TcpStream;
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

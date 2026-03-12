use futures::TryFutureExt;
use hyper::service::service_fn;
use hyper::{body::Bytes, header::CONTENT_TYPE, Request, Response};
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder;
use prometheus::{Encoder, TextEncoder};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use thiserror::Error;
use tokio::net::TcpListener;
use tracing::info;

#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Server(#[from] std::io::Error),
}

pub async fn serve(port: &u16) -> Result<(), Error> {
    let address = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), *port);

    // Bind using tokio::net::TcpListener.
    // Note: This produces an std::io::Error. You may need to update
    // your `Error::Server` variant if it specifically expected `hyper::Error`.
    let listener = TcpListener::bind(&address).await.map_err(Error::Server)?;

    info!("Metrics server listening on http://{}", address);

    loop {
        // Accept incoming TCP connections
        let (stream, _remote_addr) = listener.accept().await.map_err(Error::Server)?;

        // Wrap the stream for Hyper
        let io = TokioIo::new(stream);

        // Spawn a background task so we don't block the loop
        tokio::task::spawn(async move {
            if let Err(err) = Builder::new(TokioExecutor::new())
                .serve_connection(io, service_fn(serve_req))
                .await
            {
                // Connection-level errors happen here (e.g., client disconnected unexpectedly)
                // Use your preferred logging macro instead of eprintln! if you want
                eprintln!("Error serving connection: {:?}", err);
            }
        });
    }
}

async fn serve_req(_req: Request<Bytes>) -> Result<Response<Bytes>, hyper::Error> {
    let encoder = TextEncoder::new();

    let metric_families = prometheus::gather();
    let mut buffer = Vec::new();
    encoder.encode(&metric_families, &mut buffer).unwrap();

    let response = Response::builder()
        .status(200)
        .header(CONTENT_TYPE, encoder.format_type())
        .body(Bytes::from(buffer))
        .unwrap();

    Ok(response)
}

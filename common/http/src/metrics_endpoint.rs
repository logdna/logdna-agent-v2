use futures::TryFutureExt;
use hyper::{header::CONTENT_TYPE, service::service_fn, Request, Response};

use hyper::body::Bytes;
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;

use http_body_util::Full;
use hyper_util::{rt::TokioExecutor, server::conn::auto};

use prometheus::{Encoder, TextEncoder};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use thiserror::Error;
use tracing::info;

#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Server(#[from] Box<dyn std::error::Error + Send + Sync>),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub async fn serve(port: &u16) -> Result<(), Error> {
    let address = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), *port);

    info!("Metrics server listening on http://{}", address);
    let listener = TcpListener::bind(address).await?;
    let (stream, _) = listener.accept().await?;
    let io = TokioIo::new(stream);
    auto::Builder::new(TokioExecutor::new())
        .serve_connection(io, service_fn(serve_req))
        .map_err(Error::Server)
        .await
}

async fn serve_req(
    _req: Request<hyper::body::Incoming>,
) -> Result<Response<Full<Bytes>>, hyper::Error> {
    let encoder = TextEncoder::new();

    let metric_families = prometheus::gather();
    let mut buffer = Vec::new();
    encoder.encode(&metric_families, &mut buffer).unwrap();

    let response = Response::builder()
        .status(200)
        .header(CONTENT_TYPE, encoder.format_type())
        .body(buffer.into())
        .unwrap();

    Ok(response)
}

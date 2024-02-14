use futures::TryFutureExt;
use hyper::{
    body::{Bytes, Incoming},
    header::CONTENT_TYPE,
    server::conn::http1,
    service::service_fn,
    Request, Response,
};

use http_body_util::Full;
use hyper_util::rt::TokioIo;
use prometheus::{Encoder, TextEncoder};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use thiserror::Error;
use tokio::net::TcpListener;
use tracing::info;

#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Server(#[from] hyper::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub async fn serve(port: &u16) -> Result<(), Error> {
    let address = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), *port);
    let listener = TcpListener::bind(address).await?;
    let (stream, _) = listener.accept().await?;
    let io = TokioIo::new(stream);

    let serve_future = http1::Builder::new().serve_connection(io, service_fn(serve_req));
    info!("Metrics server listening on http://{}", address);
    serve_future.map_err(Error::Server).await
}

async fn serve_req(_req: Request<Incoming>) -> Result<Response<Full<Bytes>>, hyper::Error> {
    let encoder = TextEncoder::new();

    let metric_families = prometheus::gather();
    let mut buffer = vec![];
    encoder.encode(&metric_families, &mut buffer).unwrap();

    let response = Response::builder()
        .status(200)
        .header(CONTENT_TYPE, encoder.format_type())
        .body(Full::from(Bytes::from(buffer)))
        .unwrap();

    Ok(response)
}

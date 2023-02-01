#[macro_use]
extern crate lazy_static;

use std::env;
use std::time::Duration;

use errors::K8sError;
use hyper::{client::HttpConnector, Body};
use hyper::{Request, Response};
use hyper_timeout::TimeoutConnector;
use kube::client::ConfigExt;
use kube::{config::Config, Client};
use tower::ServiceBuilder;
use tower_http::set_header::SetRequestHeaderLayer;
use tower_http::trace::TraceLayer;

use tracing::{instrument, trace, trace_span, Span};

use hyper::http::header;

pub mod errors;
pub mod event_source;
pub mod feature_leader;
pub mod kube_stats;
pub mod lease;
pub mod metrics_stats_stream;
pub mod middleware;
pub mod restarting_stream;

/// Manually create the k8s http client so that we can add a user-agent header
#[instrument(level = "trace")]
fn create_k8s_client(
    user_agent: hyper::http::header::HeaderValue,
    config: Config,
) -> Result<Client, kube::Error> {
    let timeout = config.timeout;
    let default_ns = config.default_namespace.clone();

    let client: hyper::Client<_, Body> = {
        let mut connector = HttpConnector::new();
        connector.enforce_http(false);

        let connector = hyper_rustls::HttpsConnector::from((
            connector,
            std::sync::Arc::new(config.rustls_client_config()?),
        ));

        let mut connector = TimeoutConnector::new(connector);
        connector.set_connect_timeout(timeout);
        connector.set_read_timeout(timeout);

        hyper::Client::builder().build(connector)
    };

    let stack = ServiceBuilder::new()
        .layer(config.base_uri_layer())
        .into_inner();
    let stack = ServiceBuilder::new()
        .layer(stack)
        .layer(SetRequestHeaderLayer::if_not_present(
            header::USER_AGENT,
            user_agent,
        ))
        .into_inner();
    let stack = ServiceBuilder::new()
        .layer(stack)
        .layer(tower_http::decompression::DecompressionLayer::new())
        .into_inner();

    let service = ServiceBuilder::new()
        .layer(stack)
        .option_layer(config.auth_layer()?)
        .layer(config.extra_headers_layer()?)
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(|request: &hyper::Request<Body>| {
                    trace_span!(
                        "HYPER-HTTP",
                        hyper_http.method = ?request.method(),
                        hyper_http.url = ?request.uri(),
                        hyper_http.status_code = tracing::field::Empty,
                        otel.name = "NEW NAME",
                    )
                })
                .on_request(|request: &Request<Body>, _span: &Span| {
                    trace!(
                        "*** TEST payload: {:?} headers: {:?}",
                        request.body(),
                        request.headers()
                    )
                })
                .on_response(
                    |response: &Response<Body>, latency: Duration, span: &Span| {
                        span.record("status", "value");
                        trace!("*** TEST {:?} {:?}", response.body(), latency)
                    },
                ),
        )
        .service(client);
    Ok(Client::new(service, default_ns))
}

#[instrument(level = "trace")]
pub fn create_k8s_client_default_from_env(
    user_agent: hyper::http::header::HeaderValue,
) -> Result<Client, errors::K8sError> {
    if !is_in_cluster() {
        return Err(K8sError::K8sNotInClusterError());
    }

    let span = trace_span!("k8s-client-span");
    let _enter = span.enter();
    let config = Config::from_cluster_env()?;
    Ok(create_k8s_client(user_agent, config)?)
}

fn is_in_cluster() -> bool {
    env::var("KUBERNETES_SERVICE_HOST").is_ok()
}

#[cfg(test)]
pub mod test {
    // Provide values for extern symbols PKG_NAME and PKG_VERSION
    // when building this module on it's own
    #[no_mangle]
    pub static PKG_NAME: &str = "test";
    #[no_mangle]
    pub static PKG_VERSION: &str = "test";
}

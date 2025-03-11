#[macro_use]
extern crate lazy_static;

use errors::K8sError;
use hyper::http::header;
use hyper_timeout::TimeoutConnector;
use hyper_util::client::legacy::connect::HttpConnector;
use kube::{client::ConfigExt, config::Config, Client};
use std::env;
use tower::ServiceBuilder;
use tower_http::set_header::SetRequestHeaderLayer;

pub mod errors;
pub mod event_source;
pub mod feature_leader;
pub mod kube_stats;
pub mod lease;
pub mod metrics_stats_stream;
pub mod middleware;
pub mod restarting_stream;

const K8S_WATCHER_TIMEOUT: u32 = 30;

/// Manually create the k8s http client so that we can add a user-agent header
fn create_k8s_client(
    user_agent: hyper::http::header::HeaderValue,
    config: Config,
) -> Result<Client, kube::Error> {
    let default_ns = config.default_namespace.clone();

    let client: hyper_util::client::legacy::Client<_, kube::client::Body> = {
        let mut connector = HttpConnector::new();
        connector.enforce_http(false);

        let connector = hyper_rustls::HttpsConnector::from((
            connector,
            std::sync::Arc::new(config.rustls_client_config()?),
        ));

        let mut connector = TimeoutConnector::new(connector);
        connector.set_connect_timeout(config.connect_timeout);
        connector.set_read_timeout(config.read_timeout);

        hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
            .build(connector)
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
        .map_err(tower::BoxError::from)
        .service(client);
    //let service = hyper_util::service::TowerToHyperService::new(service);
    Ok(Client::new(service, default_ns))
}

pub fn create_k8s_client_default_from_env(
    user_agent: hyper::http::header::HeaderValue,
) -> Result<Client, errors::K8sError> {
    if !is_in_cluster() {
        return Err(K8sError::K8sNotInClusterError());
    }

    let config = Config::incluster_dns()?;

    Ok(create_k8s_client(user_agent, config)?)
}

pub fn is_in_cluster() -> bool {
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

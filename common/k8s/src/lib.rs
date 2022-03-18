#[macro_use]
extern crate lazy_static;
#[macro_use]
extern crate log;

use hyper::{client::HttpConnector, Body};
use hyper_timeout::TimeoutConnector;
use kube::client::ConfigExt;
use kube::{config::Config, Client};
use std::fmt;
use tower::ServiceBuilder;
use tower_http::set_header::SetRequestHeaderLayer;

use hyper::http::header;

pub mod errors;
pub mod event_source;
pub mod middleware;
pub mod restarting_stream;

#[derive(Clone, std::fmt::Debug, PartialEq)]
pub enum K8sTrackingConf {
    Always,
    Never,
}

impl Default for K8sTrackingConf {
    fn default() -> Self {
        K8sTrackingConf::Never
    }
}

impl fmt::Display for K8sTrackingConf {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                K8sTrackingConf::Always => "always",
                K8sTrackingConf::Never => "never",
            }
        )
    }
}

#[derive(thiserror::Error, Debug)]
#[error("{0}")]
pub struct ParseK8sTrackingConf(String);

impl std::str::FromStr for K8sTrackingConf {
    type Err = ParseK8sTrackingConf;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_lowercase().as_str() {
            "always" => Ok(K8sTrackingConf::Always),
            "never" => Ok(K8sTrackingConf::Never),
            _ => Err(ParseK8sTrackingConf(format!("failed to parse {}", s))),
        }
    }
}

/// Manually create the k8s http client so that we can add a user-agent header
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
        .service(client);
    Ok(Client::new(service, default_ns))
}

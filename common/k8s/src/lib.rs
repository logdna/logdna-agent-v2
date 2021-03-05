#[macro_use]
extern crate lazy_static;
#[macro_use]
extern crate log;

use std::fmt;

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

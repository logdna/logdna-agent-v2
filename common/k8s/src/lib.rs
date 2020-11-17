#[macro_use]
extern crate lazy_static;
#[macro_use]
extern crate log;

pub mod errors;
pub mod event_source;
pub mod middleware;
pub mod restarting_stream;

#[derive(Clone, std::fmt::Debug, PartialEq)]
pub enum K8sEventLogConf {
    Always,
    Never,
}

#[derive(thiserror::Error, Debug)]
#[error("{0}")]
pub struct ParseK8sEventLogConf(String);

impl std::str::FromStr for K8sEventLogConf {
    type Err = ParseK8sEventLogConf;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_lowercase().as_str() {
            "always" => Ok(K8sEventLogConf::Always),
            "never" => Ok(K8sEventLogConf::Never),
            _ => Err(ParseK8sEventLogConf(format!("failed to parse {}", s))),
        }
    }
}

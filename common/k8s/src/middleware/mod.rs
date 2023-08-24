use regex::Regex;

mod metadata;
mod runner;

pub use metadata::*;
pub use runner::*;

lazy_static! {
    static ref K8S_REG: Regex = Regex::new(
        r#"^/var/log/containers/([a-z0-9A-Z\-.]+)_([a-z0-9A-Z\-.]+)_([a-z0-9A-Z\-.]+)-([a-z0-9]{64}).log$"#
    ).unwrap_or_else(|e| panic!("K8S_REG Regex::new() failed: {}", e));
}

pub struct ParseResult {
    pod_name: String,
    pod_namespace: String,
    container_name: String,
}

impl ParseResult {
    fn new(pod_name: String, pod_namespace: String, container_name: String) -> ParseResult {
        ParseResult {
            pod_name,
            pod_namespace,
            container_name,
        }
    }
}

pub fn parse_container_path(path: &str) -> Option<ParseResult> {
    let captures = K8S_REG.captures(path)?;
    Some(ParseResult::new(
        captures.get(1)?.as_str().into(),
        captures.get(2)?.as_str().into(),
        captures.get(3)?.as_str().into(),
    ))
}

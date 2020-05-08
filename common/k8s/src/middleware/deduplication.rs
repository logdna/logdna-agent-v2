use crate::middleware::parse_container_path;
use http::types::body::LineBuilder;
use middleware::{Middleware, Status};
use metrics::Metrics;

pub struct K8sDeduplication;

impl K8sDeduplication {
    pub fn new() -> Self {
        Self {}
    }
}

impl Middleware for K8sDeduplication {
    fn run(&self) {}

    fn process(&self, lines: Vec<LineBuilder>) -> Status {
        for line in lines.iter() {
            if let Some(ref file_name) = line.file {
                if let Some(_) = parse_container_path(&file_name) {
                    Metrics::k8s().increment_lines();
                    return Status::Ok(vec![line.clone()]);
                }
            }
        }
        Status::Ok(lines)
    }
}

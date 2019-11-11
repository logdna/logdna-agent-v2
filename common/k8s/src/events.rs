use http::types::body::LineBuilder;
use kube::api::{v1Event, Informer, WatchEvent};
use kube::{api::Api, client::APIClient, config};
use std::env;
use std::time::{Duration, Instant};

pub struct K8sEvents {
    inf: Informer<v1Event>,
    last_poll: Instant,
    node_name: String,
}

impl K8sEvents {
    pub fn new() -> Self {
        let config = match config::incluster_config() {
            Ok(v) => v,
            Err(e) => {
                error!("failed to connect to k8s cluster: {}", e);
                panic!("{}", e);
            }
        };

        let api = APIClient::new(config);
        let resource = Api::v1Event(api);

        let inf = Informer::new(resource).init().expect("k8s informer failed");

        K8sEvents {
            inf,
            last_poll: Instant::now(),
            node_name: env::var("NODE_NAME").unwrap_or("".into()),
        }
    }

    pub fn read_lines<F: FnMut(LineBuilder)>(&mut self, mut callback: F) {
        if self.last_poll.elapsed() < Duration::from_secs(1) {
            return;
        }

        self.last_poll = Instant::now();

        if let Err(e) = self.inf.poll() {
            error!("error polling api server: {}", e);
            return;
        };

        while let Some(event) = self.inf.pop() {
            match event {
                WatchEvent::Added(e) | WatchEvent::Modified(e) | WatchEvent::Deleted(e) => {
                    if let Some(ref source) = e.source {
                        if let Some(ref host) = source.host {
                            if self.node_name == *host {
                                if let Ok(json) = serde_json::to_string(&e) {
                                    let mut line = LineBuilder::new()
                                        .line(json)
                                        .app("kube-events")
                                        .level(e.type_);

                                    if let Some(ref name) = e.involvedObject.name {
                                        line = line.host(name);
                                    }

                                    callback(line)
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

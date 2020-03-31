use std::collections::HashMap;
use std::env;

use parking_lot::Mutex;
use regex::Regex;

use http::types::body::{KeyValueMap, LineBuilder};
use k8s_openapi::api::core::v1::Pod;
use kube::{
    api::{ListParams, Resource, WatchEvent},
    client::APIClient,
    config,
    runtime::Informer,
    Api,
};
use metrics::Metrics;
use middleware::{Middleware, Status};

use futures::stream::StreamExt;
use tokio::runtime::{Builder, Runtime};

lazy_static! {
    static ref K8S_REG: Regex = Regex::new(
        r#"^/var/log/containers/([a-z0-9A-Z\-.]+)_([a-z0-9A-Z\-.]+)_([a-z0-9A-Z\-.]+)-([a-z0-9]{64}).log$"#
    ).expect("K8S_REG Regex::new()");
}

quick_error! {
    #[derive(Debug)]
    enum Error {
        Io(e: std::io::Error) {
            from()
            display("{}", e)
        }
        Utf(e: std::string::FromUtf8Error) {
            from()
            display("{}", e)
        }
        Regex {
            from()
            display("failed to parse path")
        }
        K8s(e: kube::Error) {
            from()
            display("{}", e)
        }
    }
}

pub struct K8sMiddleware {
    metadata: Mutex<HashMap<String, PodMetadata>>,
    informer: Mutex<Option<Informer<Pod>>>,
    runtime: Mutex<Option<Runtime>>,
}

impl K8sMiddleware {
    pub fn new() -> Self {
        let mut runtime = Builder::new()
            .threaded_scheduler()
            .enable_all()
            .core_threads(2)
            .build()
            .unwrap();
        let this = runtime.block_on(async {
            let node = env::var("NODE_NAME").unwrap();

            let config = config::incluster_config().unwrap();
            let client = APIClient::new(config);

            let params = ListParams::default().fields(&format!("spec.nodeName={}", node));
            let mut metadata = HashMap::new();

            let pods = Api::<Pod>::all(client.clone()).list(&params).await.unwrap();
            for pod in pods {
                let real_pod_meta = pod.metadata.unwrap();
                let pod_meta_data = PodMetadata {
                    name: real_pod_meta.name.unwrap(),
                    namespace: real_pod_meta.namespace.unwrap(),
                    labels: real_pod_meta.labels.unwrap().into(),
                    annotations: real_pod_meta.annotations.unwrap().into(),
                };
                metadata.insert(
                    format!("{}_{}", pod_meta_data.name, pod_meta_data.namespace),
                    pod_meta_data,
                );
            }

            K8sMiddleware {
                metadata: Mutex::new(metadata),
                informer: Mutex::new(Some(Informer::new(client, params, Resource::all::<Pod>()))),
                runtime: Mutex::new(None),
            }
        });

        *this.runtime.lock() = Some(runtime);
        this
    }

    fn handle_pod(&self, event: WatchEvent<Pod>) {
        match event {
            WatchEvent::Added(pod) => {
                let real_pod_meta = pod.metadata.unwrap();
                let pod_meta_data = PodMetadata {
                    name: real_pod_meta.name.unwrap(),
                    namespace: real_pod_meta.namespace.unwrap(),
                    labels: real_pod_meta.labels.unwrap().into(),
                    annotations: real_pod_meta.annotations.unwrap().into(),
                };
                self.metadata.lock().insert(
                    format!("{}_{}", pod_meta_data.name, pod_meta_data.namespace),
                    pod_meta_data,
                );
                Metrics::k8s().increment_creates();
            }
            WatchEvent::Modified(pod) => {
                let real_pod_meta = pod.metadata.unwrap();
                if let Some(pod_meta_data) = self.metadata.lock().get_mut(&format!(
                    "{}_{}",
                    real_pod_meta.name.unwrap(),
                    real_pod_meta.namespace.unwrap()
                )) {
                    pod_meta_data.labels = real_pod_meta.labels.unwrap().into();
                    pod_meta_data.annotations = real_pod_meta.annotations.unwrap().into();
                }
            }
            WatchEvent::Deleted(pod) => {
                let real_pod_meta = pod.metadata.unwrap();
                self.metadata.lock().remove(&format!(
                    "{}_{}",
                    real_pod_meta.name.unwrap(),
                    real_pod_meta.namespace.unwrap()
                ));
                Metrics::k8s().increment_deletes();
            }
            WatchEvent::Error(e) => {
                debug!("kubernetes api error event: {:?}", e);
            }
        }
    }
}

impl Middleware for K8sMiddleware {
    fn run(&self) {
        let mut runtime = self.runtime.lock().take().unwrap();
        let informer = self.informer.lock().take().unwrap();

        runtime.block_on(async move {
            loop {
                let mut pods = informer.poll().await.unwrap().boxed();
                Metrics::k8s().increment_polls();

                while let Some(Ok(event)) = pods.next().await {
                    self.handle_pod(event);
                }
            }
        });
    }

    fn process(&self, lines: Vec<LineBuilder>) -> Status {
        let mut container_line = None;
        for line in lines.iter() {
            if let Some(ref file_name) = line.file {
                if let Some((name, namespace)) = parse_container_path(&file_name) {
                    if let Some(pod_meta_data) =
                        self.metadata.lock().get(&format!("{}_{}", name, namespace))
                    {
                        Metrics::k8s().increment_lines();
                        let mut new_line = line.clone();
                        new_line = new_line.labels(pod_meta_data.labels.clone());
                        new_line = new_line.annotations(pod_meta_data.annotations.clone());
                        container_line = Some(new_line);
                    }
                }
            }
        }

        if let Some(line) = container_line {
            return Status::Ok(vec![line]);
        }

        Status::Ok(lines)
    }
}

fn parse_container_path(path: &str) -> Option<(String, String)> {
    let captures = K8S_REG.captures(path)?;
    Some((
        captures.get(1)?.as_str().into(),
        captures.get(2)?.as_str().into(),
    ))
}

struct PodMetadata {
    name: String,
    namespace: String,
    labels: KeyValueMap,
    annotations: KeyValueMap,
}

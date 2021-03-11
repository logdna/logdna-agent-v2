use crate::errors::K8sError;
use crate::middleware::parse_container_path;
use futures::stream::TryStreamExt;
use futures::StreamExt;
use http::types::body::{KeyValueMap, LineMetaMut};
use k8s_openapi::api::core::v1::Pod;
use kube::{api::ListParams, config::Config, Api, Client};

use kube_runtime::watcher;
use kube_runtime::watcher::Event as WatcherEvent;

use metrics::Metrics;
use middleware::{Middleware, Status};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::convert::TryFrom;
use std::env;
use thiserror::Error;
use tokio::runtime::{Builder, Runtime};

#[derive(Error, Debug)]
enum Error {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Utf(#[from] std::string::FromUtf8Error),
    #[error(transparent)]
    K8s(#[from] kube::Error),
}

pub struct K8sMetadata {
    metadata: Mutex<HashMap<(String, String), PodMetadata>>,
    api: Api<Pod>,
    runtime: Mutex<Option<Runtime>>,
}

// TODO refactor to use kube-rs Reflector instead of manually managing hashmap
impl K8sMetadata {
    pub fn new() -> Result<Self, K8sError> {
        let mut runtime = match Builder::new()
            .threaded_scheduler()
            .enable_all()
            .core_threads(2)
            .build()
        {
            Ok(v) => v,
            Err(e) => {
                return Err(K8sError::InitializationError(format!(
                    "unable to build tokio runtime: {}",
                    e
                )))
            }
        };
        let this = runtime.block_on(async {
            let config = match Config::from_cluster_env() {
                Ok(v) => v,
                Err(e) => {
                    return Err(K8sError::InitializationError(format!(
                        "unable to get cluster configuration info: {}",
                        e
                    )))
                }
            };
            let client = Client::new(config);

            let mut params = ListParams::default();
            if let Ok(node) = env::var("NODE_NAME") {
                params = ListParams::default().fields(&format!("spec.nodeName={}", node));
            }

            let mut metadata = HashMap::new();

            match Api::<Pod>::all(client.clone()).list(&params).await {
                Ok(pods) => {
                    for pod in pods {
                        let pod_meta_data = match PodMetadata::try_from(pod) {
                            Ok(v) => v,
                            Err(e) => {
                                warn!("ignoring pod on initialization: {}", e);
                                continue;
                            }
                        };
                        metadata.insert(
                            (pod_meta_data.name.clone(), pod_meta_data.namespace.clone()),
                            pod_meta_data,
                        );
                    }
                }
                Err(e) => {
                    return Err(K8sError::InitializationError(format!(
                        "unable to poll pods during initialization: {}",
                        e
                    )));
                }
            }

            Ok(K8sMetadata {
                metadata: Mutex::new(metadata),
                api: Api::<Pod>::all(client),
                runtime: Mutex::new(None),
            })
        });

        if let Ok(ref middleware) = this {
            *middleware.runtime.lock() = Some(runtime);
        }
        this
    }

    fn handle_pod(&self, event: kube_runtime::watcher::Event<Pod>) -> Result<(), K8sError> {
        match event {
            WatcherEvent::Applied(pod) => {
                let pod_meta_data = PodMetadata::try_from(pod)?;
                let mut metadata = self.metadata.lock();
                metadata
                    .entry((pod_meta_data.name.clone(), pod_meta_data.namespace.clone()))
                    .and_modify(|e| {
                        Metrics::k8s().increment_creates();
                        *e = pod_meta_data.clone()
                    })
                    .or_insert_with(|| {
                        Metrics::k8s().increment_creates();
                        pod_meta_data
                    });
            } // insert or update
            WatcherEvent::Deleted(pod) => {
                let pod_meta_data = PodMetadata::try_from(pod)?;
                self.metadata
                    .lock()
                    .remove(&(pod_meta_data.name, pod_meta_data.namespace));
                Metrics::k8s().increment_deletes();
            } // remove
            WatcherEvent::Restarted(pods) => {
                let mut metadata = self.metadata.lock();
                for pod in pods {
                    let pod_meta_data = PodMetadata::try_from(pod)?;
                    metadata
                        .entry((pod_meta_data.name.clone(), pod_meta_data.namespace.clone()))
                        .and_modify(|e| {
                            Metrics::k8s().increment_creates();
                            *e = pod_meta_data.clone()
                        })
                        .or_insert_with(|| {
                            Metrics::k8s().increment_creates();
                            pod_meta_data
                        });
                }
            }
        }
        Ok(())
    }
}

impl Middleware for K8sMetadata {
    fn run(&self) {
        let mut runtime = self
            .runtime
            .lock()
            .take()
            .expect("tokio runtime not initialized");

        runtime.block_on(async move {
            let watcher = watcher(self.api.clone(), ListParams::default());
            watcher
                .into_stream()
                .filter_map(|r| async {
                    match r {
                        Ok(event) => Some(event),
                        Err(e) => {
                            log::warn!("k8s watch stream error: {}", e);
                            None
                        }
                    }
                })
                .for_each(|p| async {
                    self.handle_pod(p)
                        .unwrap_or_else(|e| log::warn!("unable to process pod event: {}", e));
                })
                .await;
        });
    }

    fn process<'a>(&self, line: &'a mut dyn LineMetaMut) -> Status<&'a mut dyn LineMetaMut> {
        if let Some(ref file_name) = line.get_file() {
            if let Some(key) = parse_container_path(&file_name) {
                if let Some(pod_meta_data) = self.metadata.lock().get(&key) {
                    if line
                        .set_annotations(pod_meta_data.annotations.clone())
                        .is_err()
                    {
                        return Status::Skip;
                    };
                    if line.set_labels(pod_meta_data.labels.clone()).is_err() {
                        return Status::Skip;
                    };
                }
            }
        }
        Status::Ok(line)
    }
}

impl TryFrom<k8s_openapi::api::core::v1::Pod> for PodMetadata {
    type Error = K8sError;

    fn try_from(value: k8s_openapi::api::core::v1::Pod) -> Result<Self, Self::Error> {
        let real_pod_meta = value.metadata;

        let name = match real_pod_meta.name {
            Some(v) => v,
            None => {
                return Err(K8sError::PodMissingMetaError("metadata.name"));
            }
        };
        let namespace = match real_pod_meta.namespace {
            Some(v) => v,
            None => {
                return Err(K8sError::PodMissingMetaError("metadata.namespace"));
            }
        };

        Ok(PodMetadata {
            name,
            namespace,
            labels: real_pod_meta
                .labels
                .map_or_else(KeyValueMap::new, |v| v.into()),
            annotations: real_pod_meta
                .annotations
                .map_or_else(KeyValueMap::new, |v| v.into()),
        })
    }
}

#[derive(Clone)]
struct PodMetadata {
    name: String,
    namespace: String,
    labels: KeyValueMap,
    annotations: KeyValueMap,
}

use crate::errors::K8sError;
use crate::middleware::parse_container_path;
use futures::stream::StreamExt;
use http::types::body::{KeyValueMap, LineBuilder};
use k8s_openapi::api::core::v1::Pod;
use kube::{
    api::{ListParams, WatchEvent},
    config::Config,
    runtime::Informer,
    Api, Client,
};
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
    informer: Mutex<Informer<Pod>>,
    runtime: Mutex<Option<Runtime>>,
}

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
                informer: Mutex::new(Informer::new(Api::all(client)).params(params)),
                runtime: Mutex::new(None),
            })
        });

        if let Ok(ref middleware) = this {
            *middleware.runtime.lock() = Some(runtime);
        }
        this
    }

    fn handle_pod(&self, event: WatchEvent<Pod>) {
        match event {
            WatchEvent::Added(pod) => {
                let pod_meta_data = match PodMetadata::try_from(pod) {
                    Ok(v) => v,
                    Err(e) => {
                        warn!("ignoring pod added event: {}", e);
                        return;
                    }
                };
                self.metadata.lock().insert(
                    (pod_meta_data.name.clone(), pod_meta_data.namespace.clone()),
                    pod_meta_data,
                );
                Metrics::k8s().increment_creates();
            }
            WatchEvent::Modified(pod) => {
                let new_pod_meta_data = match PodMetadata::try_from(pod) {
                    Ok(v) => v,
                    Err(e) => {
                        warn!("ignoring pod modified event: {}", e);
                        return;
                    }
                };
                if let Some(old_pod_meta_data) = self
                    .metadata
                    .lock()
                    .get_mut(&(new_pod_meta_data.name, new_pod_meta_data.namespace))
                {
                    old_pod_meta_data.labels = new_pod_meta_data.labels;
                    old_pod_meta_data.annotations = new_pod_meta_data.annotations;
                }
            }
            WatchEvent::Deleted(pod) => {
                let pod_meta_data = match PodMetadata::try_from(pod) {
                    Ok(v) => v,
                    Err(e) => {
                        warn!("ignoring pod deleted event: {}", e);
                        return;
                    }
                };
                self.metadata
                    .lock()
                    .remove(&(pod_meta_data.name, pod_meta_data.namespace));
                Metrics::k8s().increment_deletes();
            }
            WatchEvent::Bookmark(pod) => {
                let pod_meta_data = match PodMetadata::try_from(pod) {
                    Ok(v) => v,
                    Err(e) => {
                        warn!("ignoring pod deleted event: {}", e);
                        return;
                    }
                };
                debug!(
                    "got bookmark event for pod=\"{}\",namespace=\"{}\"",
                    &pod_meta_data.name, &pod_meta_data.namespace
                )
            }
            WatchEvent::Error(e) => {
                debug!("kubernetes api error event: {:?}", e);
            }
        }
    }
}

impl Middleware for K8sMetadata {
    fn run(&self) {
        let mut runtime = self
            .runtime
            .lock()
            .take()
            .expect("tokio runtime not initialized");
        let informer = self.informer.lock();

        runtime.block_on(async move {
            loop {
                let mut pods = match informer.poll().await {
                    Ok(v) => v.boxed(),
                    Err(e) => {
                        error!("unable to poll kubernetes api for pods: {}", e);
                        continue;
                    }
                };
                Metrics::k8s().increment_polls();

                while let Some(Ok(event)) = pods.next().await {
                    self.handle_pod(event);
                }
            }
        });
    }

    fn process(&self, mut lines: Vec<LineBuilder>) -> Status {
        for line in lines.iter_mut() {
            if let Some(ref file_name) = line.file {
                if let Some(key) = parse_container_path(&file_name) {
                    if let Some(pod_meta_data) = self.metadata.lock().get(&key) {
                        line.annotations = pod_meta_data.annotations.clone().into();
                        line.labels = pod_meta_data.labels.clone().into();
                    }
                }
            }
        }
        Status::Ok(lines)
    }
}

impl TryFrom<k8s_openapi::api::core::v1::Pod> for PodMetadata {
    type Error = K8sError;

    fn try_from(value: k8s_openapi::api::core::v1::Pod) -> Result<Self, Self::Error> {
        let real_pod_meta = match value.metadata {
            Some(v) => v,
            None => {
                return Err(K8sError::PodMissingMetaError("metadata"));
            }
        };

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

struct PodMetadata {
    name: String,
    namespace: String,
    labels: KeyValueMap,
    annotations: KeyValueMap,
}

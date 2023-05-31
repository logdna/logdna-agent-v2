use crate::errors::K8sError;
use crate::middleware::parse_container_path;
use futures::StreamExt;
use futures::TryStreamExt;
use http::types::body::{KeyValueMap, LineBufferMut};
use k8s_openapi::api::core::v1::Pod;
use kube::{
    api::ListParams,
    runtime::{
        reflector,
        reflector::ObjectRef,
        utils::StreamBackoff,
        watcher::{watcher, Event as WatcherEvent},
    },
    Api, Client, ResourceExt,
};
use serde_json::json;
use std::{fmt, sync::Mutex};

use backoff::ExponentialBackoff;
use metrics::Metrics;
use middleware::{Middleware, Status};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{error, trace, warn};

#[derive(Error, Debug)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Utf(#[from] std::string::FromUtf8Error),
    #[error(transparent)]
    K8s(#[from] kube::Error),
}

#[derive(Debug)]
pub enum LogEvent {
    New,
    Update,
    Delete,
}

impl fmt::Display for LogEvent {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            LogEvent::New => write!(f, "registering new pod"),
            LogEvent::Update => write!(f, "updating existing pod"),
            LogEvent::Delete => write!(f, "deleting pod"),
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct MetaObject {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename(serialize = "Image Name"))]
    pub image_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename(serialize = "Tag"))]
    pub tag: Option<String>,
}

pub struct StoreSwapIntent<'a> {
    parent: &'a K8sMetadata,
    store: reflector::Store<Pod>,
}

impl<'a> StoreSwapIntent<'a> {
    fn new(parent: &'a K8sMetadata, store: reflector::Store<Pod>) -> StoreSwapIntent<'a> {
        StoreSwapIntent { parent, store }
    }

    pub fn swap(self) {
        *self.parent.store.lock().unwrap() = Some(self.store);
    }
}

#[derive(Default)]
pub struct K8sMetadata {
    store: Mutex<Option<reflector::Store<Pod>>>,
}

impl K8sMetadata {
    pub fn kick_over(
        &self,
        client: Client,
        node_name: Option<&str>,
    ) -> Result<
        (
            impl futures::StreamExt<Item = WatcherEvent<Pod>>,
            StoreSwapIntent,
        ),
        Error,
    > {
        let api = Api::<Pod>::all(client);

        let store_writer = reflector::store::Writer::default();
        let store = store_writer.as_reader();

        let params = if let Some(node) = node_name {
            ListParams::default().fields(&format!("spec.nodeName={}", node))
        } else {
            ListParams::default()
        };

        let watched = StreamBackoff::new(watcher(api, params), ExponentialBackoff::default());
        let swap_intent = StoreSwapIntent::new(self, store.clone());

        Ok((
            reflector(store_writer, watched)
                .map_ok(|event| {
                    event.modify(|pod| {
                        pod.managed_fields_mut().clear();
                    })
                })
                .inspect(move |event| match event {
                    Ok(p) => {
                        K8sMetadata::handle_pod(&store, p)
                            .unwrap_or_else(|e| warn!("unable to process pod event: {}", e));
                    }
                    Err(e) => warn!("k8s watch stream error: {}", e),
                })
                .take_while(|x| std::future::ready(x.is_ok()))
                .map(Result::unwrap),
            swap_intent,
        ))
    }

    fn handle_pod(
        reader: &reflector::Store<Pod>,
        event: &WatcherEvent<Pod>,
    ) -> Result<(), K8sError> {
        match event {
            WatcherEvent::Applied(pod) => {
                let obj_ref = ObjectRef::from_obj(pod);
                if reader.get(&obj_ref).is_none() {
                    Metrics::k8s().increment_creates();
                    log_watcher_pod(LogEvent::New, pod)
                } else {
                    log_watcher_pod(LogEvent::Update, pod)
                }
            }
            WatcherEvent::Deleted(pod) => {
                Metrics::k8s().increment_deletes();
                log_watcher_pod(LogEvent::Delete, pod)
            }
            WatcherEvent::Restarted(pods) => {
                trace!("registering all pods...");
                for pod in pods {
                    Metrics::k8s().increment_creates();
                    log_watcher_pod(LogEvent::New, pod)
                }
            }
        }
        Ok(())
    }
}

impl Middleware for K8sMetadata {
    fn run(&self) {}

    #[allow(clippy::question_mark)]
    fn process<'a>(&self, line: &'a mut dyn LineBufferMut) -> Status<&'a mut dyn LineBufferMut> {
        if let Some(file_name) = line.get_file() {
            if let Some(parse_result) = parse_container_path(file_name) {
                let obj_ref =
                    ObjectRef::new(&parse_result.pod_name).within(&parse_result.pod_namespace);
                if let Some(ref store) = *self.store.lock().unwrap() {
                    if let Some(pod) = store.get(&obj_ref) {
                        let meta_object =
                            extract_image_name_and_tag(parse_result.container_name, pod.as_ref());
                        if meta_object.is_some() && line.set_meta(json!(meta_object)).is_err() {
                            trace!("Unable to set meta object{:?}", meta_object);
                            return Status::Skip;
                        }

                        if let Some(ref annotations) = pod.metadata.annotations {
                            if line
                                .set_annotations(
                                    annotations
                                        .iter()
                                        .fold(KeyValueMap::new(), |acc, (k, v)| acc.add(k, v)),
                                )
                                .is_err()
                            {
                                return Status::Skip;
                            };
                        }

                        if let Some(ref labels) = pod.metadata.labels {
                            if line
                                .set_labels(
                                    labels
                                        .iter()
                                        .fold(KeyValueMap::new(), |acc, (k, v)| acc.add(k, v)),
                                )
                                .is_err()
                            {
                                return Status::Skip;
                            };
                        }
                    }
                }
            }
        }
        Status::Ok(line)
    }
}

fn extract_image_name_and_tag(container_name: String, pod: &Pod) -> Option<MetaObject> {
    if let Some(spec) = &pod.spec {
        for container in &spec.containers {
            if container.name.eq_ignore_ascii_case(&container_name) && container.image.is_some() {
                let container_image = container.image.clone().unwrap();

                if let Some(split) = container_image.split_once(':') {
                    let image = split.0.to_string();
                    let image_tag = split.1.to_string();
                    return Some(MetaObject {
                        image_name: Some(image),
                        tag: Some(image_tag),
                    });
                } else {
                    let image = container_image;
                    return Some(MetaObject {
                        image_name: Some(image),
                        tag: None,
                    });
                }
            }
        }
    }

    None
}

fn log_watcher_pod(log_event: LogEvent, pod: &Pod) {
    trace!(
        "{} \"{}\" in namespace \"{}\"",
        log_event,
        pod.metadata
            .name
            .clone()
            .unwrap_or_else(|| "UNKNOWN POD".into()),
        pod.metadata
            .namespace
            .clone()
            .unwrap_or_else(|| "UNKNOWN NAMESPACE".into())
    );
    match log_event {
        LogEvent::New | LogEvent::Update => {
            trace!(
                "\n\tlabels = {:?}\n\tannotations = {:?}",
                pod.metadata.labels.clone().unwrap_or_default(),
                pod.metadata.annotations.clone().unwrap_or_default()
            )
        }
        _ => (),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::types::body::{LineBuilder, LineMeta};
    use k8s_openapi::api::core::v1::{Container, PodSpec};
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
    use std::collections::BTreeMap;
    use std::convert::TryFrom;

    #[cfg(test)]
    impl TryFrom<&k8s_openapi::api::core::v1::Pod> for PodMetadata {
        type Error = K8sError;

        fn try_from(value: &k8s_openapi::api::core::v1::Pod) -> Result<Self, Self::Error> {
            let real_pod_meta = &value.metadata;

            let name = match real_pod_meta.name {
                Some(ref v) => v,
                None => {
                    return Err(K8sError::PodMissingMetaError("metadata.name"));
                }
            };
            let namespace = match real_pod_meta.namespace {
                Some(ref v) => v,
                None => {
                    return Err(K8sError::PodMissingMetaError("metadata.namespace"));
                }
            };

            Ok(PodMetadata {
                name: name.to_string(),
                namespace: namespace.to_string(),
                labels: real_pod_meta.labels.clone().take().unwrap_or_default(),
                //.map_or_else(KeyValueMap::new, |v| v.iter().fold(KeyValueMap::new(), |acc, (k, v)| acc.add(k, v))),
                annotations: real_pod_meta.annotations.clone().take().unwrap_or_default(), //.as_ref().map_or_else(KeyValueMap::new, |v| v.iter().fold(KeyValueMap::new(), |acc, (k, v)| acc.add(k, v))),
            })
        }
    }

    #[derive(Clone, Default)]
    #[cfg(test)]
    struct PodMetadata {
        name: String,
        namespace: String,
        labels: BTreeMap<String, String>,
        annotations: BTreeMap<String, String>,
    }

    #[tokio::test]
    async fn test_process_with_file_that_can_not_be_parsed() {
        let k8s_meta = get_instance(Vec::new());
        let mut line = LineBuilder::new().line("abc").file("abc.log");
        let result = k8s_meta.process(&mut line);
        assert!(matches!(&result, Status::Ok(_)));
        if let Status::Ok(l) = result {
            assert!(l.get_annotations().is_none());
            assert!(l.get_labels().is_none());
        }
    }

    #[tokio::test]
    async fn test_process_with_different_files() {
        let matching_file1 = "/var/log/containers/first_file_sample-f39155eb652f5161f4a34b1fbd89a4d361e76ccb6c3cdc0e2c18e0d0abb26516.log";
        let matching_file2 = "/var/log/containers/second_file_sample-f39155eb652f5161f4a34b1fbd89a4d361e76ccb6c3cdc0e2c18e0d0abb26516.log";
        let k8s_meta = get_instance(vec![get_pod_metadata("first", "file")]);
        let mut lines = vec![
            LineBuilder::new().line("line 0").file(matching_file1),
            // 1: File not matching
            LineBuilder::new()
                .line("line 1")
                .file("/tmp/not_matching_file.log"),
            LineBuilder::new().line("line 2").file(matching_file1),
            // 3: The file matches but there's no metadata for it
            LineBuilder::new().line("line 3").file(matching_file2),
            // 4..6 Repeat multiple times w/ file matches with metadata
            LineBuilder::new().line("line 4").file(matching_file1),
            LineBuilder::new().line("line 5").file(matching_file1),
        ];

        for (i, line) in lines.iter_mut().enumerate() {
            let result = k8s_meta.process(line);
            if let Status::Ok(_) = result {
                assert_eq!(line.line, Some(format!("line {}", i)));
                if i == 1 || i == 3 {
                    assert!(line.get_annotations().is_none());
                    assert!(line.get_labels().is_none());
                } else {
                    assert!(line.get_annotations().is_some());
                    assert!(line.get_labels().is_some());
                }
            } else {
                panic!("Unexpected status");
            }
        }
    }

    #[tokio::test]
    async fn test_extract_image_tag() {
        let test_pod = create_pod(Some("test:tag".to_string()));
        let result = extract_image_name_and_tag("Container".to_string(), &test_pod).unwrap();

        assert_eq!("test".to_string(), result.image_name.unwrap());
        assert_eq!("tag".to_string(), result.tag.unwrap());
    }

    #[tokio::test]
    async fn test_extract_image_tag_no_tag() {
        let test_pod = create_pod(Some("test".to_string()));
        let result = extract_image_name_and_tag("Container".to_string(), &test_pod).unwrap();

        assert_eq!("test".to_string(), result.image_name.unwrap());
        assert_eq!(None, result.tag);
    }

    #[tokio::test]
    async fn test_extract_image_tag_no_value() {
        let test_pod = create_pod(None);
        let result = extract_image_name_and_tag("Container".to_string(), &test_pod);

        assert!(result.is_none());
    }

    fn create_pod(image: Option<String>) -> Pod {
        let meta = ObjectMeta {
            ..Default::default()
        };

        let spec = PodSpec {
            containers: vec![Container {
                name: "Container".to_string(),
                image,
                ..Default::default()
            }],
            ..Default::default()
        };

        Pod {
            metadata: meta,
            spec: Some(spec),
            status: None,
        }
    }

    fn get_instance(pods: Vec<PodMetadata>) -> K8sMetadata {
        let mut store_w = reflector::store::Writer::default();
        for meta in pods.into_iter() {
            let PodMetadata {
                name,
                namespace,
                labels,
                annotations,
            } = meta;
            let pod_meta = ObjectMeta {
                name: Some(name),
                namespace: Some(namespace),
                labels: Some(labels),
                annotations: Some(annotations),
                ..Default::default()
            };
            let pod = Pod {
                metadata: pod_meta,
                spec: None,
                status: None,
            };
            store_w.apply_watcher_event(&WatcherEvent::Applied(pod));
        }
        K8sMetadata {
            store: Mutex::new(Some(store_w.as_reader())),
        }
    }

    fn get_pod_metadata(name: impl Into<String>, namespace: impl Into<String>) -> PodMetadata {
        PodMetadata {
            name: name.into(),
            namespace: namespace.into(),
            labels: Default::default(),
            annotations: Default::default(),
        }
    }
}

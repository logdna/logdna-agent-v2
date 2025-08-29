use crate::errors::K8sError;
use crate::middleware::{parse_container_path, ParseResult};
use crate::K8S_WATCHER_TIMEOUT;
use middleware::MiddlewareError;
use rate_limit_macro::rate_limit;

use std::{
    collections::HashMap,
    fmt,
    ops::DerefMut,
    sync::{Arc, Mutex},
};

use arc_interner::ArcIntern;
use async_channel::Receiver;

use futures::{
    stream::{self, select},
    StreamExt, TryStreamExt,
};
use http::types::body::{KeyValueMap, LineBufferMut};
use k8s_openapi::api::core::v1::{Container, Pod, PodSpec};

use kube::{
    runtime::{
        reflector,
        reflector::ObjectRef,
        utils::StreamBackoff,
        watcher::{watcher, Config as WatcherConfig, Event as WatcherEvent},
    },
    Api, Client, ResourceExt,
};
use serde_json::json;

use backoff::ExponentialBackoff;
use metrics::Metrics;
use middleware::{Middleware, Status};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, error, trace, warn};

#[cfg(feature = "integration_tests")]
lazy_static::lazy_static! {
    static ref MOCK_NO_PODS: bool = std::env::var(config::env_vars::MOCK_NO_PODS).is_ok();
}

#[derive(Error, Debug)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Utf(#[from] std::string::FromUtf8Error),
    #[error(transparent)]
    K8s(#[from] kube::Error),
    #[error("Failed to lock state")]
    K8sMiddlewareState,
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

    pub fn swap(self) -> Result<(), Error> {
        self.parent
            .state
            .lock()
            .as_mut()
            .map(|state| {
                state.store = Some(self.store);
            })
            .map_err(|_| Error::K8sMiddlewareState)
    }
}

#[derive(Eq, Hash, PartialEq, Clone, Debug)]
pub struct ContainerIdent {
    name: ArcIntern<String>,
}

#[derive(Eq, Hash, PartialEq, Clone, Debug)]
pub struct PodIdent {
    name: ArcIntern<String>,
}

#[derive(Eq, Hash, PartialEq, Clone, Debug)]
struct PodContainers {
    pod_ident: PodIdent,
    containers: Vec<ContainerIdent>,
}

impl From<&ParseResult> for ContainerIdent {
    fn from(parse_result: &ParseResult) -> Self {
        ContainerIdent {
            name: ArcIntern::new(parse_result.container_name.clone()),
        }
    }
}

impl From<&ParseResult> for PodIdent {
    fn from(parse_result: &ParseResult) -> Self {
        let mut pod_namespace = parse_result.pod_namespace.clone();
        let pod_name = parse_result.pod_name.clone();
        pod_namespace.push('-');
        pod_namespace.push_str(&pod_name);
        PodIdent {
            name: ArcIntern::new(pod_namespace),
        }
    }
}

impl From<(&str, &str)> for PodIdent {
    fn from((pod_namespace, pod_name): (&str, &str)) -> Self {
        let mut buf = pod_namespace.to_string();
        buf.push('-');
        buf.push_str(pod_name);
        PodIdent {
            name: ArcIntern::new(buf),
        }
    }
}

impl From<&Pod> for PodContainers {
    fn from(pod: &Pod) -> Self {
        let pod_ident: PodIdent = pod
            .metadata
            .namespace
            .as_deref()
            .zip(pod.metadata.name.as_deref())
            .map(|pair| pair.into())
            .unwrap();
        Self {
            pod_ident,
            containers: pod
                .spec
                .as_ref()
                .map(|spec| {
                    spec.containers
                        .iter()
                        .map(|container| ContainerIdent {
                            name: ArcIntern::new(container.name.clone()),
                        })
                        .collect()
                })
                .unwrap_or_default(),
        }
    }
}

#[derive(Default)]
struct DeletionState {
    pending_deletions: HashMap<PodIdent, WatcherEvent<Pod>>,
    pod_lookup: HashMap<PodIdent, PodContainers>,
}

#[derive(Default)]
struct K8sMetadataState {
    store: Option<reflector::Store<Pod>>,
    deletion_state: Arc<tokio::sync::Mutex<DeletionState>>,
}

pub struct K8sMetadata {
    state: Mutex<K8sMetadataState>,
    deletion_ack_recv: Arc<Receiver<Vec<std::path::PathBuf>>>,
}

impl K8sMetadata {
    pub fn new(deletion_ack_recv: Receiver<Vec<std::path::PathBuf>>) -> Self {
        Self {
            state: Mutex::new(K8sMetadataState::default()),
            deletion_ack_recv: Arc::new(deletion_ack_recv),
        }
    }

    #[cfg(test)]
    pub fn with_store(
        deletion_ack_recv: Receiver<Vec<std::path::PathBuf>>,
        store: reflector::Store<Pod>,
    ) -> Self {
        Self {
            state: Mutex::new(K8sMetadataState {
                store: Some(store),
                ..Default::default()
            }),
            deletion_ack_recv: Arc::new(deletion_ack_recv),
        }
    }

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

        let wc = WatcherConfig::default()
            .timeout(K8S_WATCHER_TIMEOUT)
            .any_semantic();

        let wc = if let Some(node) = node_name {
            wc.fields(&format!("spec.nodeName={node}"))
        } else {
            wc
        };

        let watcher = watcher(api, wc).map_ok(|ev| {
            ev.modify(|pod| {
                pod.managed_fields_mut().clear();
                pod.status = None;

                if let Some(pod_spec) = pod.spec.as_mut() {
                    let orig = std::mem::take(pod_spec);
                    let filtered_pod_spec = PodSpec {
                        containers: orig
                            .containers
                            .into_iter()
                            .map(|container| Container {
                                name: container.name,
                                image: container.image,
                                ..Default::default()
                            })
                            .collect(),
                        ..Default::default()
                    };
                    *pod_spec = filtered_pod_spec;
                }
            })
        });

        let watcher = StreamBackoff::new(watcher, ExponentialBackoff::default());

        let (deletion_ack_recv, deletion_state) = {
            let state = self.state.lock().map_err(|_| Error::K8sMiddlewareState)?;
            // Take the deletion ack stream, copy the deletion state stream
            (self.deletion_ack_recv.clone(), state.deletion_state.clone())
        };

        let delete_stream = stream::unfold(
            (deletion_state.clone(), deletion_ack_recv),
            move |(deletion_state, deletion_ack_recv)| async move {
                let next_event = loop {
                    let (deleted_container, deleted_container_pod) = match deletion_ack_recv
                        .recv()
                        .await
                        .map(|deleted_container_paths| {
                            deleted_container_paths
                                .into_iter()
                                .filter_map(|path| {
                                    path.into_os_string()
                                        .into_string()
                                        .ok()
                                        .as_deref()
                                        .and_then(parse_container_path)
                                })
                                .next()
                                .map(|parse_result| {
                                    (
                                        <&ParseResult as Into<ContainerIdent>>::into(&parse_result),
                                        <&ParseResult as Into<PodIdent>>::into(&parse_result),
                                    )
                                })
                        }) {
                        Ok(Some(tuple)) => tuple,
                        Err(e) => {
                            error!("unable to receive delete events: {}", e);
                            return None;
                        }
                        _ => continue,
                    };

                    let mut lock_guard = deletion_state.lock().await;
                    let DeletionState {
                        ref mut pod_lookup,
                        ref mut pending_deletions,
                    } = lock_guard.deref_mut();
                    if let std::collections::hash_map::Entry::Occupied(pod_containers_entry) =
                        pod_lookup
                            .entry(deleted_container_pod.clone())
                            .and_modify(|pod| {
                                pod.containers
                                    .retain(|container| container.name != deleted_container.name);
                            })
                    {
                        let pod_containers = pod_containers_entry.get();
                        break match Ok(pod_containers
                            .containers
                            .is_empty()
                            .then(|| pending_deletions.remove(&deleted_container_pod))
                            .flatten())
                        .transpose()
                        {
                            Some(event) => {
                                pod_containers_entry.remove();
                                Some(event)
                            }
                            _ => continue,
                        };
                    } else {
                        continue;
                    }
                };
                next_event
                    .map(move |next_event| (Some(next_event), (deletion_state, deletion_ack_recv)))
            },
        );

        let watch_stream = stream::unfold(
            (Box::pin(watcher), deletion_state),
            move |(mut watcher, deletion_state)| async move {
                // Check if we have a delete acknowledgement
                let next_event = loop {
                    let next_event = {
                        // No deletes to process, just read from the watcher
                        let next_event = watcher.next().await;
                        if let Some(Ok(event)) = next_event {
                            let pod_container: Option<PodContainers> =
                                if let WatcherEvent::Deleted(ref pod) = event {
                                    Some(pod.into())
                                } else {
                                    None
                                };

                            if let Some(pod_container) = pod_container {
                                let mut lock_guard = deletion_state.lock().await;
                                let DeletionState {
                                    ref mut pod_lookup,
                                    ref mut pending_deletions,
                                } = lock_guard.deref_mut();

                                pod_lookup
                                    .insert(pod_container.pod_ident.clone(), pod_container.clone());
                                pending_deletions.insert(pod_container.pod_ident, event);
                                continue;
                            } else {
                                Some(Ok(event))
                            }
                        } else {
                            next_event
                        }
                    };

                    break next_event;
                };
                next_event.map(move |next_event| (Some(next_event), (watcher, deletion_state)))
            },
        );

        let stream = select(delete_stream, watch_stream).filter_map(std::future::ready);

        let swap_intent = StoreSwapIntent::new(self, store.clone());

        Ok((
            reflector(store_writer, stream)
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
                if let Some(ref store) = self.state.lock().unwrap().store {
                    if let Some(pod) = store.get(&obj_ref) {
                        let meta_object =
                            extract_image_name_and_tag(parse_result.container_name, pod.as_ref());
                        if meta_object.is_some() && line.set_meta(json!(meta_object)).is_err() {
                            debug!("Unable to set meta object{:?}", meta_object);
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
                                debug!("Unable to set annotations {:?}", annotations);
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
                                debug!("Unable to set labels {:?}", labels);
                                return Status::Skip;
                            };
                        }
                    } else {
                        trace!("pod metadata is not available {:?}", obj_ref);
                        return Status::Ok(line);
                    }
                }
            }
        }
        Status::Ok(line)
    }

    fn validate<'a>(
        &self,
        line: &'a dyn LineBufferMut,
    ) -> Result<&'a dyn LineBufferMut, MiddlewareError> {
        // here we retry only pod log lines that do not have k8s metadata
        let file_name = line.get_file().unwrap_or("");
        rate_limit!(rate = 1, interval = 5, {
            debug!("validate line from file: '{:?}'", file_name);
        });
        if let Some(parse_result) = parse_container_path(file_name) {
            #[cfg(feature = "integration_tests")]
            if *MOCK_NO_PODS {
                return Err(MiddlewareError::Retry(self.name().into()));
            }

            if let Some(ref store) = self.state.lock().unwrap().store {
                let obj_ref =
                    ObjectRef::new(&parse_result.pod_name).within(&parse_result.pod_namespace);
                if store.get(&obj_ref).is_some() {
                    return Ok(line);
                }
            }

            // line does not have metadata yet
            return Err(MiddlewareError::Retry(self.name().into()));
        }
        Ok(line)
    }

    fn name(&self) -> &'static str {
        std::any::type_name::<K8sMetadata>()
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
    use k8s_openapi::api::core::v1::Container;
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
                labels: real_pod_meta.labels.clone().unwrap_or_default(),
                //.map_or_else(KeyValueMap::new, |v| v.iter().fold(KeyValueMap::new(), |acc, (k, v)| acc.add(k, v))),
                annotations: real_pod_meta.annotations.clone().unwrap_or_default(), //.as_ref().map_or_else(KeyValueMap::new, |v| v.iter().fold(KeyValueMap::new(), |acc, (k, v)| acc.add(k, v))),
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
                assert_eq!(line.line, Some(format!("line {i}")));
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
        K8sMetadata::with_store(async_channel::unbounded().1, store_w.as_reader())
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

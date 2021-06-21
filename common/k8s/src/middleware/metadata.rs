use crate::errors::K8sError;
use crate::middleware::parse_container_path;
use futures::StreamExt;
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
    Api, Client,
};
use std::pin::Pin;

use backoff::ExponentialBackoff;
use metrics::Metrics;
use middleware::{Middleware, Status};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Utf(#[from] std::string::FromUtf8Error),
    #[error(transparent)]
    K8s(#[from] kube::Error),
}

pub struct K8sMetadata {
    store: reflector::Store<Pod>,
}

impl K8sMetadata {
    pub async fn new(
        client: Client,
        node_name: Option<&str>,
    ) -> Result<(Pin<Box<dyn futures::Future<Output = ()> + Send>>, Self), Error> {
        let api = Api::<Pod>::all(client);

        let store_writer = reflector::store::Writer::default();
        let store = store_writer.as_reader();

        let params = if let Some(node) = node_name {
            ListParams::default().fields(&format!("spec.nodeName={}", node))
        } else {
            ListParams::default()
        };

        let watched = StreamBackoff::new(watcher(api, params), ExponentialBackoff::default());

        let reflector = reflector(store_writer, watched);

        let driver = {
            let store = store.clone();
            async move {
                reflector
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
                        K8sMetadata::handle_pod(&store, p)
                            .unwrap_or_else(|e| log::warn!("unable to process pod event: {}", e));
                    })
                    .await
            }
        };

        Ok((Box::pin(driver), K8sMetadata { store }))
    }

    fn handle_pod(
        reader: &reflector::Store<Pod>,
        event: WatcherEvent<Pod>,
    ) -> Result<(), K8sError> {
        match event {
            WatcherEvent::Applied(pod) => {
                let obj_ref = ObjectRef::from_obj(&pod);
                if reader.get(&obj_ref).is_none() {
                    Metrics::k8s().increment_creates();
                }
            }
            WatcherEvent::Deleted(_) => {
                Metrics::k8s().increment_deletes();
            }
            WatcherEvent::Restarted(pods) => {
                for _ in pods {
                    Metrics::k8s().increment_creates();
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
                if let Some(pod) = self.store.get(&obj_ref) {
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
        Status::Ok(line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::types::body::{LineBuilder, LineMeta};
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
            store: store_w.as_reader(),
        }
    }

    #[tokio::test]
    #[cfg(target_os = "linux")]
    async fn test_init_max_elapsed_time() {
        let config = Config::new("https://127.0.0.10/".parse::<Uri>().unwrap());
        let client = Client::try_from(config).unwrap();
        let start = Instant::now();
        let max_time = Duration::from_millis(2000);

        // It error out
        assert!(matches!(
            K8sMetadata::initialize(&client, max_time).await,
            Err(_)
        ));

        // Some time passed
        assert!(start.elapsed() > Duration::from_millis(1000));
        assert!(start.elapsed() < max_time * 2);
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

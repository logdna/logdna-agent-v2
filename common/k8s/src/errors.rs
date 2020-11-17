use thiserror::Error;

#[derive(Clone, Debug, Error)]
pub enum K8sError {
    #[error("pod missing {0}")]
    PodMissingMetaError(&'static str),
    #[error("failed to initialize kubernetes middleware {0}")]
    InitializationError(String),
}

#[derive(Debug, Error)]
pub enum K8sEventStreamError {
    #[error(transparent)]
    WatcherError(kube_runtime::watcher::Error),
    #[error(transparent)]
    SerializationError(#[from] serde_json::Error),
}

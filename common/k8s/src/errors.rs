use thiserror::Error;

#[derive(Debug, Error)]
pub enum K8sError {
    #[error("pod missing {0}")]
    PodMissingMetaError(&'static str),
    #[error("failed to initialize kubernetes middleware {0}")]
    InitializationError(String),
    #[error(transparent)]
    K8sError(#[from] kube::Error),
    #[error(transparent)]
    K8sInClusterError(#[from] kube::config::InClusterError),
}

#[derive(Debug, Error)]
pub enum K8sEventStreamError {
    #[error(transparent)]
    WatcherError(kube::runtime::watcher::Error),
    #[error(transparent)]
    SerializationError(#[from] serde_json::Error),
    #[error(transparent)]
    K8sError(#[from] kube::Error),
}

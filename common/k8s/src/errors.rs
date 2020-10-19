use thiserror::Error;

#[derive(Clone, Debug, Error)]
pub enum K8sError {
    #[error("pod missing {0}")]
    PodMissingMetaError(&'static str),
    #[error("failed to initialize kubernetes middleware {0}")]
    InitializationError(String),
}

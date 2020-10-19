use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum WatchError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0:?} was not a valid utf8 path")]
    PathNonUtf8(PathBuf),
    #[error("Duplicate Watch")]
    Duplicate,
}

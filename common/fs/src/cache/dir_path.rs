use std::ops::Deref;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Error, std::fmt::Debug)]
pub enum DirPathBufError {
    #[error("{0:?} is not a directory")]
    NotADirPath(PathBuf),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

// Strongly typed wrapper around PathBuf, cannot be constructed unless
// the directory it's referring to exists
#[derive(std::fmt::Debug, Clone)]
pub struct DirPathBuf {
    inner: PathBuf,
}

impl Deref for DirPathBuf {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.inner
    }
}

impl std::convert::TryFrom<PathBuf> for DirPathBuf {
    type Error = DirPathBufError;
    fn try_from(path: PathBuf) -> Result<Self, Self::Error> {
        //TODO: We want to allow paths that are not yet present: LOG-10041
        // For now, prevent validation on Windows
        #[cfg(unix)]
        if std::fs::canonicalize(&path)?.is_dir() {
            Ok(DirPathBuf { inner: path })
        } else {
            Err(DirPathBufError::NotADirPath(path))
        }

        #[cfg(windows)]
        {
            Ok(DirPathBuf { inner: path })
        }
    }
}

impl std::convert::TryFrom<&Path> for DirPathBuf {
    type Error = String;
    fn try_from(path: &Path) -> Result<Self, Self::Error> {
        #[cfg(unix)]
        if path.is_dir() {
            Ok(DirPathBuf { inner: path.into() })
        } else {
            path.to_str().map_or_else(
                || Err("path is not a directory and cannot be formatted".into()),
                |path| Err(format!("{} is not a directory", path)),
            )
        }

        #[cfg(windows)]
        {
            Ok(DirPathBuf { inner: path.into() })
        }
    }
}

impl std::convert::AsRef<Path> for DirPathBuf {
    fn as_ref(&self) -> &Path {
        &self.inner
    }
}

impl From<DirPathBuf> for PathBuf {
    fn from(d: DirPathBuf) -> PathBuf {
        d.inner
    }
}

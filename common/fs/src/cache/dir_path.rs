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
#[derive(std::fmt::Debug, Clone, PartialEq)]
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
        //TODO: Need to fix error handling here
        match find_valid_path(Some(path.clone())) {
            Ok(p) => Ok(p),
            _ => Err(DirPathBufError::NotADirPath(path)),
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

fn find_valid_path(path: Option<PathBuf>) -> Result<DirPathBuf, DirPathBufError> {
    match path {
        Some(p) => {
            if p.is_dir() {
                return Ok(DirPathBuf { inner: p });
            }
            warn!("{:?} is not a directory; moving to parent directory", p);
            find_valid_path(level_up(&p))
        }
        None => Err(DirPathBufError::NotADirPath(path.unwrap())),
    }
}

fn level_up(path: &Path) -> Option<PathBuf> {
    let mut parent_path = PathBuf::new();
    match path.parent() {
        Some(p) => {
            parent_path.push(p);
            Some(parent_path)
        }
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_level_up() {
        let mut expe_pathbuf = PathBuf::new();
        expe_pathbuf.push("/test-directory");

        let new_pathbuf = level_up(Path::new("/test-directory/sub-directory"));
        assert_eq!(expe_pathbuf, new_pathbuf.unwrap());

        let root_path = level_up(Path::new(""));
        assert_eq!(None, root_path);
    }
}

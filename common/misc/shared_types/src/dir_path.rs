use lazy_static::lazy_static;
use regex::Regex;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use thiserror::Error;
use tracing::{error, warn};

#[cfg(not(windows))]
lazy_static! {
    static ref REGEX_FS_ROOT: Regex = Regex::new(r"^/$").unwrap();
}
#[cfg(windows)]
lazy_static! {
    static ref REGEX_FS_ROOT: Regex = Regex::new(r"^ [a-zA-Z]:[\\/]$").unwrap();
}

#[derive(Error, std::fmt::Debug)]
pub enum DirPathBufError {
    #[error("{0:?} is not a directory")]
    NotADirPath(PathBuf),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("directory config error occured")]
    EmptyDirPath(Option<PathBuf>),
}

// Strongly typed wrapper around PathBuf, cannot be constructed unless
// the directory it's referring to exists
#[derive(Eq, std::fmt::Debug, Clone, PartialEq)]
pub struct DirPathBuf {
    pub inner: PathBuf,
    pub postfix: Option<PathBuf>,
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
        match find_valid_path(Some(path.clone()), None) {
            Ok(p) => Ok(p),
            _ => Err(DirPathBufError::NotADirPath(path)),
        }
    }
}

impl std::convert::TryFrom<&Path> for DirPathBuf {
    type Error = String;
    fn try_from(path: &Path) -> Result<Self, Self::Error> {
        if path.is_dir() {
            Ok(DirPathBuf {
                inner: path.into(),
                postfix: None,
            })
        } else {
            path.to_str().map_or_else(
                || Err("path is not a directory and cannot be formatted".into()),
                |path| Err(format!("{path} is not a directory")),
            )
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

/// If a path does not exist, the function continue to move to the parent directory
/// until it finds a valid directory. When a valid directory is found, it prepend the
/// missing piece of the directory path to `postfix` and returns the `DirPathBuf` result.
fn find_valid_path(
    path: Option<PathBuf>,
    postfix: Option<PathBuf>,
) -> Result<DirPathBuf, DirPathBufError> {
    match path {
        Some(some_path) => {
            if std::fs::canonicalize(&some_path).is_ok() {
                Ok(DirPathBuf {
                    inner: some_path,
                    postfix,
                })
            } else {
                warn!(
                    "{:?} is not a directory; moving to parent directory",
                    some_path
                );
                let mut some_postfix = PathBuf::new();
                if let Some(p) = postfix {
                    some_postfix.push(p);
                }
                let parent = match level_up(&some_path) {
                    Some(some_parent) => {
                        if REGEX_FS_ROOT
                            .is_match(some_parent.to_str().expect("invalid unicode path").as_ref())
                        {
                            warn!("{:?} does not exist up to the root level!", some_path);
                        }
                        some_parent
                    }
                    None => return Err(DirPathBufError::NotADirPath(some_path)),
                };
                let pop_path = some_path.strip_prefix(&parent).unwrap();
                let mut new_postfix = PathBuf::from(pop_path);
                new_postfix.push(some_postfix);
                find_valid_path(Some(parent), Some(new_postfix))
            }
        }
        None => Err(DirPathBufError::EmptyDirPath(path)),
    }
}

/// Given a path, returns an `Option<PathBuf>` of the parent directory.
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
    use std::env::temp_dir;

    use super::*;

    #[test]
    fn test_level_up() {
        let mut expe_pathbuf = PathBuf::new();
        expe_pathbuf.push("/test-directory");

        let new_pathbuf = level_up(Path::new("/test-directory/sub-directory"));
        assert_eq!(expe_pathbuf, new_pathbuf.unwrap());

        let invalid_path = level_up(Path::new("/"));
        assert_eq!(None, invalid_path);

        let root_path = level_up(Path::new(""));
        assert_eq!(None, root_path);
    }

    #[test]
    fn test_find_valid_path() {
        let mut test_path = PathBuf::new();
        let test_postfix = PathBuf::new();
        let mut expected_pathbuff = PathBuf::new();
        expected_pathbuff.push(Path::new("sub-test-path"));

        let tmp_dir = temp_dir();
        let tmp_test_dir = tmp_dir.join("test-dir-0");
        std::fs::create_dir(&tmp_test_dir).expect("could not create tmp directory");
        assert!(tmp_test_dir.is_dir());

        test_path.push(tmp_test_dir.join("sub-test-path"));
        let test_result = find_valid_path(Some(test_path), Some(test_postfix));
        assert_eq!(expected_pathbuff, test_result.unwrap().postfix.unwrap());

        // Clean up
        std::fs::remove_dir(&tmp_test_dir).expect("could not remove tmp directory");
        assert!(!tmp_test_dir.is_dir());
    }

    #[test]
    fn test_deep_find_valid_path() {
        let mut test_path = PathBuf::new();
        let test_postfix = PathBuf::new();
        let mut expected_pathbuff = PathBuf::new();
        expected_pathbuff.push(Path::new("sub-path/sub-sub-path"));

        let tmp_dir = temp_dir();
        let tmp_test_dir = tmp_dir.join("test-dir-1");
        std::fs::create_dir(&tmp_test_dir).expect("could not create tmp directory");
        assert!(tmp_test_dir.is_dir());

        test_path.push(tmp_test_dir.join("sub-path/sub-sub-path"));
        let test_result = find_valid_path(Some(test_path), Some(test_postfix));
        assert_eq!(expected_pathbuff, test_result.unwrap().postfix.unwrap());

        // Clean up
        std::fs::remove_dir(&tmp_test_dir).expect("could not remove tmp directory");
        assert!(!tmp_test_dir.is_dir());
    }

    #[test]
    fn test_filename_find_valid_path() {
        let mut test_path = PathBuf::new();
        let test_postfix = PathBuf::new();
        let mut expected_pathbuff = PathBuf::new();
        expected_pathbuff.push(Path::new("test_file.log/"));

        let tmp_dir = temp_dir();
        let tmp_test_dir = tmp_dir.join("test-dir-2");
        std::fs::create_dir(&tmp_test_dir).expect("could not create tmp directory");
        assert!(tmp_test_dir.is_dir());

        test_path.push(tmp_test_dir.join("test_file.log"));
        let test_result = find_valid_path(Some(test_path), Some(test_postfix));
        assert_eq!(expected_pathbuff, test_result.unwrap().postfix.unwrap());

        // Clean up
        std::fs::remove_dir(&tmp_test_dir).expect("could not remove tmp directory");
        assert!(!tmp_test_dir.is_dir());
    }

    #[test]
    fn test_empty_find_valid_path() {
        let mut test_path = PathBuf::new();
        let test_postfix = PathBuf::new();

        test_path.push(Path::new(""));
        let test_result = find_valid_path(Some(test_path), Some(test_postfix));
        assert!(test_result.is_err());
    }

    #[test]
    fn test_invalid_find_valid_path() {
        let mut test_path = PathBuf::new();
        let test_postfix = PathBuf::new();

        test_path.push(Path::new("ivc:"));
        let test_result = find_valid_path(Some(test_path), Some(test_postfix));
        assert!(test_result.is_err());
    }

    #[test]
    fn test_root_find_valid_path() {
        let mut test_path = PathBuf::new();
        let test_postfix = PathBuf::new();

        test_path.push(Path::new("/"));
        let test_result = find_valid_path(Some(test_path), Some(test_postfix));
        assert_eq!(Path::new(""), test_result.unwrap().postfix.unwrap());
    }

    #[test]
    fn test_root_lvl_find_valid_path() {
        let mut test_path = PathBuf::new();
        let test_postfix = PathBuf::new();
        let mut expected_pathbuff = PathBuf::new();
        expected_pathbuff.push(Path::new("does-not-exist"));

        test_path.push(Path::new("/does-not-exist"));
        let result = find_valid_path(Some(test_path), Some(test_postfix));
        assert_eq!(expected_pathbuff, result.unwrap().postfix.unwrap());
    }
}

use std::ops::Deref;
use std::path::{Path, PathBuf};
use thiserror::Error;

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
        #[cfg(unix)]
        match find_valid_path(Some(path.clone()), None) {
            Ok(p) => Ok(p),
            _ => Err(DirPathBufError::NotADirPath(path)),
        }

        #[cfg(windows)]
        {
            Ok(DirPathBuf {
                inner: path,
                postfix: None,
            })
        }
    }
}

impl std::convert::TryFrom<&Path> for DirPathBuf {
    type Error = String;
    fn try_from(path: &Path) -> Result<Self, Self::Error> {
        #[cfg(unix)]
        if path.is_dir() {
            Ok(DirPathBuf {
                inner: path.into(),
                postfix: None,
            })
        } else {
            path.to_str().map_or_else(
                || Err("path is not a directory and cannot be formatted".into()),
                |path| Err(format!("{} is not a directory", path)),
            )
        }

        #[cfg(windows)]
        {
            Ok(DirPathBuf {
                inner: path.into(),
                postfix: None,
            })
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
/// until it finds a valid directory.
///
/// When a valid directory is found, it adds the missing piece of the directory
/// path to `postfix` and returns the `DirPathBuf` result.
fn find_valid_path(
    path: Option<PathBuf>,
    postfix: Option<PathBuf>,
) -> Result<DirPathBuf, DirPathBufError> {
    match path {
        Some(p) => {
            if matches!(std::fs::canonicalize(&p), Ok(_)) {
                Ok(DirPathBuf { inner: p, postfix })
            } else {
                warn!("{:?} is not a directory; moving to parent directory", p);
                let root_pathbuf = Path::new("/");
                let mut postfix_pathbuf = PathBuf::new();
                let mut pop_pathbuf = PathBuf::new();
                if let Some(pop) = postfix {
                    pop_pathbuf.push(pop);
                }
                let parent = match level_up(&p) {
                    Some(p) => {
                        if p == root_pathbuf {
                            warn!("root level directory was missing, as a result configured directory recursed to the root level!");
                        }
                        p
                    }
                    None => return Err(DirPathBufError::NotADirPath(p)),
                };

                let tmp_dir = parent;
                let postfix_path = p.strip_prefix(tmp_dir).ok();
                postfix_pathbuf.push(postfix_path.unwrap());
                postfix_pathbuf.push(pop_pathbuf);
                find_valid_path(level_up(&p), Some(postfix_pathbuf))
            }
        }
        None => Err(DirPathBufError::EmptyDirPath(path)),
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

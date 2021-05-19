use std::cell::RefCell;
use std::path::{Path, PathBuf};

use crate::cache::Children;
use crate::cache::TailedFile;

use crate::cache::tailed_file::LazyLineSerializer;

#[derive(Debug)]
pub enum Entry {
    File {
        path: PathBuf,
        data: RefCell<TailedFile<LazyLineSerializer>>,
    },
    Dir {
        /// A map of entry keys by file name
        children: Children,
        path: PathBuf,
    },
    Symlink {
        /// The target of the symlink
        link: PathBuf,
        path: PathBuf,
    },
}

impl Entry {
    pub fn path(&self) -> &Path {
        match self {
            Entry::Dir { path: wd, .. }
            | Entry::Symlink { path: wd, .. }
            | Entry::File { path: wd, .. } => wd,
        }
    }

    pub fn set_path(&mut self, path: PathBuf) {
        match self {
            Entry::File { path: wd, .. }
            | Entry::Dir { path: wd, .. }
            | Entry::Symlink { path: wd, .. } => *wd = path,
        }
    }

    pub fn link(&self) -> Option<&PathBuf> {
        match self {
            Entry::Symlink { link, .. } => Some(link),
            _ => None,
        }
    }

    /// Gets a mutable reference to the children
    pub fn children(&mut self) -> Option<&mut Children> {
        match self {
            Entry::Dir { children, .. } => Some(children),
            _ => None,
        }
    }
}

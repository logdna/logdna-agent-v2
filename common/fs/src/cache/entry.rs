use std::cell::RefCell;
use std::path::{Path, PathBuf};

use crate::cache::guarded_option::GuardedOption;
use crate::cache::Children;
use crate::cache::TailedFile;

use crate::cache::tailed_file::LazyLineSerializer;

pub struct Entry {
    pub path: PathBuf,
    pub kind: EntryKind,
}

#[derive(Debug)]
pub enum EntryKind {
    File {
        data: RefCell<TailedFile<LazyLineSerializer>>,
    },
    Dir {
        /// A map of entry keys by file name
        children: Children,
        dependencies: Vec<PathBuf>,
    },
    Symlink {
        /// The target of the symlink
        target: GuardedOption<PathBuf>,
    },
}

impl Entry {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn kind(&self) -> &EntryKind {
        &self.kind
    }

    pub fn kind_mut(&mut self) -> &mut EntryKind {
        &mut self.kind
    }

    pub fn set_path(&mut self, path: PathBuf) {
        self.path = path
    }

    /// Gets a mutable reference to the children
    pub fn children(&mut self) -> Option<&mut Children> {
        match &mut self.kind {
            EntryKind::Dir { children, .. } => Some(children),
            _ => None,
        }
    }

    pub fn dependencies(&mut self) -> Option<&mut Vec<PathBuf>> {
        match &mut self.kind {
            EntryKind::Dir { dependencies, .. } => Some(dependencies),
            _ => None,
        }
    }
}

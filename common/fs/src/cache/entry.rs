use std::cell::RefCell;
use std::ffi::OsString;
use std::path::PathBuf;

use inotify::WatchDescriptor;

use crate::cache::TailedFile;
use crate::cache::{Children, EntryKey};
use crate::rule::Rules;

use crate::cache::tailed_file::LazyLineSerializer;

#[derive(Debug)]
pub enum Entry {
    File {
        name: OsString,
        parent: EntryKey,
        wd: WatchDescriptor,
        data: RefCell<TailedFile<LazyLineSerializer>>,
    },
    Dir {
        name: OsString,
        parent: Option<EntryKey>,
        children: Children,
        wd: WatchDescriptor,
    },
    Symlink {
        name: OsString,
        parent: EntryKey,
        link: PathBuf,
        wd: WatchDescriptor,
        rules: Rules,
    },
}

impl Entry {
    pub fn name(&self) -> &OsString {
        match self {
            Entry::File { name, .. } | Entry::Dir { name, .. } | Entry::Symlink { name, .. } => {
                name
            }
        }
    }

    pub fn parent(&self) -> Option<EntryKey> {
        match self {
            Entry::File { parent, .. } | Entry::Symlink { parent, .. } => Some(*parent),
            Entry::Dir { parent, .. } => *parent,
        }
    }

    pub fn set_parent(&mut self, new_parent: EntryKey) {
        match self {
            Entry::File { parent, .. } | Entry::Symlink { parent, .. } => *parent = new_parent,
            Entry::Dir { parent, .. } => *parent = Some(new_parent),
        }
    }

    pub fn set_name(&mut self, new_name: OsString) {
        match self {
            Entry::File { name, .. } | Entry::Dir { name, .. } | Entry::Symlink { name, .. } => {
                *name = new_name
            }
        }
    }

    pub fn link(&self) -> Option<&PathBuf> {
        match self {
            Entry::Symlink { link, .. } => Some(link),
            _ => None,
        }
    }

    pub fn children(&self) -> Option<&Children> {
        match self {
            Entry::Dir { children, .. } => Some(children),
            _ => None,
        }
    }

    pub fn children_mut(&mut self) -> Option<&mut Children> {
        match self {
            Entry::Dir { children, .. } => Some(children),
            _ => None,
        }
    }

    pub fn watch_descriptor(&self) -> &WatchDescriptor {
        match self {
            Entry::Dir { wd, .. } | Entry::Symlink { wd, .. } | Entry::File { wd, .. } => wd,
        }
    }
}

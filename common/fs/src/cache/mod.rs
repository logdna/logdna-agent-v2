use crate::cache::entry::Entry;
use crate::cache::event::Event;
use crate::cache::tailed_file::TailedFile;
use crate::lookback::Lookback;
use crate::rule::{RuleDef, Rules, Status};
use notify_stream::{Event as WatchEvent, RecursiveMode, Watcher};

use state::{FileId, Span, SpanVec};

use std::cell::RefCell;
use std::collections::hash_map::Entry as HashMapEntry;
use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::fs::read_dir;
use std::iter::FromIterator;
use std::ops::Deref;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::{fmt, fs, io};

use futures::{Stream, StreamExt};
use slotmap::{DefaultKey, SlotMap};
use smallvec::SmallVec;
use thiserror::Error;
use time::OffsetDateTime;
use tokio::sync::Mutex;

pub mod dir_path;
pub mod entry;
pub mod event;
pub mod tailed_file;
pub use dir_path::{DirPathBuf, DirPathBufError};
use metrics::Metrics;
use std::time::Duration;

type Children = HashMap<OsString, EntryKey>;
type Symlinks = HashMap<PathBuf, Vec<EntryKey>>;
type WatchDescriptors = HashMap<PathBuf, Vec<EntryKey>>;

pub type EntryKey = DefaultKey;

type EntryMap = SlotMap<EntryKey, entry::Entry>;
type FsResult<T> = Result<T, Error>;

pub const EVENT_STREAM_BUFFER_COUNT: usize = 1000;

#[derive(Debug, Error)]
pub enum Error {
    #[error("error watching: {0:?} {1:?}")]
    Watch(PathBuf, notify_stream::Error),
    #[error("got event for untracked watch descriptor: {0:?}")]
    WatchEvent(PathBuf),
    #[error("unexpected existing entry")]
    Existing,
    #[error("failed to find entry")]
    Lookup,
    #[error("failed to find parent entry")]
    ParentLookup,
    #[error("parent should be a directory")]
    ParentNotValid,
    #[error("path is not valid: {0:?}")]
    PathNotValid(PathBuf),
    #[error("The process lacks permissions to view directory contents")]
    DirectoryListNotValid(io::Error, PathBuf),
    #[error("encountered errors when inserting recursively: {0:?}")]
    InsertRecursively(Vec<Error>),
    #[error("error reading file: {0:?}")]
    File(io::Error),
}

type EventTimestamp = time::OffsetDateTime;

/// Turns an inotify event into an event stream
fn as_event_stream(
    fs: Arc<Mutex<FileSystem>>,
    event_result: WatchEvent,
    event_time: EventTimestamp,
) -> impl Stream<Item = (Result<Event, Error>, EventTimestamp)> {
    let a = match fs
        .try_lock()
        .expect("couldn't lock filesystem cache")
        .process(event_result)
    {
        Ok(events) => events
            .into_iter()
            .map(Ok)
            .collect::<SmallVec<[Result<Event, Error>; 2]>>(),
        Err(e) => {
            let mut result: SmallVec<[Result<Event, Error>; 2]> = SmallVec::new();
            result.push(Err(e));
            result
        }
    };

    let a = a
        .into_iter()
        .map(move |event_result| (event_result, event_time));

    futures::stream::iter(a)
}

fn get_initial_events(
    fs: &Arc<Mutex<FileSystem>>,
) -> impl Stream<Item = (Result<Event, Error>, EventTimestamp)> {
    let init_time = OffsetDateTime::now_utc();
    let initial_events = {
        let mut fs = fs.try_lock().expect("could not lock filesystem cache");

        let mut acc = Vec::new();
        if !fs.initial_events.is_empty() {
            for event in std::mem::take(&mut fs.initial_events) {
                acc.push((Ok(event), init_time))
            }
        }
        acc
    };

    futures::stream::iter(initial_events)
}

fn get_resume_events(
    fs: &Arc<Mutex<FileSystem>>,
) -> impl Stream<Item = (WatchEvent, EventTimestamp)> {
    let _fs = fs.try_lock().expect("couldn't lock filesystem cache");
    _fs.resume_events_recv
        .clone()
        .map({
            let fs = fs.clone();
            move |(inode, event_time)| {
                fs.try_lock()
                    .expect("couldn't lock filesystem cache")
                    .wd_by_inode
                    .get(&inode)
                    .map(|wd| (WatchEvent::Write(wd.clone()), event_time))
            }
        })
        .filter_map(|e| async { e })
}

pub struct FileSystem {
    watcher: Watcher,
    pub entries: Rc<RefCell<EntryMap>>,
    symlinks: Symlinks,
    watch_descriptors: WatchDescriptors,
    wd_by_inode: HashMap<u64, PathBuf>,
    master_rules: Rules,
    initial_dirs: Vec<DirPathBuf>,
    initial_dir_rules: Rules,

    initial_events: Vec<Event>,

    lookback_config: Lookback,
    initial_offsets: HashMap<FileId, SpanVec>,

    resume_events_recv: async_channel::Receiver<(u64, EventTimestamp)>,
    resume_events_send: async_channel::Sender<(u64, EventTimestamp)>,
}

#[cfg(unix)]
fn add_initial_dir_rules(rules: &mut Rules, path: &DirPathBuf) {
    // Include one for self and the rest of its children
    rules.add_inclusion(
        RuleDef::glob_rule(path.join(r"**").to_str().expect("invalid unicode in path"))
            .expect("invalid glob rule format"),
    );
    rules.add_inclusion(
        RuleDef::glob_rule(path.to_str().expect("invalid unicode in path"))
            .expect("invalid glob rule format"),
    );
}

#[cfg(windows)]
fn add_initial_dir_rules(rules: &mut Rules, path: &DirPathBuf) {
    // Include one for self and the rest of its children
    rules.add_inclusion(
        RuleDef::glob_rule(format!("{}*", path.to_str().expect("invalid unicode in path")).as_str())
            .expect("invalid glob rule format"),
    );
}

impl FileSystem {
    pub fn new(
        initial_dirs: Vec<DirPathBuf>,
        lookback_config: Lookback,
        initial_offsets: HashMap<FileId, SpanVec>,
        rules: Rules,
        delay: Duration,
    ) -> Self {
        let (resume_events_send, resume_events_recv) = async_channel::unbounded();
        initial_dirs.iter().for_each(|path| {
            if !path.is_dir() {
                panic!("initial dirs must be dirs")
            }
        });
        let watcher = Watcher::new(delay);
        let entries = SlotMap::new();

        let mut initial_dir_rules = Rules::new();

        for path in initial_dirs.iter() {
            add_initial_dir_rules(&mut initial_dir_rules, path);
        }

        let mut fs = Self {
            entries: Rc::new(RefCell::new(entries)),
            symlinks: Symlinks::new(),
            watch_descriptors: WatchDescriptors::new(),
            wd_by_inode: HashMap::new(),
            master_rules: rules,
            initial_dirs: initial_dirs.clone(),
            initial_dir_rules,
            lookback_config,
            initial_offsets,
            watcher,
            initial_events: Vec::new(),
            resume_events_recv,
            resume_events_send,
        };

        let entries = fs.entries.clone();
        let mut entries = entries.borrow_mut();

        let mut initial_dirs_events = Vec::new();
        for dir in initial_dirs
            .into_iter()
            .map(|path| -> PathBuf { path.into() })
        {
            let mut path_cpy: PathBuf = dir.clone();
            loop {
                if !path_cpy.exists() {
                    path_cpy.pop();
                } else {
                    break;
                }
            }
            if let Err(e) = fs.insert(&path_cpy, &mut initial_dirs_events, &mut entries) {
                // It can failed due to permissions or some other restriction
                debug!(
                    "Initial insertion of {} failed: {}",
                    path_cpy.to_str().unwrap(),
                    e
                );
            }
        }

        for event in initial_dirs_events {
            match event {
                Event::New(entry_key) => {
                    fs.initial_events.push(Event::Initialize(entry_key));
                }
                _ => panic!("unexpected event in initialization"),
            };
        }

        fs
    }

    pub fn stream_events(
        fs: Arc<Mutex<FileSystem>>,
    ) -> Result<impl Stream<Item = (Result<Event, Error>, EventTimestamp)>, std::io::Error> {
        let events_stream = {
            let watcher = &fs
                .try_lock()
                .expect("could not lock filesystem cache")
                .watcher;
            watcher.receive()
        };

        let initial_events = get_initial_events(&fs);
        let resume_events_recv = get_resume_events(&fs);
        let events = futures::stream::select(resume_events_recv, events_stream)
            .map(|event_result| async { event_result })
            .buffered(EVENT_STREAM_BUFFER_COUNT)
            .map(move |(event, event_time)| {
                let fs = fs.clone();
                as_event_stream(fs, event, event_time)
            })
            .flatten();

        Ok(initial_events.chain(events))
    }

    /// Handles inotify events and may produce Event(s) that are returned upstream through sender
    fn process(&mut self, watch_event: WatchEvent) -> FsResult<Vec<Event>> {
        let _entries = self.entries.clone();
        let mut _entries = _entries.borrow_mut();

        debug!("handling notify event {:#?}", watch_event);

        // TODO: Remove OsString names
        let result = match watch_event {
            WatchEvent::Create(wd) => self.process_create(&wd, &mut _entries),
            //TODO: Handle Write event for directories
            WatchEvent::Write(wd) => self.process_modify(&wd),
            WatchEvent::Remove(wd) => self.process_delete(&wd, &mut _entries),
            WatchEvent::Rename(from_wd, to_wd) => {
                // Source path should exist and be tracked to be a move
                let is_from_path_ok = self
                    .get_first_entry(&from_wd)
                    .map(|entry| self.entry_path_passes(entry, &_entries))
                    .unwrap_or(false);

                // Target path pass the inclusion/exclusion rules to be a move
                let is_to_path_ok = self.passes(&to_wd, &_entries);

                if is_to_path_ok && is_from_path_ok {
                    self.process_rename(&from_wd, &to_wd, &mut _entries)
                } else if is_to_path_ok {
                    self.process_create(&to_wd, &mut _entries)
                } else if is_from_path_ok {
                    self.process_delete(&from_wd, &mut _entries)
                } else {
                    // Most likely parent was removed, dropping all child watch descriptors
                    // and we've got the child watch event already queued up
                    debug!("Move event received from targets that are not watched anymore");
                    Ok(Vec::new())
                }
            }
            WatchEvent::Error(e, p) => {
                debug!(
                    "There was an error mapping a file change: {:?} ({:?})",
                    e, p
                );
                Ok(Vec::new())
            }
            _ => {
                // TODO: Map the rest of the events explicitly
                Ok(Vec::new())
            }
        };

        if let Err(e) = result {
            match e {
                Error::PathNotValid(path) => {
                    debug!("Path is not longer valid: {}", path.display());
                }
                Error::WatchEvent(path) => {
                    debug!("Processing event for untracked path: {}", path.display());
                }
                _ => {
                    warn!("Processing notify event resulted in error: {}", e);
                }
            }
            Ok(Vec::new())
        } else {
            result
        }
    }

    fn process_create(
        &mut self,
        watch_descriptor: &Path,
        _entries: &mut EntryMap,
    ) -> FsResult<Vec<Event>> {
        let path = &watch_descriptor;

        let mut events = Vec::new();
        //TODO: Check duplicates
        self.insert(&path, &mut events, _entries)
            .map(move |_| events)
    }

    fn process_modify(&mut self, watch_descriptor: &Path) -> FsResult<Vec<Event>> {
        let mut entry_ptrs_opt = None;
        if let Some(entries) = self.watch_descriptors.get_mut(watch_descriptor) {
            entry_ptrs_opt = Some(entries.clone())
        }

        // TODO: If symlink => revisit target
        if let Some(mut entry_ptrs) = entry_ptrs_opt {
            let mut events = Vec::new();
            for entry_ptr in entry_ptrs.iter_mut() {
                events.push(Event::Write(*entry_ptr));
            }

            Ok(events)
        } else {
            Err(Error::WatchEvent(watch_descriptor.to_owned()))
        }
    }

    fn process_delete(
        &mut self,
        watch_descriptor: &Path,
        _entries: &mut EntryMap,
    ) -> FsResult<Vec<Event>> {
        if let Ok(entry_key) = self.get_first_entry(watch_descriptor) {
            let entry = _entries.get(entry_key).ok_or(Error::Lookup)?;
            let path = entry.path().to_path_buf();
            if !self.initial_dirs.iter().any(|dir| dir.as_ref() == path) {
                let mut events = Vec::new();
                return self
                    .remove(&path, &mut events, _entries)
                    .map(move |_| events);
            }
        } else {
            // The file is already gone (event was queued up), safely ignore
            trace!(
                "processing delete for {:?} was ignored as entry was not found",
                watch_descriptor
            );
        }

        Ok(Vec::new())
    }

    fn get_initial_offset(&self, path: &Path, inode: FileId) -> SpanVec {
        fn _lookup_offset(
            initial_offsets: &HashMap<FileId, SpanVec>,
            key: &FileId,
            path: &Path,
        ) -> Option<SpanVec> {
            if let Some(offset) = initial_offsets.get(key) {
                debug!("Got offset {:?} from state using key {:?}", offset, path);
                Some(offset.clone())
            } else {
                None
            }
        }
        match self.lookback_config {
            Lookback::Start => {
                _lookup_offset(&self.initial_offsets, &inode, path).unwrap_or_default()
            }
            Lookback::SmallFiles => {
                // Check the actual file len
                let file_len = path.metadata().map(|m| m.len()).unwrap_or(0);
                let smallfiles_offset = if file_len < 8192 {
                    SpanVec::new()
                } else {
                    [Span::new(0, file_len).unwrap()].iter().collect()
                };
                _lookup_offset(&self.initial_offsets, &inode, path).unwrap_or(smallfiles_offset)
            }
            Lookback::None => path
                .metadata()
                .map(|m| [Span::new(0, m.len()).unwrap()].iter().collect())
                .unwrap_or_default(),
        }
    }

    /// Inserts a new entry when the path validates the inclusion/exclusion rules.
    ///
    /// Returns `Ok(Some(entry))` pointing to the newly created entry.
    ///
    /// When the path doesn't pass the rules or the path is invalid, it returns `Ok(None)`.
    /// When the file watcher can't be added or the parent dir can not be created, it
    /// returns an `Err`.
    fn insert(
        &mut self,
        path: &Path,
        events: &mut Vec<Event>,
        _entries: &mut EntryMap,
    ) -> FsResult<Option<EntryKey>> {
        if !self.passes(path, _entries) {
            info!("ignoring {:?}", path);
            return Ok(None);
        }

        let link_path = path.read_link();
        if !path.exists() && !link_path.is_ok() {
            warn!("attempted to insert non existent path {:?}", path);
            return Ok(None);
        }

        // Check if it exists already
        if let Some(entry_keys) = self.watch_descriptors.get(path) {
            debug!("watch descriptor for {} already exists", path.display());
            return Ok(Some(entry_keys[0]));
        }

        let is_dir = match fs::metadata(path) {
            Ok(m) => m.is_dir(),
            Err(_) => {
                if !link_path.is_ok() {
                    return Err(Error::PathNotValid(path.into()));
                }
                // Dangling symlinks have no accessible metadata
                false
            }
        };

        if is_dir {
            // Watch recursively
            let contents =
                fs::read_dir(path).map_err(|e| Error::DirectoryListNotValid(e, path.into()))?;
            // Insert the parent directory first
            trace!("inserting directory {}", path.display());

            // We don't differentiate between directory and symlink to a directory
            let new_entry = Entry::Dir {
                children: Default::default(),
                path: path.into(),
            };

            self.watcher
                .watch(&path, RecursiveMode::NonRecursive)
                .map_err(|e| Error::Watch(path.to_path_buf(), e))?;
            let new_key = self.register_as_child(new_entry, _entries)?;
            events.push(Event::New(new_key));

            for dir_entry in contents {
                if dir_entry.is_err() {
                    continue;
                }
                let dir_entry = dir_entry.unwrap();
                if let Err(e) = self.insert(&dir_entry.path(), events, _entries) {
                    info!(
                        "error found when inserting child entry for {:?}: {}",
                        path, e
                    );
                }
            }
            return Ok(Some(new_key));
        }

        let mut symlink_target = None;
        let new_entry = match link_path {
            Ok(target) => {
                trace!(
                    "inserting symlink {} with target {}",
                    path.display(),
                    target.display()
                );

                symlink_target = Some(target.clone());

                Entry::Symlink {
                    link: target,
                    path: path.into(),
                }
            }
            _ => {
                trace!("inserting file {}", path.display());
                let inode = path.metadata().map_err(Error::File)?.ino();
                self.wd_by_inode.insert(inode, path.into());

                let offsets = self.get_initial_offset(path, inode.into());
                let initial_offset = offsets.first().map(|offset| offset.end).unwrap_or(0);

                Metrics::fs().increment_tracked_files();
                Entry::File {
                    path: path.into(),
                    data: RefCell::new(
                        TailedFile::new(path, offsets, Some(self.resume_events_send.clone()))
                            .map_err(Error::File)?,
                    ),
                }
            }
        };

        self.watcher
            .watch(&path, RecursiveMode::NonRecursive)
            .map_err(|e| Error::Watch(path.to_path_buf(), e))?;
        // TODO: Maybe change method abstractions
        let new_key = self.register_as_child(new_entry, _entries)?;
        events.push(Event::New(new_key));

        // Insert the link target
        if let Some(target) = symlink_target {
            match self.insert(&target, events, _entries) {
                Err(e) => {
                    // The insert of the target failed, the changes to the symlink itself
                    // are going to be tracked, continue
                    warn!(
                        "insert target {} of symlink {} resulted in error {}",
                        target.display(),
                        path.display(),
                        e
                    );
                }
                Ok(None) => {
                    // The insert of the target failed, the changes to the symlink itself
                    // are going to be tracked, continue
                    error!(
                        "insert target {} of symlink {} could not be added",
                        target.display(),
                        path.display()
                    );
                }
                _ => {}
            }
        }

        Ok(Some(new_key))
    }

    fn register(&mut self, entry_key: EntryKey, _entries: &mut EntryMap) -> FsResult<()> {
        let entry = _entries.get(entry_key).ok_or(Error::Lookup)?;
        let path = entry.path();

        self.watch_descriptors
            .entry(path.to_path_buf())
            .or_insert_with(Vec::new)
            .push(entry_key);

        if let Entry::Symlink { link, .. } = entry.deref() {
            self.symlinks
                .entry(link.clone())
                .or_insert_with(Vec::new)
                .push(entry_key);
        }

        info!("watching {:?}", path);
        Ok(())
    }

    /// Removes the entry reference from watch_descriptors and symlinks
    fn unregister(&mut self, entry_key: EntryKey, _entries: &mut EntryMap) {
        let entry = match _entries.get(entry_key) {
            Some(v) => v,
            None => {
                error!("failed to find entry to unregister");
                return;
            }
        };

        let path = entry.path().to_path_buf();
        let entries = match self.watch_descriptors.get_mut(&path) {
            Some(v) => v,
            None => {
                error!("attempted to remove untracked watch descriptor {:?}", path);
                return;
            }
        };

        entries.retain(|other| *other != entry_key);
        if entries.is_empty() {
            self.watch_descriptors.remove(&path);
            if let Err(e) = self.watcher.unwatch_if_exists(&path) {
                // Log and continue
                debug!(
                    "unwatching {:?} resulted in an error, likely due to a dangling symlink {:?}",
                    path, e
                );
            }
        }

        if let Entry::Symlink { link, .. } = entry.deref() {
            let entries = match self.symlinks.get_mut(link) {
                Some(v) => v,
                None => {
                    error!("attempted to remove untracked symlink {:?}", path);
                    return;
                }
            };

            entries.retain(|other| *other != entry_key);
            if entries.is_empty() {
                self.symlinks.remove(link);
            }
        }

        info!("unwatching {:?}", path);
    }

    fn remove(
        &mut self,
        path: &Path,
        events: &mut Vec<Event>,
        _entries: &mut EntryMap,
    ) -> FsResult<()> {
        let entry_key = self.lookup(path, _entries).ok_or(Error::Lookup)?;
        let parent = path.parent().map(|p| self.lookup(p, _entries)).flatten();

        if let Some(parent) = parent {
            let name = path
                .file_name()
                .ok_or_else(|| Error::PathNotValid(path.to_path_buf()))?;
            match _entries.get_mut(parent) {
                None => {}
                Some(parent_entry) => {
                    parent_entry
                        .children()
                        .ok_or(Error::ParentNotValid)?
                        .remove(&name.to_owned());
                }
            }
        }

        self.drop_entry(entry_key, events, _entries);

        Ok(())
    }

    /// Emits `Delete` events, removes the entry and its children from
    /// watch descriptors and symlinks.
    fn drop_entry(
        &mut self,
        entry_key: EntryKey,
        events: &mut Vec<Event>,
        _entries: &mut EntryMap,
    ) {
        self.unregister(entry_key, _entries);
        if let Some(entry) = _entries.get(entry_key) {
            let mut _children = vec![];
            let mut _links = vec![];
            match entry.deref() {
                Entry::Dir { children, .. } => {
                    for child in children.values() {
                        _children.push(*child);
                    }
                }
                Entry::Symlink { ref link, .. } => {
                    // This is a hacky way to check if there are any remaining
                    // symlinks pointing to `link`
                    if !self.passes(link, _entries) {
                        _links.push(link.clone())
                    }

                    events.push(Event::Delete(entry_key));
                }
                Entry::File { .. } => {
                    Metrics::fs().decrement_tracked_files();
                    events.push(Event::Delete(entry_key));
                }
            }

            for child in _children {
                self.drop_entry(child, events, _entries);
            }

            for link in _links {
                // Ignore error
                self.remove(&link, events, _entries).unwrap_or_default();
            }
        }
    }

    // `from` is the path from where the file or dir used to live
    // `to is the path to where the file or dir now lives
    // e.g from = /var/log/syslog and to = /var/log/syslog.1.log
    fn process_rename(
        &mut self,
        from_path: &Path,
        to_path: &Path,
        _entries: &mut EntryMap,
    ) -> FsResult<Vec<Event>> {
        let new_parent = to_path.parent().map(|p| self.lookup(p, _entries)).flatten();

        let mut events = Vec::new();
        match self.lookup(from_path, _entries) {
            Some(entry_key) => {
                let entry = _entries.get_mut(entry_key).ok_or(Error::Lookup)?;
                let new_name = to_path
                    .file_name()
                    .ok_or_else(|| Error::PathNotValid(to_path.into()))?
                    .to_owned();

                let old_name = entry
                    .path()
                    .file_name()
                    .ok_or_else(|| Error::PathNotValid(entry.path().into()))?
                    .to_owned();

                // Try to remove from parent
                if let Some(parent) = from_path
                    .parent()
                    .map(|p| self.lookup(p, _entries))
                    .flatten()
                {
                    _entries
                        .get_mut(parent)
                        .ok_or(Error::ParentLookup)?
                        .children()
                        .ok_or(Error::ParentNotValid)?
                        .remove(&old_name);
                }

                let entry = _entries.get_mut(entry_key).ok_or(Error::Lookup)?;
                entry.set_path(to_path.to_path_buf());

                // Remove previous reference and add new one
                self.watch_descriptors.remove(to_path);
                self.watch_descriptors
                    .entry(to_path.to_path_buf())
                    .or_insert_with(Vec::new)
                    .push(entry_key);

                if let Some(new_parent) = new_parent {
                    _entries
                        .get_mut(new_parent)
                        .ok_or(Error::ParentLookup)?
                        .children()
                        .ok_or(Error::ParentNotValid)?
                        .insert(new_name, entry_key);
                }
            }
            None => {
                self.insert(to_path, &mut events, _entries)?;
            }
        }
        Ok(events)
    }

    /// Inserts the entry, registers it, looks up for the parent and set itself as a children.
    fn register_as_child(
        &mut self,
        new_entry: Entry,
        entries: &mut EntryMap,
    ) -> FsResult<EntryKey> {
        let component = new_entry
            .path()
            .file_name()
            .ok_or_else(|| Error::PathNotValid(new_entry.path().into()))?
            .to_os_string();
        let parent_path = new_entry.path().parent().map(|p| p.to_path_buf());
        let new_key = entries.insert(new_entry);
        self.register(new_key, entries)?;

        // Try to find parent
        if let Some(parent_path) = parent_path {
            if let Some(parent_key) = self.watch_descriptors.get(&parent_path) {
                let parent_key = parent_key.get(0).copied().ok_or(Error::ParentLookup)?;
                return match entries
                    .get_mut(parent_key)
                    .ok_or(Error::ParentLookup)?
                    .children()
                    .ok_or(Error::ParentNotValid)?
                    .entry(component)
                {
                    HashMapEntry::Vacant(v) => Ok(*v.insert(new_key)),
                    // TODO: Maybe consider to silently replace
                    _ => Err(Error::Existing),
                };
            } else {
                trace!("parent with path {:?} not found", parent_path);
            }
        }

        // Parent was not found but it's actively tracked
        Ok(new_key)
    }

    /// Returns the entry that represents the supplied path.
    /// When the path is not represented and therefore has no entry then `None` is return.
    pub fn lookup(&self, path: &Path, _entries: &EntryMap) -> Option<EntryKey> {
        self.watch_descriptors.get(path).map(|entries| entries[0])
    }

    fn is_symlink_target(&self, path: &Path, _entries: &EntryMap) -> bool {
        for (_, symlink_ptrs) in self.symlinks.iter() {
            for symlink_ptr in symlink_ptrs.iter() {
                if let Some(symlink) = _entries.get(*symlink_ptr) {
                    match symlink {
                        Entry::Symlink { link, .. } => {
                            if link == path {
                                return true;
                            }
                        }
                        _ => {
                            panic!(
                                "did not expect non symlink entry in symlinks master map for path {:?}",
                                path
                            );
                        }
                    }
                } else {
                    error!("failed to find entry from symlink targets");
                };
            }
        }
        false
    }

    /// Determines whether the path is within the initial dir
    /// and either passes the master rules (e.g. "*.log") or it's a directory
    pub(crate) fn is_initial_dir_target(&self, path: &Path) -> bool {
        // Must be within the initial dir
        if self.initial_dir_rules.passes(path) != Status::Ok {
            return false;
        }

        // The file should validate the file rules or be a directory
        if self.master_rules.passes(path) != Status::Ok {
            if let Ok(metadata) = std::fs::metadata(path) {
                return metadata.is_dir();
            }
            return false;
        }

        true
    }

    /// Helper method for checking if a path passes exclusion/inclusion rules
    fn passes(&self, path: &Path, _entries: &EntryMap) -> bool {
        self.is_initial_dir_target(path) || self.is_symlink_target(path, _entries)
    }

    fn entry_path_passes(&self, entry_key: EntryKey, entries: &EntryMap) -> bool {
        entries
            .get(entry_key)
            .map(|e| self.passes(e.path(), &entries))
            .unwrap_or(false)
    }

    /// Returns the first entry based on the `WatchDescriptor`, returning an `Err` when not found.
    fn get_first_entry(&self, wd: &Path) -> FsResult<EntryKey> {
        let entries = self
            .watch_descriptors
            .get(wd)
            .ok_or_else(|| Error::WatchEvent(wd.to_owned()))?;

        if !entries.is_empty() {
            Ok(entries[0])
        } else {
            Err(Error::WatchEvent(wd.to_owned()))
        }
    }
}

// conditionally implement std::fmt::Debug if the underlying type T implements it
impl fmt::Debug for FileSystem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut builder = f.debug_struct("FileSystem");
        builder.field("symlinks", &&self.symlinks);
        builder.field("watch_descriptors", &&self.watch_descriptors);
        builder.field("master_rules", &&self.master_rules);
        builder.field("initial_dir_rules", &&self.initial_dir_rules);
        builder.field("initial_events", &&self.initial_events);
        builder.finish()
    }
}

// recursively scans a directory for unlimited depth
fn recursive_scan(path: &Path) -> Vec<PathBuf> {
    if !path.is_dir() {
        return vec![];
    }

    let mut paths = vec![path.to_path_buf()];

    // read all files/dirs in path at depth 1
    let tmp_paths = match read_dir(&path) {
        Ok(v) => v,
        Err(e) => {
            error!("failed accessing {:?}: {:?}", path, e);
            return paths;
        }
    };
    // iterate over all the paths and call recursive_scan on all dirs
    for tmp_path in tmp_paths {
        let path = match tmp_path {
            Ok(path) => path.path(),
            Err(e) => {
                error!("failed scanning directory {:?}: {:?}", path, e);
                continue;
            }
        };
        // if the path is a dir then recursively scan it also
        // so that we have an unlimited depth scan
        if path.is_dir() {
            paths.append(&mut recursive_scan(&path))
        } else {
            paths.push(path)
        }
    }

    paths
}

// Split the path into it's components.
fn into_components(path: &Path) -> Vec<OsString> {
    path.components()
        .filter_map(|c| match c {
            Component::RootDir => Some("/".into()),
            Component::Normal(path) => Some(path.into()),
            _ => None,
        })
        .collect()
}

// Build a rule for all sub paths for a path e.g. /var/log/containers => include [/, /var, /var/log, /var/log/containers]
fn into_rules(path: PathBuf) -> Rules {
    let mut rules = Rules::new();
    append_rules(&mut rules, path);
    rules
}

// Attach rules for all sub paths for a path
fn append_rules(rules: &mut Rules, mut path: PathBuf) {
    rules.add_inclusion(
        RuleDef::glob_rule(path.join(r"**").to_str().expect("invalid unicode in path"))
            .expect("invalid glob rule format"),
    );

    loop {
        rules.add_inclusion(
            RuleDef::glob_rule(path.to_str().expect("invalid unicode in path"))
                .expect("invalid glob rule format"),
        );
        if !path.pop() {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rule::{RuleDef, Rules};
    use crate::test::LOGGER;
    use pin_utils::pin_mut;
    use std::convert::TryInto;
    use std::fs::{copy, create_dir, hard_link, remove_dir_all, remove_file, rename, File};
    use std::{io, panic};
    use std::io::Write;
    use tempfile::{tempdir, TempDir};

    static DELAY: Duration = Duration::from_millis(200);

    macro_rules! take_events {
        ($x: expr) => {
            tokio::time::sleep(DELAY * 2).await;
            tokio::time::sleep(Duration::from_millis(20)).await;
            let stream = FileSystem::stream_events($x.clone()).unwrap();
            pin_mut!(stream);
            loop {
                tokio::select! {
                    _ = stream.next() => {}
                    _ = tokio::time::sleep(Duration::from_millis(100)) => {
                        break;
                    }
                }
            }
        };
    }

    macro_rules! lookup {
        ( $x:expr, $y: expr ) => {{
            let fs = $x.lock().await;
            let entry_keys = fs.watch_descriptors.get(&$y);
            if entry_keys.is_none() {
                None
            } else {
                Some(entry_keys.unwrap()[0])
            }
        }};
    }

    macro_rules! lookup_entry {
        ( $x:expr, $y: expr ) => {{
            let fs = loop {
                if let Ok(fs) = $x.try_lock() {
                    break fs;
                }
            };
            let entries = fs.entries.clone();
            let entries = entries.borrow();
            fs.lookup(&$y, &entries)
        }};
    }

    macro_rules! assert_is_file {
        ( $x:expr, $y: expr ) => {
            assert!($y.is_some());
            {
                let fs = $x.lock().await;
                let entries = fs.entries.borrow();
                assert!(matches!(
                    entries.get($y.unwrap()).unwrap().deref(),
                    Entry::File { .. }
                ));
            }
        };
    }

    #[cfg(target_os = "windows")]
    fn symlink_file<P: AsRef<Path>, Q: AsRef<Path>>(original: P, link: Q) -> io::Result<()> {
        std::os::windows::fs::symlink_file(original, link)
    }

    #[cfg(target_os = "windows")]
    fn symlink_dir<P: AsRef<Path>, Q: AsRef<Path>>(original: P, link: Q) -> io::Result<()> {
        std::os::windows::fs::symlink_dir(original, link)
    }

    #[cfg(unix)]
    fn symlink_file<P: AsRef<Path>, Q: AsRef<Path>>(original: P, link: Q) -> io::Result<()> {
        std::os::unix::fs::symlink(original, link)
    }

    #[cfg(unix)]
    fn symlink_dir<P: AsRef<Path>, Q: AsRef<Path>>(original: P, link: Q) -> io::Result<()> {
        // No difference in unix between a symlink of a file and a dir
        symlink_file(original, link)
    }

    fn new_fs<T: Default + Clone + std::fmt::Debug>(
        path: PathBuf,
        rules: Option<Rules>,
    ) -> FileSystem {
        let rules = rules.unwrap_or_else(|| {
            let mut rules = Rules::new();
            rules.add_inclusion(RuleDef::glob_rule(r"**").unwrap());
            rules
        });
        FileSystem::new(
            vec![path
                .as_path()
                .try_into()
                .unwrap_or_else(|_| panic!("{:?} is not a directory!", path))],
            Lookback::Start,
            HashMap::new(),
            rules,
            DELAY,
        )
    }

    fn create_fs(path: &Path) -> Arc<Mutex<FileSystem>> {
        Arc::new(Mutex::new(new_fs::<()>(path.to_path_buf(), None)))
    }

    #[tokio::test]
    async fn filesystem_init_test() {
        let temp_dir = tempdir().unwrap();
        let dir = temp_dir.path();

        let file_path = dir.join("a.log");
        File::create(&file_path).unwrap();

        let fs = create_fs(dir);

        take_events!(fs);

        let entry_key = lookup!(fs, file_path);
        assert_is_file!(fs, entry_key);
    }

    // Simulates the `create_move` log rotation strategy
    #[tokio::test]
    async fn filesystem_rotate_create_move() {
        let temp_dir = tempdir().unwrap();
        let path = temp_dir.path();

        let fs = create_fs(path);

        tokio::time::sleep(Duration::from_millis(200)).await;
        let a = path.join("a");
        File::create(&a).unwrap();

        take_events!(fs);
        let entry_key = lookup!(fs, a);
        assert_is_file!(fs, entry_key);

        let new = path.join("a.new");
        rename(&a, &new).unwrap();

        take_events!(fs);

        // Previous name should not have an associated entry
        let entry_key = lookup!(fs, a);
        assert!(entry_key.is_none());

        let entry_key = lookup!(fs, new);
        assert_is_file!(fs, entry_key);

        // Create a new file in place
        File::create(&a).unwrap();

        take_events!(fs);
        let entry_key = lookup!(fs, a);
        assert_is_file!(fs, entry_key);
    }

    // Simulates the `create_copy` log rotation strategy
    #[tokio::test]
    async fn filesystem_rotate_create_copy() -> io::Result<()> {
        let tempdir = TempDir::new()?;
        let path = tempdir.path().to_path_buf();
        let fs = create_fs(&path);

        let a = path.join("a");
        File::create(&a)?;
        take_events!(fs);
        let entry_key = lookup!(fs, a);
        assert_is_file!(fs, entry_key);

        // Copy and remove
        let old = path.join("a.old");
        copy(&a, &old)?;
        take_events!(fs);

        remove_file(&a).unwrap();

        take_events!(fs);
        let entry_key = lookup!(fs, a);
        assert!(entry_key.is_none());
        let entry_key = lookup!(fs, old);
        assert_is_file!(fs, entry_key);

        // Recreating files in Windows in the same location is tricky subject
        // It should usually be done with retries
        #[cfg(unix)]
        {
            // Recreate original file back
            File::create(&a).unwrap();
            take_events!(fs);
            let entry_key = lookup!(fs, a);
            assert_is_file!(fs, entry_key);
        }

        Ok(())
    }

    // Creates a plain old dir
    #[tokio::test]
    async fn filesystem_create_dir() {
        let tempdir = TempDir::new().unwrap();
        let path = tempdir.path().to_path_buf();

        let fs = create_fs(&path);
        take_events!(fs);
        let entry_key = lookup!(fs, path);
        assert!(entry_key.is_some());

        let fs = fs.lock().await;
        let entries = fs.entries.borrow();
        assert!(matches!(
            entries.get(entry_key.unwrap()).unwrap().deref(),
            Entry::Dir { .. }
        ));
    }

    /// Creates a dir w/ dots and a file after initialization
    #[tokio::test]
    async fn filesystem_create_dir_after_init() -> io::Result<()> {
        let tempdir = TempDir::new()?;
        let path = tempdir.path().to_path_buf();

        let fs = create_fs(&path);
        take_events!(fs);

        // Use a subdirectory with dots
        let sub_dir = path.join("sub.dir");
        create_dir(&sub_dir)?;
        let file_path = sub_dir.join("insert.log");
        File::create(&file_path)?;

        take_events!(fs);
        let entry_key = lookup!(fs, file_path);
        assert_is_file!(fs, entry_key);
        Ok(())
    }

    // Creates a plain old file
    #[tokio::test]
    async fn filesystem_create_file() -> io::Result<()> {
        let tempdir = TempDir::new()?;
        let path = tempdir.path().to_path_buf();

        let fs = create_fs(&path);
        let file_path = path.join("insert.log");
        File::create(&file_path)?;
        take_events!(fs);

        let entry_key = lookup!(fs, file_path);
        assert_is_file!(fs, entry_key);
        Ok(())
    }

    /// Creates a symlink directory
    #[tokio::test]
    async fn filesystem_create_symlink_directory() -> io::Result<()> {
        let tempdir = TempDir::new()?;
        let path = tempdir.path().to_path_buf();

        let fs = create_fs(&path);

        let a = path.join("a");
        let b = path.join("b");
        create_dir(&a)?;
        symlink_dir(&a, &b)?;

        take_events!(fs);

        let entry_key_dir = lookup!(fs, a);
        assert!(entry_key_dir.is_some());
        let entry_key_symlink = lookup!(fs, b);
        assert!(entry_key_dir.is_some());
        let _fs = fs.lock().await;
        let _entries = &_fs.entries;
        let _entries = _entries.borrow();
        match _entries.get(entry_key_dir.unwrap()).unwrap().deref() {
            Entry::Dir { .. } => {}
            _ => panic!("wrong entry type"),
        };

        assert!(matches!(
            _entries.get(entry_key_symlink.unwrap()).unwrap().deref(),
            Entry::Dir { .. }
        ));

        Ok(())
    }

    /// Creates a hardlink
    #[tokio::test]
    async fn filesystem_create_hardlink() -> io::Result<()> {
        let tempdir = TempDir::new()?;
        let path = tempdir.path().to_path_buf();

        let fs = create_fs(&path);

        let file_path = path.join("insert.log");
        let hard_path = path.join("hard.log");
        File::create(file_path.clone())?;
        hard_link(&file_path, &hard_path)?;

        take_events!(fs);

        let entry_key_file = lookup!(fs, file_path);
        let entry_key_link = lookup!(fs, hard_path);
        assert_is_file!(fs, entry_key_file);

        // Hardlinks are yielded as files
        assert_is_file!(fs, entry_key_link);

        Ok(())
    }

    // Deletes a directory
    #[tokio::test]
    async fn filesystem_delete_filled_dir() -> io::Result<()> {
        let tempdir = TempDir::new()?;
        let path = tempdir.path().to_path_buf();

        let file_path = path.join("file.log");
        let sym_path = path.join("sym.log");
        let hard_path = path.join("hard.log");
        File::create(file_path.clone())?;
        symlink_file(&file_path, &sym_path)?;
        hard_link(&file_path, &hard_path)?;

        let fs = create_fs(&path);

        assert!(lookup!(fs, path).is_some());
        assert!(lookup!(fs, file_path).is_some());
        assert!(lookup!(fs, sym_path).is_some());
        assert!(lookup!(fs, hard_path).is_some());

        tempdir.close()?;
        take_events!(fs);

        // It's a root dir, make sure it's still there
        assert!(lookup!(fs, path).is_some());
        assert!(lookup!(fs, file_path).is_none());
        assert!(lookup!(fs, sym_path).is_none());
        assert!(lookup!(fs, hard_path).is_none());

        Ok(())
    }

    // Deletes a directory
    #[tokio::test]
    async fn filesystem_delete_nested_filled_dir() -> io::Result<()> {
        // Now make a nested dir
        let rootdir = TempDir::new()?;
        let rootpath = rootdir.path().to_path_buf();

        let tempdir = TempDir::new_in(rootpath.clone())?;
        let path = tempdir.path().to_path_buf();

        let file_path = path.join("file.log");
        let sym_path = path.join("sym.log");
        let hard_path = path.join("hard.log");
        File::create(file_path.clone())?;
        symlink_file(&file_path, &sym_path)?;
        hard_link(&file_path, &hard_path)?;

        let fs = create_fs(&rootpath);

        assert!(lookup!(fs, path).is_some());
        assert!(lookup!(fs, file_path).is_some());
        assert!(lookup!(fs, sym_path).is_some());
        assert!(lookup!(fs, hard_path).is_some());

        tempdir.close()?;

        take_events!(fs);
        assert!(lookup!(fs, path).is_none());
        assert!(lookup!(fs, file_path).is_none());
        assert!(lookup!(fs, sym_path).is_none());
        assert!(lookup!(fs, hard_path).is_none());

        Ok(())
    }

    // Deletes a file
    #[tokio::test]
    async fn filesystem_delete_file() -> io::Result<()> {
        let _ = env_logger::Builder::from_default_env().try_init();
        let tempdir = TempDir::new()?;
        let path = tempdir.path().to_path_buf();

        let file_path = path.join("file.log");
        File::create(file_path.clone())?;

        let fs = create_fs(&path);

        assert!(lookup!(fs, file_path).is_some());

        remove_file(&file_path)?;
        take_events!(fs);

        take_events!(fs);
        take_events!(fs);

        assert!(lookup!(fs, file_path).is_none());

        Ok(())
    }

    // Deletes a symlink
    #[tokio::test]
    async fn filesystem_delete_symlink() {
        let tempdir = TempDir::new().unwrap();
        let path = tempdir.path().to_path_buf();

        let a = path.join("a");
        let b = path.join("b");
        create_dir(&a).unwrap();
        symlink_dir(&a, &b).unwrap();

        let fs = create_fs(&path);

        remove_dir_all(&b).unwrap();
        take_events!(fs);

        assert!(lookup!(fs, a).is_some());
        assert!(lookup!(fs, b).is_none());
    }

    /// Deletes a symlink that points to a not tracked directory
    #[tokio::test]
    async fn filesystem_delete_symlink_to_untracked_dir() -> io::Result<()> {
        let tempdir = TempDir::new()?;
        let tempdir2 = TempDir::new()?.into_path();
        let path = tempdir.path().to_path_buf();

        let real_dir_path = tempdir2.join("real_dir_sample");
        let symlink_path = path.join("symlink_sample");
        create_dir(&real_dir_path)?;
        symlink_dir(&real_dir_path, &symlink_path)?;

        let fs = create_fs(&path);
        assert!(lookup!(fs, symlink_path).is_some());
        // Symlink dirs are not followed
        assert!(lookup!(fs, real_dir_path).is_none());

        remove_dir_all(&symlink_path)?;
        take_events!(fs);

        assert!(lookup!(fs, symlink_path).is_none());
        assert!(lookup!(fs, real_dir_path).is_none());
        Ok(())
    }

    // Deletes the pointee of a symlink
    #[tokio::test]
    async fn filesystem_delete_symlink_pointee() -> io::Result<()> {
        let tempdir = TempDir::new()?;
        let path = tempdir.path().to_path_buf();

        let a = path.join("a");
        let b = path.join("b");
        File::create(&a)?;
        symlink_file(&a, &b)?;

        let fs = create_fs(&path);

        remove_file(&a)?;
        take_events!(fs);

        assert!(lookup!(fs, a).is_none());

        // Windows watcher might have dangling links and it's ok
        #[cfg(not(windows))]
        assert!(lookup!(fs, b).is_none());

        Ok(())
    }

    // Deletes a hardlink
    #[tokio::test]
    async fn filesystem_delete_hardlink() -> io::Result<()> {
        let tempdir = TempDir::new()?;
        let path = tempdir.path().to_path_buf();

        let a = path.join("a");
        let b = path.join("b");
        File::create(a.clone())?;
        hard_link(&a, &b)?;

        let fs = create_fs(&path);

        assert!(lookup!(fs, a).is_some());
        assert!(lookup!(fs, b).is_some());

        remove_file(&b)?;
        take_events!(fs);

        assert!(lookup!(fs, a).is_some());
        assert!(lookup!(fs, b).is_none());
        Ok(())
    }

    // Deletes the pointee of a hardlink (not totally accurate since we're not deleting the inode
    // entry, but what evs)
    #[tokio::test]
    async fn filesystem_delete_hardlink_pointee() -> io::Result<()> {
        let tempdir = TempDir::new()?;
        let path = tempdir.path().to_path_buf();

        let a = path.join("a");
        let b = path.join("b");
        File::create(a.clone())?;
        hard_link(&a, &b)?;

        let fs = create_fs(&path);

        remove_file(&a)?;
        take_events!(fs);

        assert!(lookup!(fs, a).is_none());
        assert!(lookup!(fs, b).is_some());
        Ok(())
    }

    /// Moves a directory within the watched directory
    ///
    /// Only run on unix-like systems as moving a directory on Windows with file handles open is
    /// not supported.
    #[cfg(unix)]
    #[tokio::test]
    async fn filesystem_move_dir_internal() -> io::Result<()> {
        let tempdir = TempDir::new()?.into_path();
        let path = tempdir.clone();

        let old_dir_path = path.join("old");
        let new_dir_path = path.join("new");
        let file_path = old_dir_path.join("file.log");
        let sym_path = old_dir_path.join("sym.log");
        let hard_path = old_dir_path.join("hard.log");
        create_dir(&old_dir_path)?;
        File::create(file_path.clone())?;
        symlink_file(&file_path, &sym_path)?;
        hard_link(&file_path, &hard_path)?;

        let fs = create_fs(&path);

        rename(&old_dir_path, &new_dir_path)?;
        take_events!(fs);

        assert!(lookup!(fs, old_dir_path).is_none());
        assert!(lookup!(fs, file_path).is_none());
        assert!(lookup!(fs, sym_path).is_none());
        assert!(lookup!(fs, hard_path).is_none());

        let entry = lookup!(fs, new_dir_path);
        assert!(entry.is_some());

        let entry2 = lookup!(fs, new_dir_path.join("file.log"));
        assert!(entry2.is_some());

        let entry3 = lookup!(fs, new_dir_path.join("hard.log"));
        assert!(entry3.is_some());

        let entry4 = lookup!(fs, new_dir_path.join("sym.log"));
        assert!(entry4.is_some());

        let _fs = fs.lock().await;
        let _entries = &_fs.entries;
        let _entries = _entries.borrow();
        match _entries.get(entry.unwrap()).unwrap().deref() {
            Entry::Dir { .. } => {}
            _ => panic!("wrong entry type"),
        };

        match _entries.get(entry2.unwrap()).unwrap().deref() {
            Entry::File { .. } => {}
            _ => panic!("wrong entry type"),
        };

        match _entries.get(entry3.unwrap()).unwrap().deref() {
            Entry::File { .. } => {}
            _ => panic!("wrong entry type"),
        };

        match _entries.get(entry4.unwrap()).unwrap().deref() {
            Entry::Symlink { link, .. } => {
                // symlinks don't update so this link is bad
                assert_eq!(*link, file_path);
            }
            _ => panic!("wrong entry type"),
        };

        Ok(())
    }

    /// Moves a directory out
    ///
    /// Only run on unix-like systems as moving a directory on Windows with file handles open is
    /// not supported.
    #[cfg(unix)]
    #[tokio::test]
    async fn filesystem_move_dir_out() -> io::Result<()> {
        let tempdir = TempDir::new()?;
        let path = tempdir.path().to_path_buf();

        let old_dir_path = path.join("old");
        let new_dir_path = path.join("new");
        let file_path = old_dir_path.join("file.log");
        let sym_path = old_dir_path.join("sym.log");
        let hard_path = old_dir_path.join("hard.log");
        create_dir(&old_dir_path)?;
        File::create(file_path.clone())?;
        symlink_file(&file_path, &sym_path)?;
        hard_link(&file_path, &hard_path)?;

        let fs = Arc::new(Mutex::new(new_fs::<()>(old_dir_path.clone(), None)));

        rename(&old_dir_path, &new_dir_path)?;
        take_events!(fs);

        assert!(lookup!(fs, new_dir_path).is_none());
        assert!(lookup!(fs, new_dir_path.join("file.log")).is_none());
        assert!(lookup!(fs, new_dir_path.join("hard.log")).is_none());
        assert!(lookup!(fs, new_dir_path.join("sym.log")).is_none());
        Ok(())
    }

    /// Moves a directory in
    ///
    /// Only run on unix-like systems as moving a directory on Windows with file handles open is
    /// not supported.
    #[cfg(unix)]
    #[tokio::test]
    async fn filesystem_move_dir_in() -> io::Result<()> {
        let old_tempdir = TempDir::new()?;
        let old_path = old_tempdir.path().to_path_buf();

        let new_tempdir = TempDir::new()?;
        let new_path = new_tempdir.path().to_path_buf();

        let old_dir_path = old_path.join("old");
        let new_dir_path = new_path.join("new");
        let file_path = old_dir_path.join("file.log");
        let sym_path = old_dir_path.join("sym.log");
        let hard_path = old_dir_path.join("hard.log");
        create_dir(&old_dir_path)?;
        File::create(file_path.clone())?;
        symlink_file(&file_path, &sym_path)?;
        hard_link(&file_path, &hard_path)?;

        let fs = Arc::new(Mutex::new(new_fs::<()>(new_path, None)));

        assert!(lookup!(fs, old_dir_path).is_none());
        assert!(lookup!(fs, new_dir_path).is_none());
        assert!(lookup!(fs, file_path).is_none());
        assert!(lookup!(fs, sym_path).is_none());
        assert!(lookup!(fs, hard_path).is_none());

        rename(&old_dir_path, &new_dir_path)?;
        take_events!(fs);

        let entry = lookup!(fs, new_dir_path);
        assert!(entry.is_some());

        let entry2 = lookup!(fs, new_dir_path.join("file.log"));
        assert!(entry2.is_some());

        let entry3 = lookup!(fs, new_dir_path.join("hard.log"));
        assert!(entry3.is_some());

        let entry4 = lookup!(fs, new_dir_path.join("sym.log"));
        assert!(entry4.is_some());

        let _fs = fs.lock().await;
        let _entries = &_fs.entries;
        let _entries = _entries.borrow();
        match _entries.get(entry.unwrap()).unwrap().deref() {
            Entry::Dir { .. } => {}
            _ => panic!("wrong entry type"),
        };

        match _entries.get(entry2.unwrap()).unwrap().deref() {
            Entry::File { .. } => {}
            _ => panic!("wrong entry type"),
        };

        match _entries.get(entry3.unwrap()).unwrap().deref() {
            Entry::File { .. } => {}
            _ => panic!("wrong entry type"),
        };

        match _entries.get(entry4.unwrap()).unwrap().deref() {
            Entry::Symlink { link, .. } => {
                // symlinks don't update so this link is bad
                assert_eq!(*link, file_path);
            }
            _ => panic!("wrong entry type"),
        };
        Ok(())
    }

    // Moves a file within the watched directory
    #[tokio::test]
    async fn filesystem_move_file_internal() -> io::Result<()> {
        let tempdir = TempDir::new()?;
        let path = tempdir.path().to_path_buf();

        let fs = Arc::new(Mutex::new(new_fs::<()>(path.clone(), None)));

        let file_path = path.join("insert.log");
        let new_path = path.join("new.log");
        File::create(file_path.clone())?;
        rename(&file_path, &new_path)?;

        take_events!(fs);

        let entry = lookup!(fs, file_path);
        assert!(entry.is_none());

        let entry = lookup!(fs, new_path);
        assert!(entry.is_some());
        let _fs = fs.lock().await;
        let _entries = &_fs.entries;
        let _entries = _entries.borrow();
        match _entries.get(entry.unwrap()).unwrap().deref() {
            Entry::File { .. } => {}
            _ => panic!("wrong entry type"),
        };
        Ok(())
    }

    // Moves a file out of the watched directory
    #[tokio::test]
    async fn filesystem_move_file_out() -> io::Result<()> {
        let tempdir = TempDir::new()?;
        let path = tempdir.path().to_path_buf();

        let watch_path = path.join("watch");
        let other_path = path.join("other");
        create_dir(&watch_path)?;
        create_dir(&other_path)?;

        let file_path = watch_path.join("inside.log");
        let move_path = other_path.join("outside.log");
        File::create(file_path.clone())?;

        let fs = Arc::new(Mutex::new(new_fs::<()>(watch_path, None)));

        rename(&file_path, &move_path)?;

        take_events!(fs);

        let entry = lookup!(fs, file_path);
        assert!(entry.is_none());

        let entry = lookup!(fs, move_path);
        assert!(entry.is_none());
        Ok(())
    }

    // Moves a file into the watched directory
    #[tokio::test]
    async fn filesystem_move_file_in() -> io::Result<()> {
        let tempdir = TempDir::new()?;
        let path = tempdir.path().to_path_buf();

        let watch_path = path.join("watch");
        let other_path = path.join("other");
        create_dir(&watch_path)?;
        create_dir(&other_path)?;

        let file_path = other_path.join("inside.log");
        let move_path = watch_path.join("outside.log");
        File::create(file_path.clone())?;

        let fs = Arc::new(Mutex::new(new_fs::<()>(watch_path, None)));

        rename(&file_path, &move_path)?;
        File::create(file_path.clone())?;

        take_events!(fs);

        let entry = lookup!(fs, file_path);
        assert!(entry.is_none());

        let entry = lookup!(fs, move_path);
        assert!(entry.is_some());
        let _fs = fs.lock().await;
        let _entries = &_fs.entries;
        let _entries = _entries.borrow();
        match _entries.get(entry.unwrap()).unwrap().deref() {
            Entry::File { .. } => {}
            _ => panic!("wrong entry type"),
        };
        Ok(())
    }

    /// Moves a file out of the watched directory
    ///
    /// Only run on unix-like systems as moving stuff on Windows when being read is not handled
    /// correctly.
    #[cfg(unix)]
    #[tokio::test]
    async fn filesystem_move_symlink_file_out() -> io::Result<()> {
        let _ = env_logger::Builder::from_default_env().try_init();
        let tempdir = TempDir::new()?;
        let path = tempdir.path().to_path_buf();

        let watch_path = path.join("watch");
        let other_path = path.join("other");
        create_dir(&watch_path)?;
        create_dir(&other_path)?;

        let file_path = other_path.join("inside.log");
        let move_path = other_path.join("outside.tmp");
        let sym_path = watch_path.join("sym.log");
        File::create(file_path.clone())?;
        symlink_file(&file_path, &sym_path)?;

        let fs = Arc::new(Mutex::new(new_fs::<()>(watch_path, None)));

        take_events!(fs);

        let entry = lookup!(fs, sym_path);
        assert!(entry.is_some());

        let entry = lookup!(fs, file_path);
        assert!(entry.is_some());

        let entry = lookup!(fs, move_path);
        assert!(entry.is_none());

        rename(&file_path, &move_path)?;

        take_events!(fs);

        // Dangling symlinks are not tracked
        let entry = lookup!(fs, sym_path);
        assert!(entry.is_none());

        let entry = lookup!(fs, file_path);
        assert!(entry.is_none());

        let entry = lookup!(fs, move_path);
        assert!(entry.is_none());
        Ok(())
    }

    // Watch symlink target that is excluded
    #[tokio::test]
    async fn filesystem_watch_symlink_w_excluded_target() -> io::Result<()> {
        let tempdir = TempDir::new()?;
        let path = tempdir.path().to_path_buf();

        let mut rules = Rules::new();
        rules.add_inclusion(RuleDef::glob_rule("*.log").unwrap());
        rules.add_inclusion(
            RuleDef::glob_rule(&*format!("{}{}", tempdir.path().to_str().unwrap(), "*")).unwrap(),
        );
        rules.add_exclusion(RuleDef::glob_rule("*.tmp").unwrap());
        let file_path = path.join("test.tmp");
        let sym_path = path.join("test.log");
        File::create(file_path.clone())?;

        let fs = Arc::new(Mutex::new(new_fs::<()>(path, Some(rules))));

        let entry = lookup!(fs, file_path);
        assert!(entry.is_none());

        symlink_file(&file_path, &sym_path)?;

        take_events!(fs);
        let entry = lookup!(fs, sym_path);
        assert!(entry.is_some());
        Ok(())
    }

    #[tokio::test]
    async fn filesystem_test_basic_ops_per_platform() -> io::Result<()> {
        let tempdir = TempDir::new()?;
        let tempdir2 = TempDir::new()?;
        let path = tempdir.path().to_path_buf();

        let file1_path = path.join("test_file.log");
        let file2_path = path.join("another_file.log");
        let sym_path = path.join("test_symlink.log");
        let sub_dir_path = path.join("test_dir");
        let sub_dir_file_path = sub_dir_path.join("test_sub_file.log");

        create_dir(&sub_dir_path)?;
        let mut file1 = File::create(&file1_path)?;
        let mut file2 = File::create(&file2_path)?;
        let mut file3 = File::create(&sub_dir_file_path)?;
        symlink_file(&file1_path, &sym_path)?;

        let fs = Arc::new(Mutex::new(new_fs::<()>(path, None)));

        let entry = lookup!(fs, file1_path);
        assert!(entry.is_some());

        writeln!(file1, "hello")?;
        writeln!(file2, "hello")?;
        writeln!(file3, "hello")?;

        drop(file1);
        drop(file2);
        drop(file3);

        take_events!(fs);

        remove_file(&file1_path).unwrap();
        remove_file(&sym_path).unwrap();

        // Move file out of directory
        rename(&file2_path, tempdir2.path().join("another_file.log")).unwrap();

        // Remove dir with contents (intermittently fails on windows)
        #[cfg(unix)]
        remove_dir_all(&sub_dir_path).unwrap();

        Ok(())
    }
}

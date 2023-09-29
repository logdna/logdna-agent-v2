use crate::cache::delayed_stream::delayed_stream;
use crate::cache::entry::Entry;
use crate::cache::event::Event;
use crate::cache::tailed_file::TailedFile;
use crate::lookback::Lookback;
use crate::rule::{RuleDef, Rules, Status};

use metrics::Metrics;
use notify_stream::{Event as WatchEvent, RecursiveMode, Watcher};
use state::{FileId, Span, SpanVec};

use std::cell::RefCell;
use std::collections::hash_map::Entry as HashMapEntry;
use std::collections::{HashMap, HashSet};
use std::convert::TryFrom;
use std::ffi::OsString;
use std::ops::Deref;
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, SystemTime};
use std::{fmt, fs, io};

use futures::{Stream, StreamExt};
use slotmap::{DefaultKey, SlotMap};
use smallvec::SmallVec;
use thiserror::Error;
use time::OffsetDateTime;
use tracing::{debug, error, info, instrument, trace, warn};

use state::{FileOffsetFlushHandle, FileOffsetWriteHandle};

pub mod delayed_stream;
pub mod dir_path;
pub mod entry;
pub mod event;
mod event_debouncer;
pub mod tailed_file;

pub use dir_path::{DirPathBuf, DirPathBufError};

type Children = HashMap<OsString, EntryKey>;
type Symlinks = HashMap<PathBuf, Vec<EntryKey>>;
type WatchDescriptors = HashMap<PathBuf, Vec<EntryKey>>;

pub type EntryKey = DefaultKey;

type EntryMap = SlotMap<EntryKey, entry::Entry>;
type FsResult<T> = Result<T, Error>;

pub const EVENT_STREAM_BUFFER_COUNT: usize = 1000;
const EVENT_RETRY_DELAY_MS: u64 = 1000;
const EVENT_RETRY_COUNT: u32 = 5;
const TAIL_WARN_THRESHOLD_B: u64 = 3000000;

#[derive(Debug, Error)]
pub enum Error {
    #[error("error watching: {0:?} {1:?}")]
    Watch(Option<PathBuf>, notify_stream::Error),
    #[error("error need to rescan")]
    Rescan,
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
    File(#[from] io::Error),
}

enum FsEntry {
    File {
        path: PathBuf,
        inode: u64,
    },
    Dir {
        path: PathBuf,
    },
    Symlink {
        path: PathBuf,
        target: Option<PathBuf>,
    },
}

impl TryFrom<&Path> for FsEntry {
    type Error = std::io::Error;

    fn try_from(path: &Path) -> Result<Self, std::io::Error> {
        let meta = path_abs::PathInfo::symlink_metadata(path)?;
        if meta.file_type().is_symlink() {
            Ok(FsEntry::Symlink {
                path: path.to_path_buf(),
                target: match path_abs::PathInfo::read_link(path) {
                    Ok(p) => Some(p),
                    Err(e) if e.io_error().kind() == std::io::ErrorKind::NotFound => None,
                    Err(e) => return Err(e.into()),
                },
            })
        } else if meta.file_type().is_dir() {
            Ok(FsEntry::Dir {
                path: path.to_path_buf(),
            })
        } else if meta.file_type().is_file() {
            #[cfg(unix)]
            let inode = get_inode(path, None)?;
            #[cfg(windows)]
            let inode = {
                let file = path_abs::FileRead::open(path)?;
                get_inode(path, Some(file.as_ref()))?
            };
            Ok(FsEntry::File {
                path: path.to_path_buf(),
                inode,
            })
        } else {
            panic!(
                "Got valid path that's neither file, dir nor symlink: {:#?}",
                path
            );
        }
    }
}

type EventTimestamp = time::OffsetDateTime;

#[cfg(unix)]
pub fn get_inode(path: &Path, _file: Option<&std::fs::File>) -> std::io::Result<u64> {
    use std::os::unix::fs::MetadataExt;

    Ok(path
        .metadata()
        .map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("unable to retrieve inode for path \"{:?}\": {:?}", path, e),
            )
        })?
        .ino())
}

#[cfg(windows)]
pub fn get_inode(_path: &Path, file: Option<&std::fs::File>) -> std::io::Result<u64> {
    use winapi_util::AsHandleRef;

    file.ok_or_else(|| std::io::Error::new(std::io::ErrorKind::Other, "expected a file handle"))
        .and_then(|file| Ok(winapi_util::file::information(file.as_handle_ref())?.file_index()))
}

/// Turns an inotify event into an event stream
fn as_event_stream(
    fs: Rc<RefCell<FileSystem>>,
    watch_event: &WatchEvent,
    event_time: EventTimestamp,
) -> impl Stream<Item = (Result<Event, Error>, EventTimestamp)> {
    let a = match fs.borrow_mut().process(watch_event) {
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
    fs: &Rc<RefCell<FileSystem>>,
) -> impl Stream<Item = (Result<Event, Error>, EventTimestamp)> {
    let init_time = OffsetDateTime::now_utc();
    let initial_events = {
        let mut fs = fs.borrow_mut();

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
    fs: &Rc<RefCell<FileSystem>>,
) -> impl Stream<Item = (WatchEvent, EventTimestamp)> {
    let resume_events_recv = fs.borrow().resume_events_recv.clone();
    resume_events_recv
        .map({
            let fs = fs.clone();
            move |(inode, event_time)| {
                fs.borrow()
                    .wd_by_inode
                    .get(&inode)
                    .map(|wd| (WatchEvent::Write(wd.clone()), event_time))
            }
        })
        .filter_map(|e| async { e })
}

#[instrument(level = "debug", skip_all)]
fn get_retry_events(
    fs: &Rc<RefCell<FileSystem>>,
    retry_delay: Duration,
    retry_limit: u32,
) -> impl Stream<Item = (WatchEvent, EventTimestamp, u32)> {
    let retry_events = fs.borrow().retry_events_recv.clone();
    delayed_stream(retry_events, retry_delay).filter_map(move |(event, ts, retries)| async move {
        debug!("retry {} for event {:?}", retries + 1, event);
        if retries <= retry_limit {
            Some((event, ts, retries + 1))
        } else {
            None
        }
    })
}

pub struct FileSystem {
    watcher: Watcher,
    missing_dir_watcher: Option<Watcher>,
    pub entries: Rc<RefCell<EntryMap>>,
    symlinks: Symlinks,
    watch_descriptors: WatchDescriptors,
    wd_by_inode: HashMap<u64, PathBuf>,
    master_rules: Rules,
    initial_dirs: Vec<DirPathBuf>,
    initial_dir_rules: Rules,
    missing_dirs: Vec<PathBuf>,
    initial_events: Vec<Event>,
    fs_start_time: SystemTime,

    lookback_config: Lookback,
    initial_offsets: HashMap<FileId, SpanVec>,

    resume_events_recv: async_channel::Receiver<(u64, EventTimestamp)>,
    resume_events_send: async_channel::Sender<(u64, EventTimestamp)>,

    retry_events_recv: async_channel::Receiver<(WatchEvent, EventTimestamp, u32)>,
    retry_events_send: async_channel::Sender<(WatchEvent, EventTimestamp, u32)>,

    ignored_dirs: HashSet<PathBuf>,

    fo_state_handles: Option<(FileOffsetWriteHandle, FileOffsetFlushHandle)>,

    deletion_ack_sender: async_channel::Sender<Vec<PathBuf>>,

    _c: countme::Count<Self>,
}

fn add_initial_dir_rules(rules: &mut Rules, path: &DirPathBuf) {
    // Include one for self and the rest of its children
    #[cfg(windows)]
    let path_rest = format!(
        "{}\\*",
        path.to_str()
            .expect("invalid unicode in path")
            .trim_end_matches(std::path::is_separator)
    );
    #[cfg(not(windows))]
    let path_rest = format!(
        "{}/*",
        path.to_str()
            .expect("invalid unicode in path")
            .trim_end_matches(std::path::is_separator)
    );
    rules.add_inclusion(RuleDef::glob_rule(path_rest.as_str()).expect("invalid glob rule format"));
    rules.add_inclusion(
        RuleDef::glob_rule(path.to_str().expect("invalid unicode in path"))
            .expect("invalid glob rule format"),
    );
}

impl FileSystem {
    #[instrument(level = "debug", skip_all)]
    pub fn new(
        initial_dirs_original: Vec<DirPathBuf>,
        lookback_config: Lookback,
        initial_offsets: HashMap<FileId, SpanVec>,
        rules: Rules,
        fo_state_handles: Option<(FileOffsetWriteHandle, FileOffsetFlushHandle)>,
        deletion_ack_sender: async_channel::Sender<Vec<PathBuf>>,
    ) -> Self {
        let (resume_events_send, resume_events_recv) = async_channel::unbounded();
        let (retry_events_send, retry_events_recv) = async_channel::unbounded();

        let mut initial_dirs_checked = initial_dirs_original.clone();
        initial_dirs_checked.iter_mut().for_each(|path| {
            // if postfix is Some then dir does not exists
            // check again if non-existing path exists now
            if let Some(postfix) = path.clone().postfix {
                let mut full_path = PathBuf::new();
                full_path.push(&path.inner);
                full_path.push(&postfix);
                if full_path.exists() {
                    *path = DirPathBuf {
                        inner: full_path.clone(),
                        postfix: None,
                    }
                } else {
                    *path = DirPathBuf::try_from(full_path).unwrap();
                }
            }
            if !path.inner.is_dir() {
                panic!("initial dirs must be dirs")
            }
        });

        let fs_start_time = SystemTime::now();

        let mut missing_dirs: Vec<PathBuf> = Vec::new();

        let watcher = Watcher::new();
        let mut missing_dir_watcher = Watcher::new();
        let entries = SlotMap::new();

        let mut initial_dir_rules = Rules::new();
        let ignored_dirs = HashSet::new();

        // Adds initial directories and constructs missing directory
        // vector and adds prefix path to the missing directory watcher
        for dir_path in initial_dirs_checked.iter() {
            match &dir_path.postfix {
                None => {
                    add_initial_dir_rules(&mut initial_dir_rules, dir_path);
                }
                Some(postfix) => {
                    let mut full_missing_path = PathBuf::new();
                    full_missing_path.push(&dir_path.inner);
                    full_missing_path.push(postfix);
                    let full_missing_dirpathbuff = DirPathBuf {
                        inner: full_missing_path.clone(),
                        postfix: None,
                    };
                    // Add missing directory to Rules
                    add_initial_dir_rules(&mut initial_dir_rules, &full_missing_dirpathbuff);
                    info!("adding {:?} to missing directory watcher", &dir_path.inner);
                    missing_dirs.push(full_missing_path);
                    missing_dir_watcher
                        .watch(&dir_path.inner, RecursiveMode::NonRecursive)
                        .expect("Could not add path to missing directory watcher");
                }
            }
        }
        debug!("initial directory rules: {:?}\n", initial_dir_rules);

        let mut fs = Self {
            entries: Rc::new(RefCell::new(entries)),
            symlinks: Symlinks::new(),
            watch_descriptors: WatchDescriptors::new(),
            wd_by_inode: HashMap::new(),
            master_rules: rules,
            initial_dirs: initial_dirs_checked.clone(),
            initial_dir_rules,
            missing_dirs,
            fs_start_time,
            lookback_config,
            initial_offsets,
            watcher,
            missing_dir_watcher: Some(missing_dir_watcher),
            initial_events: Vec::new(),
            resume_events_recv,
            resume_events_send,
            retry_events_recv,
            retry_events_send,
            ignored_dirs,
            fo_state_handles,
            deletion_ack_sender,
            _c: countme::Count::new(),
        };

        let entries = fs.entries.clone();
        let mut entries = entries.borrow_mut();

        // Initial dirs
        let mut initial_dir_events = SmallVec::new();
        for dir in initial_dirs_checked
            .into_iter()
            .map(|path| -> PathBuf { path.into() })
        {
            let mut path_cpy: PathBuf = dir.clone();
            while path_cpy.components().count() > 1 {
                if !path_cpy.exists() {
                    path_cpy.pop();
                } else {
                    if let Err(e) = fs.insert(&path_cpy, &mut initial_dir_events, &mut entries) {
                        // It can failed due to permissions or some other restriction
                        debug!(
                            "Initial insertion of {} failed: {:?}",
                            path_cpy.to_str().unwrap(),
                            e
                        );
                    }
                    break;
                }
            }

            if path_cpy.components().count() <= 1 {
                debug!("Not recursively walking paths under {:?}", path_cpy);
                continue;
            }

            debug!("recursively walking paths under {:#?}", dir);

            let recursive_found_paths = walkdir::WalkDir::new(&dir)
                .follow_links(true)
                .into_iter()
                .filter_map(|path| {
                    path.ok().and_then(|path| {
                        fs.is_initial_dir_target(path.path())
                            .then(|| path.path().to_path_buf())
                    })
                })
                .filter_map(|path: PathBuf| {
                    fs::read_dir(&path).ok().map(|entries| {
                        let mut ret = vec![path];
                        ret.extend(entries.filter_map(|entry| {
                            entry
                                .ok()
                                .and_then(|entry| entry.path().is_file().then_some(entry.path()))
                        }));
                        ret
                    })
                })
                .flatten()
                .collect::<Vec<PathBuf>>();

            debug!(
                "recursively discovered paths under {:#?}: {:#?}",
                dir, recursive_found_paths
            );

            for path in recursive_found_paths.iter() {
                if let Err(e) = fs.insert(path, &mut initial_dir_events, &mut entries) {
                    // It can failed due to file restrictions
                    debug!(
                        "Initial recursive scan insertion of {} failed: {:?}",
                        path.to_str().unwrap(),
                        e
                    );
                }
            }
        }

        // send event to FileOffsets state to cleanup unused inodes
        let mut inodes: Vec<FileId> = Vec::new();
        // TODO convert to call-chain
        for entry in entries.values() {
            if let Entry::File { data, .. } = entry {
                let ino = data
                    .borrow()
                    .get_inner()
                    .try_lock()
                    .expect("Arc<Mutex<TailedFileInner>> clone detected!")
                    .get_inode();
                inodes.push(ino);
            }
        }
        if let Some((_, state_flush)) = fs.fo_state_handles.clone() {
            state_flush
                .do_gc_blocking(inodes)
                .expect("FileOffset state Garbage Collection failed")
        }
        for event in initial_dir_events {
            match event {
                Event::New(entry_key) => fs.initial_events.push(Event::Initialize(entry_key)),
                _ => panic!("unexpected event in initialization"),
            };
        }
        fs
    }

    // Stream events
    #[instrument(level = "debug", skip_all)]
    pub fn stream_events(
        fs: Rc<RefCell<FileSystem>>,
    ) -> Result<impl Stream<Item = Result<Event, Error>>, std::io::Error> {
        let notify_events_stream = fs.borrow().watcher.receive();

        let (missing_dirs, missing_dir_watcher, missing_dir_event_stream, retry_event_sender) = {
            let mut _mfs = fs.borrow_mut();

            let missing_dirs = _mfs.missing_dirs.clone();
            let missing_dir_watcher = _mfs.missing_dir_watcher.take().unwrap_or_else(Watcher::new);

            let missing_dir_event_stream = missing_dir_watcher.receive();
            let retry_event_sender = _mfs.retry_events_send.clone();
            (
                missing_dirs,
                missing_dir_watcher,
                missing_dir_event_stream,
                retry_event_sender,
            )
        };

        let missing_dir_event = futures::stream::unfold(
            (
                fs.clone(),
                missing_dirs,
                missing_dir_watcher,
                Box::pin(missing_dir_event_stream),
            ),
            move |(mfs, missing, mut watcher, mut stream)| async move {
                loop {
                    let (event, _) = stream.next().await?;
                    debug!("missing directory watcher event: {:?}", event);
                    if let notify_stream::Event::Create(ref path) = event {
                        if missing.contains(path) {
                            // Got a complete directory match, inserting it
                            info!(
                                "missing initial log directory {:?} was found! triggering FS rescan.",
                                path
                            );
                            return Some((
                                as_event_stream(
                                    mfs.clone(),
                                    &notify_stream::Event::Rescan,
                                    OffsetDateTime::now_utc(),
                                ),
                                (mfs, missing, watcher, stream),
                            ));
                        }
                        if missing.iter().any(|m| m.starts_with(path)) {
                            info!("found sub-path of missing directory {:?}", path);
                            for dir in missing.iter() {
                                // Check if full path was created along with sub-path
                                if dir.exists() {
                                    info!("full path exists {:?}", dir);
                                    let create_event = WatchEvent::Create(dir.clone());
                                    return Some((
                                        as_event_stream(
                                            mfs.clone(),
                                            &create_event,
                                            OffsetDateTime::now_utc(),
                                        ),
                                        (mfs, missing, watcher, stream),
                                    ));
                                }
                                watcher
                                    .watch(path, RecursiveMode::NonRecursive)
                                    .expect("Could not add inital value to missing_dir_watch");
                            }
                        }
                    }
                }
            },
        )
        .flatten();

        use futures::future::Either;

        let resume_events = Box::pin(get_resume_events(&fs));

        let events = event_debouncer::debounce_fs_events(notify_events_stream, resume_events)
            .map(Either::Left);

        let retry_events = get_retry_events(
            &fs,
            Duration::from_millis(EVENT_RETRY_DELAY_MS),
            EVENT_RETRY_COUNT,
        )
        .map(Either::Right);

        let events = futures::stream::select(events, retry_events)
            .map({
                let fs = fs.clone();
                move |e| {
                    let (event, event_time, retries) = match e {
                        Either::Left((event, event_time)) => (event, event_time, None),
                        Either::Right((event, event_time, retries)) => {
                            (event, event_time, Some(retries))
                        }
                    };
                    let fs = fs.clone();
                    as_event_stream(fs, &event, event_time).filter_map({
                        let retry_event_sender = retry_event_sender.clone();
                        move |(e, event_time)| {
                            let retry_event_sender = retry_event_sender.clone();
                            // Make sure we only clone the event if there's a error
                            let event = e.as_ref().err().map(|_| event.clone());
                            async move {
                                match e {
                                    Ok(_) => Some((e, event_time)),
                                    Err(Error::Rescan) => Some((e, event_time)),
                                    Err(e) => {
                                        match e {
                                            Error::File(io_err)
                                            | Error::DirectoryListNotValid(io_err, _) => {
                                                if io_err.kind() == io::ErrorKind::NotFound {
                                                    {}
                                                } else {
                                                    retry_event_sender
                                                        .send((
                                                            event.unwrap(),
                                                            event_time,
                                                            retries.unwrap_or(0),
                                                        ))
                                                        .await
                                                        .unwrap();
                                                }
                                            }
                                            _ => {}
                                        }
                                        None
                                    }
                                }
                            }
                        }
                    })
                }
            })
            .flatten();

        let initial_events = get_initial_events(&fs);
        Ok(
            futures::stream::select(initial_events.chain(events), missing_dir_event)
                .map(|(event, _)| event),
        )
    }

    /// Handles inotify events and may produce Event(s) that are returned upstream through sender
    #[instrument(level = "debug", skip_all)]
    fn process(&mut self, watch_event: &WatchEvent) -> FsResult<SmallVec<[Event; 2]>> {
        let _entries = self.entries.clone();
        let mut _entries = _entries.borrow_mut();

        debug!("handling notify event {:#?}", watch_event);

        // TODO: Remove OsString names
        let result = match watch_event {
            WatchEvent::Create(wd) => self.process_create(wd, &mut _entries),
            WatchEvent::Write(wd) => match self.process_modify(wd) {
                Err(Error::WatchEvent(_)) => {
                    self.process_create(wd, &mut _entries).and_then(|mut e| {
                        e.extend(self.process_modify(wd)?);
                        Ok(e)
                    })
                }
                e => e,
            },
            WatchEvent::Remove(wd) => self.process_delete(wd, &mut _entries),
            WatchEvent::Rename(from_wd, to_wd) => {
                // Source path should exist and be tracked to be a move
                let is_from_path_ok = self
                    .get_first_entry(from_wd)
                    .map(|entry| self.entry_path_passes(entry, &_entries))
                    .unwrap_or(false);

                // Target path pass the inclusion/exclusion rules to be a move
                let is_to_path_ok = self.passes(to_wd, &_entries);

                if is_to_path_ok && is_from_path_ok {
                    self.process_rename(from_wd, to_wd, &mut _entries)
                } else if is_to_path_ok {
                    self.process_create(to_wd, &mut _entries)
                } else if is_from_path_ok {
                    self.process_delete(from_wd, &mut _entries)
                } else {
                    // Most likely parent was removed, dropping all child watch descriptors
                    // and we've got the child watch event already queued up
                    debug!("Move event received from targets that are not watched anymore");
                    Ok(SmallVec::new())
                }
            }
            WatchEvent::Error(e, p) => {
                debug!(
                    "There was an error mapping a file change: {:?} ({:?})",
                    e, p
                );
                Err(Error::Watch(p.clone(), e.clone()))
            }
            WatchEvent::Rescan => Err(Error::Rescan),
        };

        if let Err(ref e) = result {
            match e {
                Error::PathNotValid(path) => {
                    debug!("Path is not longer valid: {}", path.display());
                }
                Error::WatchEvent(path) => {
                    debug!("Processing event for untracked path: {}", path.display());
                }
                Error::File(err) => {
                    warn!("Processing notify event resulted in error: {:?}", e);
                    if err.to_string().contains("(os error 24)") {
                        error!(
                            "Agent process has hit the limit of maximum number of open files. \
                            Try to increase the Open Files system limit."
                        );
                        std::process::exit(24);
                    }
                }
                Error::Rescan => {
                    warn!("Processing notify event resulted in FS rescan");
                }
                _ => {
                    warn!("Processing notify event resulted in error: {:?}", e);
                }
            }
        }
        result
    }

    fn process_create(
        &mut self,
        watch_descriptor: &Path,
        _entries: &mut EntryMap,
    ) -> FsResult<SmallVec<[Event; 2]>> {
        let path = &watch_descriptor;

        let mut events = SmallVec::new();
        //TODO: Check duplicates
        self.insert(path, &mut events, _entries)?;
        Ok(events)
    }

    fn process_modify(&mut self, watch_descriptor: &Path) -> FsResult<SmallVec<[Event; 2]>> {
        if let Some(entries) = self.watch_descriptors.get_mut(watch_descriptor) {
            Ok(entries
                .iter()
                .map(|entry_ptr| Event::Write(*entry_ptr))
                .collect())
        } else {
            Err(Error::WatchEvent(watch_descriptor.to_owned()))
        }
    }

    #[instrument(level = "trace", skip_all)]
    fn is_reachable(
        &self,
        cuts: &mut Vec<PathBuf>,
        target: &Path,
        _entries: &EntryMap,
    ) -> FsResult<bool> {
        debug!("checking if {:#?} is reachable", target);

        let mut target_m = target;
        let target_mref = &mut target_m;

        if let Some(entry_key) = self.get_first_entry(target_mref) {
            let entry = _entries.get(entry_key).ok_or(Error::Lookup)?;
            if let Entry::Symlink {
                link: Some(ref link),
                ..
            } = entry
            {
                // If target is a symlink then we should not traverse it again in recursive calls
                cuts.push(target_mref.to_path_buf());
                let _ = std::mem::replace(target_mref, link);
            }
        }

        // Cuts is the list of paths we've checked
        if !cuts.iter().any(|cut| cut.as_path() == *target_mref)
            && (self.is_initial_dir_target(target_mref)
                || self.is_symlink_target(target_mref, _entries))
        {
            trace!("short circuit {:?} is reachable", target_mref);
            return Ok(true);
        }

        let mut path_to_root = false;

        // Walk up the parents until a root is found
        while let Some(parent) = target_mref.parent().take() {
            path_to_root = self
                .referring_symlinks(parent, _entries)
                .iter()
                .map(|target| self.is_reachable(cuts, target, _entries))
                .try_fold(false, |acc, reachable| reachable.map(|inner| acc || inner))?;

            if cuts.iter().any(|cut| cut == parent) {
                path_to_root = false;
                break;
            }
            if self.passes_rules(parent) || self.is_symlink_target(parent, _entries) {
                return Ok(true);
            }
            let _ = std::mem::replace(target_mref, parent);
        }
        trace!("{:?} is reachable?: {}", target_mref, path_to_root);
        Ok(path_to_root)
    }

    fn referring_symlinks(&self, target: &Path, _entries: &EntryMap) -> Vec<PathBuf> {
        let mut referring = Vec::new();

        for (_, symlink_ptrs) in self.symlinks.iter() {
            for symlink_ptr in symlink_ptrs.iter() {
                if let Some(symlink) = _entries.get(*symlink_ptr) {
                    match symlink {
                        Entry::Symlink { path, .. } => {
                            if path == target {
                                referring.push(path.clone())
                            }
                        }
                        _ => {
                            panic!(
                                "did not expect non symlink entry in symlinks master map for path {:?}",
                                target
                            );
                        }
                    }
                } else {
                    error!("failed to find entry from symlink targets");
                };
            }
        }
        referring
    }

    #[instrument(level = "trace", skip_all)]
    fn process_delete(
        &mut self,
        watch_descriptor: &Path,
        _entries: &mut EntryMap,
    ) -> FsResult<SmallVec<[Event; 2]>> {
        let mut events = SmallVec::new();
        if let Some(entry_key) = self.get_first_entry(watch_descriptor) {
            let entry = _entries.get(entry_key).ok_or(Error::Lookup)?;
            let path = entry.path().to_path_buf();
            if !self.initial_dirs.iter().any(|dir| dir.as_ref() == path) {
                self.remove(&path, &mut events, _entries)?;
            } else {
                info!("initial log dir {:?} removed, requesting FS rescan", path);
                return Err(Error::Rescan);
            }
        } else {
            // The file is already gone (event was queued up), safely ignore
            trace!(
                "processing delete for {:?} was ignored as entry was not found",
                watch_descriptor
            );
        }
        debug!("PROCESS DELETE RETURNING EVENTS: {:?}", &events);
        Ok(events)
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
            Lookback::Tail => {
                let mut should_lookback = false;
                let file_len = path.metadata().map(|m| m.len()).unwrap_or(0);

                if let Ok(metadata) = path.metadata() {
                    if let Ok(file_create_time) = metadata.created() {
                        if file_create_time > self.fs_start_time {
                            should_lookback = true
                        }
                    }
                }

                let smallfiles_offset = if should_lookback {
                    if file_len > TAIL_WARN_THRESHOLD_B {
                        warn!("lookback ocurred on larger file {:?}", path);
                    }

                    SpanVec::new()
                } else {
                    [Span::new(0, file_len).unwrap()].iter().collect()
                };

                _lookup_offset(&self.initial_offsets, &inode, path).unwrap_or(smallfiles_offset)
            }
        }
    }

    pub fn resolve_valid_paths(&self, entry: &Entry, entries: &EntryMap) -> SmallVec<[PathBuf; 4]> {
        // TODO: extract these Vecs or replace with SmallVec
        let mut paths = Vec::new();
        let mut current_path_buf: PathBuf = PathBuf::with_capacity(entry.path().as_os_str().len());
        self.resolve_valid_paths_helper(entry, &mut paths, &[], &mut current_path_buf, entries);
        // If there is exactly one valid path reuse our current_path_buf allocation
        if paths.len() == 1 {
            current_path_buf.clear();
            current_path_buf.push(paths.first().unwrap());
            smallvec::smallvec![current_path_buf]
        } else {
            paths.iter().map(PathBuf::from).collect()
        }
    }

    fn resolve_valid_paths_helper<'a>(
        &self,
        entry: &'a Entry,
        paths: &mut Vec<&'a OsStr>,
        components: &[&'a OsStr],
        current_path_buf: &mut PathBuf,
        entries: &'a EntryMap,
    ) {
        let components_len = components.len();
        let mut base_components: SmallVec<[&OsStr; 12]> = into_components(entry.path());
        base_components.extend_from_slice(components); // add components already discovered from previous recursive step
        if self.is_initial_dir_target(entry.path()) && !entry.path().is_dir() {
            // only want paths that fall in our watch window
            paths.push(entry.path().as_os_str());
        }

        let raw_components = base_components.as_slice();
        for i in 0..raw_components.len() - components_len {
            // only need to iterate components up to current entry
            current_path_buf.clear();
            current_path_buf.extend(raw_components[0..=i].iter());

            if let Some(symlinks) = self.symlinks.get(current_path_buf.as_path()) {
                // check if path has a symlink to it
                let symlink_components = &raw_components[(i + 1)..];
                for symlink_ptr in symlinks.iter() {
                    let symlink = entries.get(*symlink_ptr);
                    if let Some(symlink) = symlink {
                        self.resolve_valid_paths_helper(
                            symlink,
                            paths,
                            symlink_components,
                            current_path_buf,
                            entries,
                        );
                    } else {
                        error!("failed to find entry");
                    }
                }
            }
        }
    }

    /// Inserts a new entry when the path validates the inclusion/exclusion rules.
    ///
    /// Returns `Ok(Some(entry))` pointing to the newly created entry.
    ///
    /// When the path doesn't pass the rules or the path is invalid, it returns `Ok(None)`.
    /// When the file watcher can't be added or the parent dir can not be created, it
    /// returns an `Err`.
    #[instrument(level = "trace", skip_all)]
    fn insert(
        &mut self,
        path: &Path,
        events: &mut SmallVec<[Event; 2]>,
        _entries: &mut EntryMap,
    ) -> FsResult<Option<EntryKey>> {
        // Filter to make sure it passes the rules
        debug!("inserting {:?}", path);

        if !self.passes(path, _entries) {
            // Do not continuously log ignored files
            if let false = self.ignored_dirs.contains(path) {
                self.ignored_dirs.insert(path.to_path_buf());
                info!("ignoring {:?}", path);
            }
            return Ok(None);
        }

        // If we already know about it warn and return
        if self.watch_descriptors.get(path).is_some() {
            #[cfg(any(target_os = "windows", target_os = "linux"))] // bug in notify-rs where there is always a create event before most other events, just ignore.
            warn!("watch descriptor for {} already exists...", path.display());
            return Ok(None);
        }

        let new_key = match FsEntry::try_from(path) {
            Ok(FsEntry::File { ref path, inode }) => {
                trace!("inserting file {}", path.display());

                self.wd_by_inode.insert(inode, path.into());
                let offsets = self.get_initial_offset(path, inode.into());

                let new_entry = Entry::File {
                    path: path.into(),
                    data: RefCell::new(
                        TailedFile::new(path, offsets, Some(self.resume_events_send.clone()))
                            .map_err(Error::File)?,
                    ),
                };
                Metrics::fs().increment_tracked_files();
                let new_key = self.register_as_child(new_entry, _entries)?;
                trace!("registered watcher for file {:#?}", path);
                events.push(Event::New(new_key));
                new_key
            }
            Ok(FsEntry::Dir { ref path }) => {
                let contents =
                    fs::read_dir(path).map_err(|e| Error::DirectoryListNotValid(e, path.into()))?;
                // Insert the parent directory first
                trace!("inserting directory {}", path.display());

                let new_entry = Entry::Dir {
                    children: Default::default(),
                    path: path.into(),
                };

                // We use non-recursive watches and scan children manually
                // to have the same behavior across all platforms
                let new_key = self.register_as_child(new_entry, _entries)?;
                trace!("registered watcher for directory {:#?}", path);
                events.push(Event::New(new_key));

                for dir_entry in contents.flatten() {
                    if let Err(e) = self.insert(&dir_entry.path(), events, _entries) {
                        match e {
                            Error::File(err) => {
                                warn!(
                                    "error found when inserting child entry for {:?}: {}",
                                    path, err
                                );
                                if err.to_string().contains("(os error 24)") {
                                    error!(
                            "Agent process has hit the upper limit of files it's allowed to open"
                        );
                                    std::process::exit(24);
                                }
                            }
                            _ => {
                                info!(
                                    "error found when inserting child entry for {:?}: {:?}",
                                    path, e
                                );
                            }
                        }
                    }
                }
                new_key
            }
            Ok(FsEntry::Symlink { ref path, target }) => {
                // TODO: Handle self

                // Handle Target
                if let Some(ref target) = target {
                    trace!(
                        "inserting symlink {:?} with target {:?}",
                        path.display(),
                        target.display()
                    )
                } else {
                    trace!("inserting broken symlink {:?}", path);
                }

                let new_entry = Entry::Symlink {
                    link: target.clone(),
                    path: path.into(),
                };

                // Ensure the symlink's parent directory is being tracked
                if let Some(parent) = path.parent() {
                    // Manually insert the parent directory for symlink target so that we receive deletes if it's not here
                    if self.watch_descriptors.get(parent).is_none() {
                        let new_entry = Entry::Dir {
                            children: Children::default(),
                            path: parent.into(),
                        };
                        let new_key = _entries.insert(new_entry);
                        self.register(new_key, _entries)?;
                    }
                }

                // REGISTER BUT DO NOT WATCH SYMLINKS
                let new_key = self.register_as_child(new_entry, _entries)?;
                trace!("registered symlink as child of {:#?}", path.parent());
                events.push(Event::New(new_key));

                // Recursively insert the link target
                if let Some(ref target) = target {
                    match FsEntry::try_from(target.as_path())? {
                        FsEntry::Symlink { path, .. }
                        | FsEntry::File { path, .. }
                        | FsEntry::Dir { path, .. } => {
                            // Add the parent directory of the target
                            if let Some(parent) = target.parent() {
                                // Manually insert the parent directory for symlink target so that we receive deletes if it's not here
                                if self.watch_descriptors.get(parent).is_none() {
                                    let new_entry = Entry::Dir {
                                        children: Children::default(),
                                        path: parent.into(),
                                    };
                                    let new_key = _entries.insert(new_entry);
                                    self.register(new_key, _entries)?;
                                }
                            }

                            // FIXME: Insert the target as a symlink target

                            match self.insert(target, events, _entries) {
                                Err(e) => {
                                    // The insert of the target failed, the changes to the symlink itself
                                    // are going to be tracked, continue
                                    warn!(
                                        "insert target {} of symlink {} resulted in error {:?}",
                                        target.display(),
                                        path.display(),
                                        e
                                    );
                                }
                                Ok(None) => {
                                    // The insert of the target failed, the changes to the symlink itself
                                    // are going to be tracked, continue
                                    warn!(
                                        "target {} of symlink {} could not be added",
                                        target.display(),
                                        path.display()
                                    );
                                }
                                _ => {}
                            }
                        }
                    }
                }
                new_key
            }
            Err(e) => {
                rate_limit_macro::rate_limit!(rate = 1, interval = 30, {
                    warn!("Error tracking path: {}", e);
                });
                return Ok(None);
            }
        };
        Ok(Some(new_key))
    }

    fn register(&mut self, entry_key: EntryKey, _entries: &mut EntryMap) -> FsResult<()> {
        let entry = _entries.get(entry_key).ok_or(Error::Lookup)?;
        let path = entry.path();

        if let Entry::Symlink { link, .. } = entry.deref() {
            if let Some(link) = link {
                self.symlinks
                    .entry(link.clone())
                    .or_insert_with(Vec::new)
                    .push(entry_key);
            }
        } else {
            self.watcher
                .watch(path, RecursiveMode::NonRecursive)
                .map_err(|e| Error::Watch(Some(path.to_path_buf()), e))?;
        }
        info!("watching {:?}", path);
        self.watch_descriptors
            .entry(path.to_path_buf())
            .or_insert_with(Vec::new)
            .push(entry_key);

        Ok(())
    }

    /// Removes the entry reference from watch_descriptors and symlinks
    #[instrument(level = "debug", skip_all)]
    fn unregister(&mut self, entry_key: EntryKey, _entries: &mut EntryMap) {
        let entry = match _entries.get(entry_key) {
            Some(v) => v,
            None => {
                warn!("failed to find entry to unregister");
                return;
            }
        };

        let path = entry.path().to_path_buf();
        let entries = match self.watch_descriptors.get_mut(&path) {
            Some(v) => v,
            None => {
                warn!("attempted to remove untracked watch descriptor {:?}", path);
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

        if let Entry::Symlink {
            link: Some(link), ..
        } = entry.deref()
        {
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
        if let Err(e) = self.deletion_ack_sender.try_send(vec![path.clone()]) {
            warn!("unable to notify about deleted path \"{:?}\": {}", &path, e);
        }
    }

    // FIXME: We should not remove dangling symlinks nor the parent dir of the missing target
    #[instrument(level = "trace", skip_all)]
    fn remove(
        &mut self,
        path: &Path,
        events: &mut SmallVec<[Event; 2]>,
        _entries: &mut EntryMap,
    ) -> FsResult<()> {
        trace!("removing {:#?}", path);
        let entry_key = self.lookup(path, _entries).ok_or(Error::Lookup)?;
        let parent = path.parent().and_then(|p| self.lookup(p, _entries));

        if let Some(parent) = parent {
            trace!("checking if we need to remove {:#?}", path);
            if let Some((name, child_count)) = _entries
                .get_mut(parent)
                .map(|entry| (entry.path().to_path_buf(), entry.children().iter().count()))
            {
                let parent_passes = self.passes(&name, _entries);
                trace!("parent {:#?} passes? {}", &name, parent_passes);
                if !parent_passes && child_count == 1 {
                    trace!("recursing remove for {:#?}", name);
                    return self.remove(&name, events, _entries);
                }
                if let Some(parent_entry) = _entries.get_mut(parent) {
                    trace!("removing self ({:#?}) from parent", path);
                    parent_entry
                        .children()
                        .ok_or(Error::ParentNotValid)?
                        .remove(&path.as_os_str().to_owned());
                    trace!("removed {:#?} from fs cache", name);
                }
            }
        }
        self.drop_entry(entry_key, events, _entries);

        Ok(())
    }

    /// Emits `Delete` events, entry and its children from
    /// watch descriptors and symlinks.
    #[instrument(level = "trace", skip_all)]
    fn drop_entry(
        &mut self,
        entry_key: EntryKey,
        events: &mut SmallVec<[Event; 2]>,
        _entries: &mut EntryMap,
    ) {
        if let Some(entry) = _entries.get(entry_key) {
            let mut _children = vec![];
            let mut _links = vec![];
            match entry.deref() {
                Entry::Dir { children, .. } => {
                    for child in children.values() {
                        _children.push(*child);
                    }
                    self.unregister(entry_key, _entries);
                    events.push(Event::Delete(entry_key));
                }
                Entry::Symlink { ref link, path } => {
                    trace!("We're removing a symlink, check if we should unwatch it's target");
                    let mut cuts = vec![path.clone()];
                    let non_initial_paths_under = link
                        .as_deref()
                        .map(|link| {
                            walkdir::WalkDir::new(link)
                                .into_iter()
                                .filter_map(|path| {
                                    path.ok().and_then(|path| {
                                        (!self
                                            .initial_dirs
                                            .iter()
                                            .any(|dir| dir.as_ref() == path.path()))
                                        .then(|| path.path().to_path_buf())
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_else(Vec::new);

                    // Clean up self
                    self.unregister(entry_key, _entries);
                    events.push(Event::Delete(entry_key));

                    // Clean up children
                    let unreachable_paths: Vec<DefaultKey> = non_initial_paths_under
                        .iter()
                        .filter_map(|path| {
                            let is_reachable = self.is_reachable(&mut cuts, path, _entries).ok();
                            is_reachable.and_then(|b| (!b).then_some(path))
                        })
                        .filter_map(|path| {
                            let ret = self
                                .watch_descriptors
                                .get(path)
                                .map(|entry_keys| entry_keys.iter());
                            ret
                        })
                        .flatten()
                        .cloned()
                        .collect();
                    for unreachable in unreachable_paths {
                        self.drop_entry(unreachable, events, _entries);
                    }
                }
                Entry::File { ref path, .. } => {
                    Metrics::fs().decrement_tracked_files();
                    if let Some(entries) = self.symlinks.get(path) {
                        for entry in entries {
                            if let Some(entry) = _entries.get(*entry) {
                                if !self.passes(entry.path(), _entries) {
                                    _links.push(entry.path().to_path_buf())
                                }
                            }
                        }
                    }
                    self.unregister(entry_key, _entries);
                    events.push(Event::Delete(entry_key));
                }
            }

            for child in _children {
                trace!("drop entry clearing children");
                self.drop_entry(child, events, _entries);
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
    ) -> FsResult<SmallVec<[Event; 2]>> {
        let new_parent = to_path.parent().and_then(|p| self.lookup(p, _entries));

        let mut events = SmallVec::new();
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
                if let Some(parent) = from_path.parent().and_then(|p| self.lookup(p, _entries)) {
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
    #[instrument(level = "trace", skip_all)]
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

    #[instrument(level = "trace", skip_all)]
    fn is_symlink_target(&self, path: &Path, _entries: &EntryMap) -> bool {
        for (_, symlink_ptrs) in self.symlinks.iter() {
            for symlink_ptr in symlink_ptrs.iter() {
                if let Some(symlink) = _entries.get(*symlink_ptr) {
                    match symlink {
                        Entry::Symlink { link, .. } => {
                            if let Some(link) = link {
                                if link == path {
                                    trace!("Is a symlink target {:?}", path);
                                    return true;
                                }
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
        trace!("Not a symlink target {:?}", path);
        false
    }

    pub(crate) fn passes_rules(&self, path: &Path) -> bool {
        if self.initial_dir_rules.passes(path) != Status::Ok {
            return false;
        }
        if self.master_rules.passes(path) != Status::Ok {
            return false;
        }
        true
    }

    /// Determines whether the path is within the initial dir
    /// and either passes the master rules (e.g. "*.log") or it's a directory
    #[instrument(level = "debug", skip_all)]
    pub(crate) fn is_initial_dir_target(&self, path: &Path) -> bool {
        // Must be within the initial dir
        if self.initial_dir_rules.passes(path) != Status::Ok {
            return false;
        }
        debug!("{:#?} passes initial dir rules", path);

        // The file should validate the file rules or be a directory
        if self.master_rules.passes(path) != Status::Ok {
            if let Ok(metadata) = std::fs::metadata(path) {
                let is_dir = metadata.is_dir();
                debug!("{:#?} passes master rules too, is_dir? {:#?}", path, is_dir);
                return is_dir;
            }
            return false;
        }
        true
    }

    /// Helper method for checking if a path passes exclusion/inclusion rules
    fn passes(&self, path: &Path, _entries: &EntryMap) -> bool {
        self.is_initial_dir_target(path)
            || self
                .is_reachable(&mut vec![], path, _entries)
                .unwrap_or(false)
    }

    fn entry_path_passes(&self, entry_key: EntryKey, entries: &EntryMap) -> bool {
        entries
            .get(entry_key)
            .map(|e| self.passes(e.path(), entries))
            .unwrap_or(false)
    }

    /// Returns the first entry based on the `WatchDescriptor`, returning an `Err` when not found.
    fn get_first_entry(&self, wd: &Path) -> Option<EntryKey> {
        let entries = self.watch_descriptors.get(wd)?;

        if !entries.is_empty() {
            Some(entries[0])
        } else {
            None
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

use once_cell::sync::OnceCell;
use std::ffi::OsStr;
static ROOT_PATH: OnceCell<OsString> = OnceCell::new();

// Split the path into it's components.
fn into_components(path: &Path) -> SmallVec<[&OsStr; 12]> {
    path.components()
        .filter_map(|c| match c {
            // Split the path into it's components.
            Component::RootDir => {
                let root_path = ROOT_PATH.get_or_init(|| OsString::from("/"));
                Some(root_path.as_os_str())
            }
            Component::Normal(path) => Some(path),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
#[cfg(any(target_os = "windows", target_os = "linux"))]
mod tests {
    use super::*;
    use crate::rule::{RuleDef, Rules};
    use pin_utils::pin_mut;
    use std::convert::TryInto;
    use std::fs::{copy, create_dir, hard_link, remove_dir_all, remove_file, rename, File};
    #[cfg(not(windows))]
    use std::io::Write;
    use std::{io, panic};
    use tempfile::{tempdir, TempDir};

    #[cfg(windows)]
    use std::sync::mpsc;

    static DELAY: Duration = Duration::from_millis(200);

    macro_rules! take_events {
        ($x: expr) => {{
            let mut events = Vec::new();
            tokio::time::sleep(DELAY * 2).await;
            tokio::time::sleep(Duration::from_millis(20)).await;
            let stream = FileSystem::stream_events($x.clone()).unwrap();
            pin_mut!(stream);
            loop {
                tokio::select! {
                    e = stream.next() => { events.push(e) }
                    _ = tokio::time::sleep(Duration::from_millis(100)) => {
                        break;
                    }
                }
            }
            events
        }};
    }

    macro_rules! lookup {
        ( $x:expr, $y: expr ) => {{
            let fs = $x.borrow();
            fs.watch_descriptors
                .get(&$y)
                .map(|entry_keys| entry_keys[0])
        }};
    }

    macro_rules! lookup_entry_path {
        ( $x:expr, $y: expr ) => {{
            let fs = $x.borrow();
            let entries = fs.entries.borrow();
            entries.get($y).as_ref().unwrap().path().to_path_buf()
        }};
    }

    macro_rules! assert_is_file {
        ( $x:expr, $y: expr ) => {
            assert!($y.is_some());
            {
                let fs = $x.borrow();
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

    fn new_fs(path: PathBuf, rules: Option<Rules>) -> Rc<RefCell<FileSystem>> {
        let rules = rules.unwrap_or_else(|| {
            let mut rules = Rules::new();
            rules.add_inclusion(RuleDef::glob_rule(r"**").unwrap());
            rules
        });
        Rc::new(RefCell::new(FileSystem::new(
            vec![path
                .as_path()
                .try_into()
                .unwrap_or_else(|_| panic!("{:?} is not a directory!", path))],
            Lookback::Start,
            HashMap::new(),
            rules,
            None,
            async_channel::unbounded().0,
        )))
    }

    fn create_fs(path: &Path) -> Rc<RefCell<FileSystem>> {
        new_fs(path.to_path_buf(), None)
    }

    #[tokio::test]
    async fn filesystem_init_test() {
        let temp_dir = tempdir().unwrap();
        let dir = temp_dir.path();

        let file_path = dir.join("a.log");
        File::create(&file_path).unwrap();

        let fs = create_fs(dir);

        let _ = take_events!(fs);

        let entry_key = lookup!(fs, file_path);
        assert_is_file!(fs, entry_key);
    }

    // Simulates the `create_move` log rotation strategy
    #[tokio::test]
    #[cfg(any(target_os = "windows", target_os = "linux"))]
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
    #[cfg(unix)]
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

    #[tokio::test]
    #[cfg(windows)]
    async fn filesystem_rotate_create_copy_win() -> io::Result<()> {
        let tempdir = TempDir::new()?;
        let path = tempdir.path().to_path_buf();
        let fs = create_fs(&path);

        // Copy and remove
        let a = path.join("a");
        let old = path.join("a.old");

        let file_handle = std::thread::spawn({
            let a_file = a.clone();
            let old_file = old.clone();
            move || {
                File::create(&a_file).unwrap();
                assert!(&a_file.is_file());
                copy(&a_file, &old_file).unwrap();
                remove_file(&a_file).unwrap();
                assert!(!&a_file.is_file());
            }
        });
        file_handle.join().unwrap();

        take_events!(fs);
        take_events!(fs);

        take_events!(fs);
        let entry_key = lookup!(fs, a);
        assert!(entry_key.is_none());
        let entry_key = lookup!(fs, old);
        assert_is_file!(fs, entry_key);

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

        let fs = fs.borrow();
        let entries = fs.entries.borrow();
        assert!(matches!(
            entries.get(entry_key.unwrap()).unwrap().deref(),
            Entry::Dir { .. }
        ));
    }

    /// Creates a dir w/ dots and a file after initialization
    #[tokio::test]
    async fn filesystem_create_dir_after_init() -> io::Result<()> {
        let _ = env_logger::Builder::from_default_env().try_init();
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
    #[cfg(unix)]
    #[tokio::test]
    async fn filesystem_create_symlink_directory() -> io::Result<()> {
        let _ = env_logger::Builder::from_default_env().try_init();
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
        assert!(entry_key_symlink.is_some());
        let _fs = fs.borrow();
        let _entries = &_fs.entries;
        let _entries = _entries.borrow();
        match _entries.get(entry_key_dir.unwrap()).unwrap().deref() {
            Entry::Dir { .. } => {}
            _ => panic!("wrong entry type"),
        };

        assert!(matches!(
            _entries.get(entry_key_symlink.unwrap()).unwrap().deref(),
            Entry::Symlink { .. }
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
    #[cfg(target_os = "linux")]
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
    #[cfg(unix)]
    async fn filesystem_delete_nested_filled_dir() -> io::Result<()> {
        let _ = env_logger::Builder::from_default_env().try_init();
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
    #[cfg(unix)]
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

    #[tokio::test]
    #[cfg(windows)]
    async fn filesystem_delete_file_win() -> io::Result<()> {
        let (tx_main, rx_main) = mpsc::channel();
        let (tx_thread, rx_thread) = mpsc::channel();

        let _ = env_logger::Builder::from_default_env().try_init();
        let tempdir = TempDir::new()?;
        let path = tempdir.path().to_path_buf();
        let fs = create_fs(&path);
        let file_path = path.join("file.log");

        let file_handle = std::thread::spawn({
            let file = file_path.clone();
            move || {
                File::create(&file).unwrap();
                assert!(&file.is_file());
                tx_thread.send(true).unwrap();
                rx_main.recv().unwrap();
                remove_file(&file).unwrap();
                assert!(!file.is_file());
            }
        });
        rx_thread.recv().unwrap();

        tx_main.send(true).unwrap();
        file_handle.join().unwrap();

        take_events!(fs);
        take_events!(fs);
        take_events!(fs);

        assert!(lookup!(fs, file_path).is_none());

        Ok(())
    }

    // Deletes a symlink
    #[tokio::test]
    #[cfg(unix)]
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
    #[ignore]
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
    #[ignore]
    #[tokio::test]
    async fn filesystem_delete_symlink_pointee() -> io::Result<()> {
        let _ = env_logger::Builder::from_default_env().try_init();
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

    // Deletes the pointee of a symlink in untracked dir
    #[ignore]
    #[tokio::test]
    async fn filesystem_delete_untracked_symlink_pointee() -> io::Result<()> {
        let _ = env_logger::Builder::from_default_env().try_init();
        let tempdir = TempDir::new()?;
        let tempdir2 = TempDir::new()?;
        let path = tempdir.path().to_path_buf();
        let path2 = tempdir2.path().to_path_buf();

        let a = path.join("a");
        let b = path2.join("b");
        File::create(&b)?;
        symlink_file(&b, &a)?;

        let fs = create_fs(&path);
        take_events!(fs);

        remove_file(&b)?;
        let events = take_events!(fs);

        assert_eq!(
            events.len(),
            1,
            "events: {:#?}",
            events
                .into_iter()
                .map({
                    let fs = fs.clone();
                    move |e| {
                        (
                            lookup_entry_path!(fs, e.as_ref().unwrap().as_ref().unwrap().key()),
                            e,
                        )
                    }
                })
                .collect::<Vec<_>>()
        );

        assert!(lookup!(fs, b).is_none());

        Ok(())
    }

    // Deletes a hardlink
    #[tokio::test]
    #[cfg(unix)]
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

    #[tokio::test]
    #[cfg(windows)]
    async fn filesystem_delete_hardlink_win() -> io::Result<()> {
        let (tx_main, rx_main) = mpsc::channel();
        let (tx_thread, rx_thread) = mpsc::channel();

        let tempdir = TempDir::new()?;
        let path = tempdir.path().to_path_buf();
        let fs = create_fs(&path);

        // Copy and remove
        let a = path.join("a");
        let b = path.join("b");

        let file_handle = std::thread::spawn({
            let a_file = a.clone();
            let b_file = b.clone();
            move || {
                File::create(&a_file).unwrap();
                hard_link(&a_file, &b_file).unwrap();
                assert!(&a_file.is_file());
                assert!(&b_file.is_file());
                tx_thread.send(true).unwrap();
                rx_main.recv().unwrap();
                remove_file(&b_file).unwrap();
                assert!(!b_file.is_file());
            }
        });
        rx_thread.recv().unwrap();

        tx_main.send(true).unwrap();
        file_handle.join().unwrap();

        take_events!(fs);
        assert!(lookup!(fs, a).is_some());
        assert!(lookup!(fs, b).is_none());

        Ok(())
    }

    // Deletes the pointee of a hardlink (not totally accurate since we're not deleting the inode
    // entry, but what evs)
    #[tokio::test]
    #[cfg(unix)]
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

    #[tokio::test]
    #[cfg(windows)]
    async fn filesystem_delete_hardlink_pointee_win() -> io::Result<()> {
        let (tx_main, rx_main) = mpsc::channel();
        let (tx_thread, rx_thread) = mpsc::channel();

        let tempdir = TempDir::new()?;
        let path = tempdir.path().to_path_buf();
        let fs = create_fs(&path);

        let a = path.join("a");
        let b = path.join("b");
        let file_handle = std::thread::spawn({
            let a_file = a.clone();
            let b_file = b.clone();
            move || {
                File::create(&a_file).unwrap();
                hard_link(&a_file, &b_file).unwrap();
                assert!(&a_file.is_file());
                assert!(&b_file.is_file());
                tx_thread.send(true).unwrap();
                rx_main.recv().unwrap();
                remove_file(&a_file).unwrap();
                assert!(!a_file.is_file());
            }
        });
        rx_thread.recv().unwrap();

        tx_main.send(true).unwrap();
        file_handle.join().unwrap();

        take_events!(fs);
        assert!(lookup!(fs, a).is_none());
        assert!(lookup!(fs, b).is_some());

        Ok(())
    }

    /// Moves a directory within the watched directory
    ///
    /// Only run on unix-like systems as moving a directory on Windows with file handles open is
    /// not supported.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn filesystem_move_dir_internal() -> io::Result<()> {
        let _ = env_logger::Builder::from_default_env().try_init();
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

        tracing::debug!(
            "new dir contents: {:#?}",
            fs::read_dir(&new_dir_path).unwrap().collect::<Vec<_>>()
        );
        let entry = lookup!(fs, new_dir_path);
        assert!(entry.is_some());

        let entry2 = lookup!(fs, new_dir_path.join("file.log"));
        assert!(entry2.is_some());

        let entry3 = lookup!(fs, new_dir_path.join("hard.log"));
        assert!(entry3.is_some());

        let entry4 = lookup!(fs, new_dir_path.join("sym.log"));
        assert!(entry4.is_some());

        let _fs = fs.borrow();
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
            Entry::Symlink { .. } => {}
            _ => panic!("wrong entry type"),
        };

        Ok(())
    }

    /// Moves a directory out
    ///
    /// Only run on unix-like systems as moving a directory on Windows with file handles open is
    /// not supported.
    #[cfg(target_os = "linux")]
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

        let fs = new_fs(old_dir_path.clone(), None);

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
    #[cfg(target_os = "linux")]
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

        let fs = new_fs(new_path, None);

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

        let _fs = fs.borrow();
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
            Entry::Symlink { .. } => {}
            _ => panic!("wrong entry type"),
        };

        Ok(())
    }

    // Moves a file within the watched directory
    #[tokio::test]
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    async fn filesystem_move_file_internal() -> io::Result<()> {
        let tempdir = TempDir::new()?;
        let path = tempdir.path().to_path_buf();

        let fs = new_fs(path.clone(), None);

        let file_path = path.join("insert.log");
        let new_path = path.join("new.log");
        File::create(file_path.clone())?;
        rename(&file_path, &new_path)?;

        take_events!(fs);

        let entry = lookup!(fs, file_path);
        assert!(entry.is_none());

        let entry = lookup!(fs, new_path);
        assert!(entry.is_some());
        let _fs = fs.borrow();
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
    #[cfg(any(target_os = "windows", target_os = "linux"))]
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

        let fs = new_fs(watch_path, None);

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
    #[cfg(any(target_os = "windows", target_os = "linux"))]
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

        let fs = new_fs(watch_path, None);

        rename(&file_path, &move_path)?;
        File::create(file_path.clone())?;

        take_events!(fs);

        let entry = lookup!(fs, file_path);
        assert!(entry.is_none());

        let entry = lookup!(fs, move_path);
        assert!(entry.is_some());
        let _fs = fs.borrow();
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
    #[ignore]
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

        let fs = new_fs(watch_path, None);

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
    #[ignore]
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

        let fs = new_fs(path, Some(rules));

        let entry = lookup!(fs, file_path);
        assert!(entry.is_none());

        symlink_file(&file_path, &sym_path)?;

        take_events!(fs);
        let entry = lookup!(fs, sym_path);
        assert!(entry.is_some());
        Ok(())
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn filesystem_test_basic_ops_per_platform() -> io::Result<()> {
        let _ = env_logger::Builder::from_default_env().try_init();
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

        let fs = new_fs(path, None);

        let entry = lookup!(fs, file1_path);
        assert!(entry.is_some());

        take_events!(fs);

        writeln!(file1, "hello")?;
        let events = take_events!(fs);
        assert_eq!(
            events.len(),
            1, // v5 of notify-rs throws 2 write events debounced to 1
            "events: {:#?}",
            events
                .into_iter()
                .map({
                    let fs = fs.clone();
                    move |e| {
                        (
                            lookup_entry_path!(fs, e.as_ref().unwrap().as_ref().unwrap().key()),
                            e,
                        )
                    }
                })
                .collect::<Vec<_>>()
        );

        writeln!(file2, "hello")?;
        let events = take_events!(fs);
        assert_eq!(events.len(), 1, "events: {:#?}", events);

        writeln!(file3, "hello")?;
        let events = take_events!(fs);
        assert_eq!(events.len(), 1, "events: {:#?}", events);

        drop(file1);
        drop(file2);
        drop(file3);

        remove_file(&file1_path).unwrap();
        take_events!(fs);
        let entry = lookup!(fs, file1_path);
        assert!(entry.is_none());

        remove_file(&sym_path).unwrap();

        // Move file out of directory
        rename(&file2_path, tempdir2.path().join("another_file.log")).unwrap();

        // Remove dir with contents (intermittently fails on windows)
        #[cfg(unix)]
        remove_dir_all(&sub_dir_path).unwrap();

        Ok(())
    }
}

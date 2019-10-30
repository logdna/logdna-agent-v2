use std::ffi::{OsStr, OsString};
use std::fs::{canonicalize, read_dir};
use std::io;
use std::path::PathBuf;

use hashbrown::HashMap;
use inotify::{Event as InotifyEvent, EventMask, Inotify, WatchDescriptor, WatchMask};

use crate::error::WatchError;
use crate::rule::{Rule, Rules, Status};
use crate::Event;
use metrics::Metrics;
use std::mem::replace;

//todo provide examples and some extra tid bits around operational behavior
/// Used to watch the filesystem for [Events](../enum.Event.html)
///
/// Also has support for exclusion and inclusion rules to narrow the scope of watched files/directories
pub struct Watcher {
    // An instance of inotify
    inotify: Inotify,
    // A mapping of watch descriptors to paths
    // This is required because inotify operates on a watch list which (a list of i64s)
    // This provides a mapping of those ids to the corresponding paths
    // The invariant that is relied on here is that is mapping is always correct
    // The main mechanism for breaking this invariant is overflowing the kernel queue (Q_OVERFLOW)
    watch_descriptors: HashMap<WatchDescriptor, PathBuf>,
    // A list of inclusion and exclusion rules
    rules: Rules,
    // The list of dirs to watch on startup, e.g /var/log/
    // These dirs will be watched recursively
    // So if /var/log/ is in this list, /var/log/httpd/ is redundant
    initial_dirs: Vec<PathBuf>,
    initial_events: Vec<Event>,
}

impl Watcher {
    /// Creates an instance of WatchBuilder
    pub fn builder() -> WatchBuilder {
        WatchBuilder {
            initial_dirs: Vec::new(),
            rules: Rules::new(),
        }
    }

    pub fn init(&mut self) {
        // iterate over all initial dirs and add them to the watcher
        // replace is need because it takes owner ship of the initial_dirs field without consuming self
        for dir in self.initial_dirs.clone() {
            // if the watch was successful a list of watched paths will be returned
            // we only create Initiate events for files
            // the events get sent upstream through sender
            match self.watch(&dir) {
                Ok(paths) => paths
                    .into_iter()
                    .filter(|p| p.is_file())
                    .for_each(|p| self.initial_events.push(Event::Initiate(p))),
                Err(e) => error!("error initializing root path {:?}: {:?}", dir, e),
            }
        }
    }

    pub fn read_events(&mut self) -> Vec<Event> {
        if !self.initial_events.is_empty() {
            return replace(&mut self.initial_events, Vec::new());
        }

        let mut buf = [0u8; 4096];
        let events = match self.inotify.read_events(&mut buf) {
            Ok(events) => events,
            Err(e) => {
                error!("error reading from inotify fd: {}", e);
                return Vec::new();
            }
        };

        events
            .into_iter()
            .map(|e| self.process(e))
            .flatten()
            .collect()
    }
    // handles inotify events and may produce Event(s) that are return upstream through sender
    fn process(&mut self, event: InotifyEvent<&OsStr>) -> Vec<Event> {
        Metrics::fs().increment_events();
        let mut new_events = Vec::new();

        if event.mask.contains(EventMask::CREATE) {
            let empty = OsString::new();
            let name = event.name.unwrap_or(&empty);
            self.watch_descriptors
                .get(&event.wd)
                .map(|p| p.join(name))
                .and_then(|path| self.watch(&path).ok())
                .map(|paths| {
                    paths.into_iter().filter(|p| p.is_file()).for_each(|p| {
                        new_events.push(Event::New(p));
                    })
                });
        }

        if event.mask.contains(EventMask::MODIFY) {
            self.watch_descriptors
                .get(&event.wd)
                .map(|path| new_events.push(Event::Write(path.clone())));
        }

        if event.mask.contains(EventMask::DELETE_SELF) {
            self.watch_descriptors.remove(&event.wd).map(|path| {
                Some(info!(
                    "removed {:?} from watcher ({})",
                    path,
                    self.watch_descriptors.len()
                ));
                new_events.push(Event::Delete(path));
            });
        }

        if event.mask.contains(EventMask::Q_OVERFLOW) {
            panic!("overflowed kernel queue!")
        }

        new_events
    }

    /// Used to watch a file or directory, in the case it's a directory it is recursively scanned
    ///
    /// This scan has an unlimited depth, so watching /var/log/ will capture all the root and all children
    pub fn watch<P: Into<PathBuf>>(&mut self, path: P) -> Result<Vec<PathBuf>, WatchError> {
        let mut paths = Vec::new();
        let path = canonicalize(path.into())?;
        // paths needs to be valid utf8
        let path_str = path
            .to_str()
            .ok_or_else(|| WatchError::PathNonUtf8(path.clone()))?;
        // if the path is a dir we need to scan it recursively
        if path.is_dir() {
            recursive_scan(&path)
                .into_iter()
                // for each event map path -> path, path_str
                .filter_map(|p| p.to_str().map(|s| (p.clone(), s.to_string())))
                .for_each(|(p, s)| {
                    // we only apply exclusion/inclusion rules to files
                    if p.is_dir() || self.path_is_ok(&s) {
                        // if the path is added to inotify successfully
                        // we push it onto the paths vec to be return upstream
                        match self.add(&p) {
                            Ok(..) => paths.push(p),
                            Err(WatchError::Duplicate) => {}
                            Err(e) => error!("error adding {:?} to watcher: {:?}", p, e),
                        }
                    }
                })
        } else {
            // in this case we are watching a file
            // check that is passes our inclusion/exclusion rules and push it
            if self.path_is_ok(path_str) {
                match self.add(&path) {
                    Ok(..) => paths.push(path),
                    Err(WatchError::Duplicate) => {}
                    Err(e) => {
                        return Err(e);
                    }
                }
            }
        }

        Ok(paths)
    }

    // adds path to inotify and watch descriptor map
    fn add(&mut self, path: &PathBuf) -> Result<(), WatchError> {
        // make sure that the path passed in is not a symlink
        let path = canonicalize(path.clone())?;
        // add the path to the inotify with the appropriate mask
        let watch_descriptor = self.inotify.add_watch(&path, watch_mask(&path))?;
        if self.watch_descriptors.contains_key(&watch_descriptor) {
            return Err(WatchError::Duplicate);
        }
        // add the watch descriptor to the map so we can resolve the path later
        info!(
            "added {:?} to watcher ({})",
            path,
            self.watch_descriptors.len()
        );
        self.watch_descriptors.insert(watch_descriptor, path);
        Ok(())
    }
    // a helper for checking if a path passes exclusion/inclusion rules
    fn path_is_ok(&self, path: &str) -> bool {
        match self.rules.passes(path) {
            Status::Ok => true,
            Status::NotIncluded => {
                info!("{} was not included!", path);
                false
            }
            Status::Excluded => {
                info!("{} was excluded!", path);
                false
            }
        }
    }
}

// returns the watch mask depending on if a path is a file or dir
fn watch_mask(path: &PathBuf) -> WatchMask {
    if path.is_file() {
        WatchMask::MODIFY | WatchMask::DELETE_SELF
    } else {
        WatchMask::CREATE | WatchMask::DELETE_SELF
    }
}

// recursively scans a directory for unlimited depth
fn recursive_scan(path: &PathBuf) -> Vec<PathBuf> {
    let path = match canonicalize(path.clone()) {
        Ok(v) => v,
        Err(_) => {
            return Vec::new();
        }
    };

    let mut paths = vec![path.clone()];

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
        let path = match tmp_path.and_then(|p| canonicalize(p.path())) {
            Ok(v) => v,
            Err(e) => {
                error!("failed reading {:?}: {:?}", path, e);
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

/// Creates an instance of a Watcher
pub struct WatchBuilder {
    initial_dirs: Vec<PathBuf>,
    rules: Rules,
}

impl WatchBuilder {
    /// Add a dir to the list of initial dirs
    pub fn add<T: Into<PathBuf>>(mut self, path: T) -> Self {
        self.initial_dirs.push(path.into());
        self
    }
    /// Add a multiple dirs to the list of initial dirs
    pub fn add_all<T: AsRef<[PathBuf]>>(mut self, path: T) -> Self {
        self.initial_dirs.extend_from_slice(path.as_ref());
        self
    }
    /// Adds an inclusion rule
    pub fn include<T: Rule + Send + 'static>(mut self, rule: T) -> Self {
        self.rules.add_inclusion(rule);
        self
    }
    /// Adds an exclusion rule
    pub fn exclude<T: Rule + Send + 'static>(mut self, rule: T) -> Self {
        self.rules.add_exclusion(rule);
        self
    }
    /// Appends all rules from another instance of rules
    pub fn append_all<T: Into<Rules>>(mut self, rules: T) -> Self {
        self.rules.add_all(rules);
        self
    }
    /// Consumes the builder and produces an instance of the watcher
    pub fn build(self) -> Result<Watcher, io::Error> {
        Ok(Watcher {
            inotify: Inotify::init()?,
            watch_descriptors: HashMap::new(),
            rules: self.rules,
            initial_dirs: self.initial_dirs,
            initial_events: Vec::new(),
        })
    }
}

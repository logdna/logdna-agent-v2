use crate::cache::entry::Entry;
use crate::cache::event::Event;
use crate::cache::watch::{WatchEvent, Watcher};
use crate::rule::{GlobRule, Rules, Status};

use std::cell::RefCell;
use std::ffi::OsString;
use std::fmt;
use std::fs::read_dir;
use std::fs::OpenOptions;
use std::iter::FromIterator;
use std::ops::Deref;
use std::path::{Component, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use futures::{Stream, StreamExt};
use hashbrown::hash_map::Entry as HashMapEntry;
use hashbrown::HashMap;
use inotify::WatchDescriptor;
use metrics::Metrics;
use slotmap::{DefaultKey, SlotMap};

pub mod dir_path;
pub mod entry;
pub mod event;
pub use dir_path::{DirPathBuf, DirPathBufError};

mod watch;

type Children = HashMap<OsString, EntryKey>;
type Symlinks = HashMap<PathBuf, Vec<EntryKey>>;
type WatchDescriptors = HashMap<WatchDescriptor, Vec<EntryKey>>;

pub type EntryKey = DefaultKey;

type EntryMap<T> = SlotMap<EntryKey, RefCell<entry::Entry<T>>>;
type FsResult<T> = Result<T, String>;

pub struct FileSystem<T>
where
    T: Clone + std::fmt::Debug,
{
    watcher: Watcher,
    pub entries: Rc<RefCell<EntryMap<T>>>,
    root: EntryKey,

    symlinks: Symlinks,
    watch_descriptors: WatchDescriptors,

    master_rules: Rules,
    initial_dir_rules: Rules,

    initial_events: Vec<Event>,
}

impl<'a, T: 'a + Default> FileSystem<T>
where
    T: Clone + std::fmt::Debug,
{
    pub fn new(initial_dirs: Vec<DirPathBuf>, rules: Rules) -> Self {
        initial_dirs.iter().for_each(|path| {
            if !path.is_dir() {
                panic!("initial dirs must be dirs")
            }
        });
        let mut watcher = Watcher::new().expect("unable to initialize inotify");

        let mut entries = SlotMap::new();
        let root = entries.insert(RefCell::new(Entry::Dir {
            name: "/".into(),
            parent: None,
            children: Children::new(),
            wd: watcher.watch("/").expect("unable to watch /"),
        }));

        let mut initial_dir_rules = Rules::new();
        for path in initial_dirs.iter() {
            append_rules(&mut initial_dir_rules, path.as_ref().into());
        }

        let mut fs = Self {
            entries: Rc::new(RefCell::new(entries)),
            root,
            symlinks: Symlinks::new(),
            watch_descriptors: WatchDescriptors::new(),
            master_rules: rules,
            initial_dir_rules,
            watcher,
            initial_events: Vec::new(),
        };

        let entries = fs.entries.clone();
        let mut entries = entries.borrow_mut();
        fs.register(fs.root, &mut entries)
            .expect("Unable to register root");

        for dir in initial_dirs
            .into_iter()
            .map(|path| -> PathBuf { path.into() })
        {
            let mut path_cpy: PathBuf = dir.clone();
            loop {
                if !path_cpy.exists() {
                    path_cpy.pop();
                } else {
                    if let Err(e) = fs.insert(&path_cpy, &mut Vec::new(), &mut entries) {
                        // It can failed due to permissions or some other restriction
                        debug!(
                            "Initial insertion of {} failed: {}",
                            path_cpy.to_str().unwrap(),
                            e
                        );
                    }
                    break;
                }
            }

            for path in recursive_scan(&dir) {
                let mut events = Vec::new();
                if let Err(e) = fs.insert(&path, &mut events, &mut entries) {
                    // It can failed due to file restrictions
                    debug!(
                        "Initial recursive scan insertion of {} failed: {}",
                        path.to_str().unwrap(),
                        e
                    );
                } else {
                    for event in events {
                        match event {
                            Event::New(entry) => fs.initial_events.push(Event::Initialize(entry)),
                            _ => panic!("unexpected event in initialization"),
                        };
                    }
                }
            }
        }

        fs
    }

    pub fn stream_events(
        fs: Arc<Mutex<FileSystem<T>>>,
        buf: &'a mut [u8],
    ) -> Result<impl Stream<Item = Event> + 'a, std::io::Error> {
        let events_stream = {
            match fs
                .try_lock()
                .expect("could not lock filesystem cache")
                .watcher
                .event_stream(buf)
            {
                Ok(events) => events,
                Err(e) => {
                    error!("error reading from watcher: {}", e);
                    return Err(e);
                }
            }
        };

        let initial_events = {
            let mut fs = fs.try_lock().expect("could not lock filesystem cache");

            let mut acc = Vec::new();
            if !fs.initial_events.is_empty() {
                for event in std::mem::replace(&mut fs.initial_events, Vec::new()) {
                    acc.push(event)
                }
            }
            acc
        };

        let events = events_stream.into_stream().map(move |event| {
            let fs = fs.clone();
            {
                let mut acc = Vec::new();

                match event {
                    Ok(event) => {
                        fs.try_lock()
                            .expect("couldn't lock filesystem cache")
                            .process(event, &mut acc);
                        futures::stream::iter(acc)
                    }
                    _ => panic!("Inotify error"),
                }
            }
        });

        Ok(futures::stream::iter(initial_events).chain(events.flatten()))
    }

    /// Handles inotify events and may produce Event(s) that are returned upstream through sender
    fn process(&mut self, watch_event: WatchEvent, events: &mut Vec<Event>) {
        let _entries = self.entries.clone();
        let mut _entries = _entries.borrow_mut();
        Metrics::fs().increment_events();

        debug!("handling inotify event {:#?}", watch_event);

        let result = match watch_event {
            WatchEvent::Create { wd, name } | WatchEvent::MovedTo { wd, name, .. } => {
                self.process_create(&wd, name, events, &mut _entries)
            }
            WatchEvent::Modify { wd } => self.process_modify(&wd, events),
            WatchEvent::Delete { wd, name } | WatchEvent::MovedFrom { wd, name, .. } => {
                self.process_delete(&wd, name, events, &mut _entries)
            }
            WatchEvent::Move {
                from_wd,
                from_name,
                to_wd,
                to_name,
            } => {
                // directories can't have hard links so we can expect just one entry for these watch
                // descriptors
                let is_from_path_ok = self
                    .get_first_entry(&from_wd)
                    .map(|entry| self.entry_path_passes(entry, from_name.clone(), &_entries))
                    .unwrap_or(false);

                let is_to_path_ok = self
                    .get_first_entry(&to_wd)
                    .map(|entry| self.entry_path_passes(entry, to_name.clone(), &_entries))
                    .unwrap_or(false);

                if is_to_path_ok && is_from_path_ok {
                    self.process_move(&from_wd, from_name, &to_wd, to_name, events, &mut _entries)
                } else if is_to_path_ok {
                    self.process_create(&to_wd, to_name, events, &mut _entries)
                } else if is_from_path_ok {
                    self.process_delete(&from_wd, from_name, events, &mut _entries)
                } else {
                    Err("Moving paths are invalid".into())
                }
            }
            WatchEvent::Overflow => Err(
                // Files are being updated too often for inotify to catch up
                "the inotify event queue has overflowed and events have presumably been lost"
                    .into(),
            ),
        };

        if let Err(e) = result {
            warn!("Processing inotify event resulted in error: {}", e);
        }
    }

    fn process_create(
        &mut self,
        watch_descriptor: &WatchDescriptor,
        name: OsString,
        events: &mut Vec<Event>,
        _entries: &mut EntryMap<T>,
    ) -> FsResult<()> {
        // directories can't be a hard link so we're guaranteed the watch descriptor maps to one
        // entry
        let entry_key = self.get_first_entry(watch_descriptor)?;
        let entry = _entries.get(entry_key).ok_or("failed to find entry")?;
        let mut path = self.resolve_direct_path(&entry.borrow(), _entries);
        path.push(name);

        if let Some(new_entry) = self.insert(&path, events, _entries)? {
            let mut errors = vec![];
            if matches!(_entries.get(new_entry).map(|n_e| n_e.borrow()), Some(_)) {
                for new_path in recursive_scan(&path) {
                    if let Err(e) = self.insert(&new_path, events, _entries) {
                        errors.push(e);
                    }
                }
            };

            if !errors.is_empty() {
                return Err(format!(
                    "encountered errors when inserting recursively: {}",
                    errors.join(";")
                ));
            }
        }

        Ok(())
    }

    fn process_modify(
        &mut self,
        watch_descriptor: &WatchDescriptor,
        events: &mut Vec<Event>,
    ) -> FsResult<()> {
        let mut entry_ptrs_opt = None;
        if let Some(entries) = self.watch_descriptors.get_mut(watch_descriptor) {
            entry_ptrs_opt = Some(entries.clone())
        }

        if let Some(mut entry_ptrs) = entry_ptrs_opt {
            for entry_ptr in entry_ptrs.iter_mut() {
                events.push(Event::Write(*entry_ptr));
            }
            Ok(())
        } else {
            Err(format!(
                "got modify event for untracked watch descriptor: {:?}",
                watch_descriptor
            ))
        }
    }

    fn process_delete(
        &mut self,
        watch_descriptor: &WatchDescriptor,
        name: OsString,
        events: &mut Vec<Event>,
        _entries: &mut EntryMap<T>,
    ) -> FsResult<()> {
        // directories can't be a hard link so we're guaranteed the watch descriptor maps to one
        // entry
        let entry_key = self.get_first_entry(watch_descriptor)?;
        let entry = _entries.get(entry_key).ok_or("failed to find entry")?;
        let mut path = self.resolve_direct_path(&entry.borrow(), _entries);
        path.push(name);
        self.remove(&path, events, _entries)
    }

    fn process_move(
        &mut self,
        from_watch_descriptor: &WatchDescriptor,
        from_name: OsString,
        to_watch_descriptor: &WatchDescriptor,
        to_name: OsString,
        events: &mut Vec<Event>,
        _entries: &mut EntryMap<T>,
    ) -> FsResult<()> {
        let from_entry_key = self.get_first_entry(from_watch_descriptor)?;
        let to_entry_key = self.get_first_entry(to_watch_descriptor)?;

        let from_entry = _entries
            .get(from_entry_key)
            .ok_or("entry source not found")?;
        let to_entry = _entries.get(to_entry_key).ok_or("entry target not found")?;

        let mut from_path = self.resolve_direct_path(&from_entry.borrow(), _entries);
        from_path.push(from_name);
        let mut to_path = self.resolve_direct_path(&to_entry.borrow(), _entries);
        to_path.push(to_name);

        // the entry is expected to exist
        self.rename(&from_path, &to_path, events, _entries)
            .map(|_| ())
    }

    pub fn resolve_direct_path(&self, entry: &Entry<T>, _entries: &EntryMap<T>) -> PathBuf {
        let mut components = Vec::new();

        let mut name = entry.name().clone();
        let mut entry_ptr: Option<EntryKey> = entry.parent();

        loop {
            components.push(name);

            entry_ptr = match entry_ptr {
                Some(parent_ptr) => match _entries.get(parent_ptr) {
                    Some(parent_entry) => {
                        let e = parent_entry.borrow();
                        name = e.name().clone();
                        parent_entry.borrow().parent()
                    }
                    None => break,
                },
                None => break,
            }
        }

        components.reverse();
        components.into_iter().collect()
    }

    pub fn resolve_valid_paths(&self, entry: &Entry<T>, _entries: &EntryMap<T>) -> Vec<PathBuf> {
        let mut paths = Vec::new();
        self.resolve_valid_paths_helper(entry, &mut paths, Vec::new(), _entries);
        paths
    }

    fn resolve_valid_paths_helper(
        &self,
        entry: &Entry<T>,
        paths: &mut Vec<PathBuf>,
        mut components: Vec<OsString>,
        _entries: &EntryMap<T>,
    ) {
        let mut base_components: Vec<OsString> =
            into_components(&self.resolve_direct_path(entry, _entries));
        base_components.append(&mut components); // add components already discovered from previous recursive step
        let path: PathBuf = base_components.iter().collect();
        if self.is_initial_dir_target(&path) {
            // only want paths that fall in our watch window
            paths.push(path); // condense components of path that lead to the true entry into a PathBuf
        }

        let raw_components = base_components.as_slice();
        for i in 0..raw_components.len() - components.len() {
            // only need to iterate components up to current entry
            let current_path: PathBuf = raw_components[0..=i].to_vec().into_iter().collect();

            if let Some(symlinks) = self.symlinks.get(&current_path) {
                // check if path has a symlink to it
                let symlink_components = raw_components[(i + 1)..].to_vec();
                for symlink_ptr in symlinks.iter() {
                    let symlink = _entries.get(*symlink_ptr);
                    if let Some(symlink) = symlink {
                        self.resolve_valid_paths_helper(
                            &symlink.borrow(),
                            paths,
                            symlink_components.clone(),
                            _entries,
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
    fn insert(
        &mut self,
        path: &PathBuf,
        events: &mut Vec<Event>,
        _entries: &mut EntryMap<T>,
    ) -> FsResult<Option<EntryKey>> {
        if !self.passes(path, _entries) {
            info!("ignoring {:?}", path);
            return Ok(None);
        }

        if !(path.exists() || path.read_link().is_ok()) {
            warn!("attempted to insert non existent path {:?}", path);
            return Ok(None);
        }

        let parent_ref = self.create_dir(&path.parent().unwrap().into(), _entries)?;
        let parent_ref = self.follow_links(parent_ref, _entries)?;

        if parent_ref.is_none() {
            return Ok(None);
        }

        let parent_ref = parent_ref.unwrap();

        // We only need the last component, the parents are already inserted.
        let component = into_components(path)
            .pop()
            .ok_or("Unexpected path without components")?;

        // If the path is a dir, we can use create_dir to do the insert
        if path.is_dir() {
            return self.create_dir(path, _entries).map(Some);
        }

        enum Action {
            Return(EntryKey),
            CreateSymlink(PathBuf),
            CreateFile,
        }

        let parent = _entries.get(parent_ref).ok_or("failed to find entry")?;
        let action = match parent
            .borrow_mut()
            .children_mut()
            .ok_or("parent has no children")?
            .entry(component.clone())
        {
            HashMapEntry::Occupied(v) => Action::Return(*v.get()),
            HashMapEntry::Vacant(_) => match path.read_link() {
                Ok(real) => Action::CreateSymlink(real),
                Err(_) => Action::CreateFile,
            },
        };

        match action {
            Action::Return(key) => Ok(Some(key)),
            Action::CreateFile => {
                let wd = self
                    .watcher
                    .watch(path)
                    .map_err(|e| format!("error watching {:?}: {:?}", path, e))?;

                let new_entry = Entry::File {
                    name: component,
                    parent: parent_ref,
                    wd,
                    data: T::default(),
                    file_handle: OpenOptions::new().read(true).open(path).unwrap(),
                };

                let new_key = self.register_as_child(parent_ref, new_entry, _entries)?;
                events.push(Event::New(new_key));
                Ok(Some(new_key))
            }
            Action::CreateSymlink(real) => {
                let wd = self
                    .watcher
                    .watch(path)
                    .map_err(|e| format!("error watching {:?}: {:?}", path, e))?;

                let new_entry = Entry::Symlink {
                    name: component,
                    parent: parent_ref,
                    link: real.clone(),
                    wd,
                    rules: into_rules(real.clone()),
                };

                let new_key = self.register_as_child(parent_ref, new_entry, _entries)?;

                if self
                    .insert(&real, events, _entries)
                    .unwrap_or(None)
                    .is_none()
                {
                    debug!(
                        "inserting symlink {:?} which points to invalid path {:?}",
                        path, real
                    );
                }

                events.push(Event::New(new_key));
                Ok(Some(new_key))
            }
        }
    }

    fn register(&mut self, entry_ptr: EntryKey, _entries: &mut EntryMap<T>) -> FsResult<()> {
        let entry = _entries
            .get(entry_ptr)
            .ok_or("failed to find entry to register")?;
        let path = self.resolve_direct_path(&entry.borrow(), _entries);

        self.watch_descriptors
            .entry(entry.borrow().watch_descriptor().clone())
            .or_insert(Vec::new())
            .push(entry_ptr);

        if let Entry::Symlink { link, .. } = entry.borrow().deref() {
            self.symlinks
                .entry(link.clone())
                .or_insert(Vec::new())
                .push(entry_ptr);
        }

        info!("watching {:?}", path);
        Ok(())
    }

    fn unregister(&mut self, entry_ptr: EntryKey, _entries: &mut EntryMap<T>) {
        if let Some(entry) = _entries.get(entry_ptr) {
            let path = self.resolve_direct_path(&entry.borrow(), _entries);

            let wd = entry.borrow().watch_descriptor().clone();
            let entries = match self.watch_descriptors.get_mut(&wd) {
                Some(v) => v,
                None => {
                    error!("attempted to remove untracked watch descriptor {:?}", wd);
                    return;
                }
            };

            entries.retain(|other| *other != entry_ptr);
            if entries.is_empty() {
                self.watch_descriptors.remove(&wd);
                if let Err(e) = self.watcher.unwatch(wd) {
                    // Log and continue
                    debug!(
                        "unwatching {:?} resulted in an error, likely due to a dangling symlink {}",
                        path, e
                    );
                }
            }

            if let Entry::Symlink { link, .. } = entry.borrow().deref() {
                let entries = match self.symlinks.get_mut(link) {
                    Some(v) => v,
                    None => {
                        error!("attempted to remove untracked symlink {:?}", path);
                        return;
                    }
                };

                entries.retain(|other| *other != entry_ptr);
                if entries.is_empty() {
                    self.symlinks.remove(link);
                }
            }

            info!("unwatching {:?}", path);
        } else {
            error!("failed to find entry to unregister");
        };
    }

    fn remove(
        &mut self,
        path: &PathBuf,
        events: &mut Vec<Event>,
        _entries: &mut EntryMap<T>,
    ) -> FsResult<()> {
        let path_buf = path.parent().ok_or("can not remove root directory")?.into();
        let parent = self
            .lookup(&path_buf, _entries)?
            .ok_or("parent not found")?;
        let component = into_components(path)
            .pop()
            .ok_or("unexpected empty components in path")?;

        let to_drop = _entries
            .get(parent)
            .ok_or("failed to find parent entry")?
            .borrow_mut()
            .children_mut()
            .ok_or("parent should be a directory")?
            .remove(&component);

        if let Some(entry) = to_drop {
            self.drop_entry(entry, events, _entries);
        }

        Ok(())
    }

    fn drop_entry(
        &mut self,
        entry_key: EntryKey,
        events: &mut Vec<Event>,
        _entries: &mut EntryMap<T>,
    ) {
        self.unregister(entry_key, _entries);
        if let Some(entry) = _entries.get(entry_key) {
            let mut _children = vec![];
            let mut _links = vec![];
            match entry.borrow().deref() {
                Entry::Dir { children, .. } => {
                    for (_, child) in children {
                        _children.push(*child);
                        //self.drop_entry(*child, events, _entries);
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
    fn rename(
        &mut self,
        from: &PathBuf,
        to: &PathBuf,
        events: &mut Vec<Event>,
        _entries: &mut EntryMap<T>,
    ) -> FsResult<Option<EntryKey>> {
        let parent_path = to.parent().ok_or("unexpected empty parent")?;
        let new_parent = self.create_dir(&parent_path.into(), _entries)?;

        match self.lookup(from, _entries)? {
            Some(entry_key) => {
                let entry = _entries
                    .get(entry_key)
                    .ok_or("path was found but failed to find entry")?;
                let new_name = into_components(to)
                    .pop()
                    .ok_or("unexpected empty components")?;
                let old_name = entry.borrow().name().clone();

                if let Some(parent) = entry.borrow().parent() {
                    _entries
                        .get(parent)
                        .ok_or("previous parent not found")?
                        .borrow_mut()
                        .children_mut()
                        .ok_or("expected entry to be a directory")?
                        .remove(&old_name);
                }

                let mut entry = entry.borrow_mut();
                entry.set_parent(new_parent);
                entry.set_name(new_name.clone());

                Ok(_entries
                    .get(new_parent)
                    .ok_or("new parent not found")?
                    .borrow_mut()
                    .children_mut()
                    .ok_or("expected entry to be a directory")?
                    .insert(new_name, entry_key))
            }
            None => self.insert(to, events, _entries),
        }
    }

    // Creates all entries for a directory.
    // If one of the entries already exists, it is skipped over.
    // The returns a linked list of all entries.
    fn create_dir(&mut self, path: &PathBuf, _entries: &mut EntryMap<T>) -> FsResult<EntryKey> {
        let mut m_entry = self.root;

        let components = into_components(path);

        // If the path has no parents return root as the parent.
        // This commonly occurs with paths in the filesystem root, e.g /some.file or /somedir
        if components.is_empty() {
            return Ok(m_entry);
        }

        enum Action {
            Return(EntryKey),
            Lookup(PathBuf),
            CreateSymlink(PathBuf),
            CreateDir,
        }

        for (i, component) in components.iter().enumerate().skip(1) {
            let current_path = PathBuf::from_iter(&components[0..=i]);

            let action = match _entries
                .get(m_entry)
                .ok_or("parent entry not found")?
                .borrow_mut()
                .children_mut()
                .ok_or("unexpected parent without children")?
                .entry(component.clone())
            {
                HashMapEntry::Occupied(v) => {
                    let entry_key = *v.get();
                    let existing_entry = _entries
                        .get(entry_key)
                        .ok_or("unexpected entry not found")?;

                    match existing_entry.borrow().deref() {
                        Entry::Symlink { link, .. } => Action::Lookup(link.clone()),
                        _ => Action::Return(entry_key),
                    }
                }
                HashMapEntry::Vacant(_) => match current_path.read_link() {
                    Ok(real) => Action::CreateSymlink(real),
                    Err(_) => Action::CreateDir,
                },
            };

            let new_entry = match action {
                Action::Return(key) => key,
                Action::Lookup(ref link) => self
                    .lookup(link, _entries)?
                    .ok_or("path lookup for new entry not found")?,
                Action::CreateDir => {
                    let wd = self
                        .watcher
                        .watch(&current_path)
                        .map_err(|e| format!("error watching {:?}: {:?}", current_path, e))?;

                    let new_entry = Entry::Dir {
                        name: component.clone(),
                        parent: Some(m_entry),
                        children: HashMap::new(),
                        wd,
                    };

                    self.register_as_child(m_entry, new_entry, _entries)?
                }
                Action::CreateSymlink(real) => {
                    let wd = self
                        .watcher
                        .watch(&current_path)
                        .map_err(|e| format!("error watching {:?}: {:?}", current_path, e))?;

                    let new_entry = Entry::Symlink {
                        name: component.clone(),
                        parent: m_entry,
                        link: real.clone(),
                        wd,
                        rules: into_rules(real.clone()),
                    };

                    self.register_as_child(m_entry, new_entry, _entries)?;
                    self.create_dir(&real, _entries)?
                }
            };

            m_entry = new_entry;
        }
        Ok(m_entry)
    }

    /// Inserts the entry, registers it, looks up for the parent and set itself as a children.
    fn register_as_child(
        &mut self,
        parent_key: EntryKey,
        new_entry: Entry<T>,
        entries: &mut EntryMap<T>,
    ) -> FsResult<EntryKey> {
        let component = new_entry.name().clone();
        let new_key = entries.insert(RefCell::new(new_entry));
        self.register(new_key, entries)?;

        match entries
            .get(parent_key)
            .ok_or("parent directory not found when inserting child")?
            .borrow_mut()
            .children_mut()
            .ok_or("parent file expected to be a directory")?
            .entry(component)
        {
            HashMapEntry::Vacant(v) => Ok(*v.insert(new_key)),
            _ => Err("Unexpected existing child entry".into()),
        }
    }

    /// Returns the entry that represents the supplied path or `Ok(None)`.
    pub fn lookup(&self, path: &PathBuf, _entries: &EntryMap<T>) -> FsResult<Option<EntryKey>> {
        let mut parent = self.root;
        let mut components = into_components(path);
        // remove the first component because it will always be the root
        components.remove(0);

        // If the path has no components there is nothing to look up.
        if components.is_empty() {
            return Ok(None);
        }

        let last_component = components.pop().unwrap();

        for component in components {
            if let Some(entry) = self.follow_links(parent, _entries)? {
                if let Some(parent_ref) = _entries
                    .get(entry)
                    .ok_or("failed to find entry")?
                    .borrow()
                    .children()
                    .ok_or("expected directory entry")?
                    .get(&component)
                {
                    if let Some(parent_ref) = self.follow_links(*parent_ref, _entries)? {
                        parent = parent_ref;
                        continue;
                    }
                }
            }

            // When components are not found `Ok(None)` is returned
            return Ok(None);
        }

        Ok(_entries
            .get(parent)
            .ok_or("failed to find entry")?
            .borrow()
            .children()
            .ok_or("expected directory entry")?
            .get(&last_component)
            .copied())
    }

    fn follow_links(&self, entry: EntryKey, _entries: &EntryMap<T>) -> FsResult<Option<EntryKey>> {
        let mut result = entry;
        while let Some(e) = _entries.get(entry) {
            if let Some(link) = e.borrow().link() {
                if let Some(e) = self.lookup(link, _entries)? {
                    result = e;
                } else {
                    return Ok(None);
                }
            } else {
                break;
            }
        }
        Ok(Some(result))
    }

    fn is_symlink_target(&self, path: &PathBuf, _entries: &EntryMap<T>) -> bool {
        for (_, symlink_ptrs) in self.symlinks.iter() {
            for symlink_ptr in symlink_ptrs.iter() {
                if let Some(symlink) = _entries.get(*symlink_ptr) {
                    match symlink.borrow().deref() {
                        Entry::Symlink { rules, .. } => {
                            if let Status::Ok = rules.passes(path) {
                                if let Status::Ok = self.master_rules.included(path) {
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
                    error!("failed to find entry");
                };
            }
        }
        false
    }

    fn is_initial_dir_target(&self, path: &PathBuf) -> bool {
        if let Status::Ok = self.initial_dir_rules.passes(path) {
            if let Status::Ok = self.master_rules.passes(path) {
                return true;
            }
        }
        false
    }

    /// Helper method for checking if a path passes exclusion/inclusion rules
    fn passes(&self, path: &PathBuf, _entries: &EntryMap<T>) -> bool {
        self.is_initial_dir_target(path) || self.is_symlink_target(path, _entries)
    }

    fn entry_path_passes(&self, entry: EntryKey, name: OsString, _entries: &EntryMap<T>) -> bool {
        if let Some(entry_ref) = _entries.get(entry) {
            let mut path = self.resolve_direct_path(&entry_ref.borrow(), &_entries);
            path.push(name);
            self.passes(&path, &_entries)
        } else {
            false
        }
    }

    /// Returns the first entry based on the `WatchDescriptor`, returning an `Err` when not found.
    fn get_first_entry(&self, wd: &WatchDescriptor) -> FsResult<EntryKey> {
        let entries = self
            .watch_descriptors
            .get(wd)
            .ok_or_else(|| format!("got event for untracked watch descriptor: {:?}", wd))?;

        if !entries.is_empty() {
            Ok(entries[0])
        } else {
            Err(format!(
                "got event where to watch descriptors maps to no entries: {:?}",
                wd
            ))
        }
    }
}

// conditionally implement std::fmt::Debug if the underlying type T implements it
impl<T: fmt::Debug + Clone> fmt::Debug for FileSystem<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut builder = f.debug_struct("FileSystem");
        builder.field("root", &&self.root);
        builder.field("symlinks", &&self.symlinks);
        builder.field("watch_descriptors", &&self.watch_descriptors);
        builder.field("master_rules", &&self.master_rules);
        builder.field("initial_dir_rules", &&self.initial_dir_rules);
        builder.field("initial_events", &&self.initial_events);
        builder.finish()
    }
}

// recursively scans a directory for unlimited depth
fn recursive_scan(path: &PathBuf) -> Vec<PathBuf> {
    if !path.is_dir() {
        return vec![];
    }

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
fn into_components(path: &PathBuf) -> Vec<OsString> {
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
        GlobRule::new(path.join(r"**").to_str().expect("invalid unicode in path"))
            .expect("invalid glob rule format"),
    );

    loop {
        rules.add_inclusion(
            GlobRule::new(path.to_str().expect("invalid unicode in path"))
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
    use crate::rule::{GlobRule, Rules};
    use crate::test::LOGGER;
    use std::convert::TryInto;
    use std::fs::{copy, create_dir, hard_link, remove_dir_all, remove_file, rename, File};
    use std::os::unix::fs::symlink;
    use std::{io, panic};
    use tempfile::TempDir;

    macro_rules! take_events {
        ( $x:expr, $y: expr ) => {{
            use tokio::stream::StreamExt;
            let mut buf = [0u8; 4096];

            tokio_test::block_on(async {
                futures::StreamExt::collect::<Vec<_>>(futures::StreamExt::take(
                    FileSystem::stream_events($x.clone(), &mut buf)
                        .expect("failed to read events")
                        .timeout(std::time::Duration::from_millis(500)),
                    $y,
                ))
                .await
            })
        }};
    }

    macro_rules! lookup_entry {
        ( $x:expr, $y: expr ) => {{
            let fs = $x.lock().expect("failed to lock fs");
            let entries = fs.entries.clone();
            let entries = entries.borrow();
            fs.lookup(&$y, &entries).unwrap()
        }};
    }

    fn new_fs<T: Default + Clone + std::fmt::Debug>(
        path: PathBuf,
        rules: Option<Rules>,
    ) -> FileSystem<T> {
        let rules = rules.unwrap_or_else(|| {
            let mut rules = Rules::new();
            rules.add_inclusion(GlobRule::new(r"**").unwrap());
            rules
        });
        FileSystem::new(
            vec![path
                .as_path()
                .try_into()
                .unwrap_or_else(|_| panic!("{:?} is not a directory!", path))],
            rules,
        )
    }

    fn run_test<T: FnOnce() + panic::UnwindSafe>(test: T) {
        #![allow(unused_must_use, clippy::clone_on_copy)]
        LOGGER.clone();
        let result = panic::catch_unwind(|| {
            test();
        });

        assert!(result.is_ok())
    }

    // Simulates the `create_move` log rotation strategy
    #[test]
    fn filesystem_rotate_create_move() {
        run_test(|| {
            let tempdir = TempDir::new().unwrap();
            let path = tempdir.path().to_path_buf();

            let fs = Arc::new(Mutex::new(new_fs::<()>(path.clone(), None)));

            let a = path.join("a");
            File::create(&a).unwrap();

            take_events!(fs, 1);
            let entry = lookup_entry!(fs, a);
            assert!(entry.is_some());
            {
                let _fs = fs.lock().expect("couldn't lock fs");
                let _entries = &_fs.entries;
                let _entries = _entries.borrow();
                match _entries.get(entry.unwrap()).unwrap().borrow().deref() {
                    Entry::File { .. } => {}
                    _ => panic!("wrong entry type"),
                };
            }

            let old = path.join("a.old");
            rename(&a, &old).unwrap();

            take_events!(fs, 1);

            let entry = lookup_entry!(fs, a);
            assert!(entry.is_none());

            let entry = lookup_entry!(fs, old);
            assert!(entry.is_some());
            {
                let _fs = fs.lock().expect("couldn't lock fs");
                let _entries = &_fs.entries;
                let _entries = _entries.borrow();
                match _entries.get(entry.unwrap()).unwrap().borrow().deref() {
                    Entry::File { .. } => {}
                    _ => panic!("wrong entry type"),
                };
            }
            File::create(&a).unwrap();

            take_events!(fs, 1);

            let entry = lookup_entry!(fs, a);
            assert!(entry.is_some());
            {
                let _fs = fs.lock().expect("couldn't lock fs");
                let _entries = &_fs.entries;
                let _entries = _entries.borrow();
                match _entries.get(entry.unwrap()).unwrap().borrow().deref() {
                    Entry::File { .. } => {}
                    _ => panic!("wrong entry type"),
                };
            }
        });
    }

    // Simulates the `create_copy` log rotation strategy
    #[test]
    fn filesystem_rotate_create_copy() {
        run_test(|| {
            let tempdir = TempDir::new().unwrap();
            let path = tempdir.path().to_path_buf();

            let fs = Arc::new(Mutex::new(new_fs::<()>(path.clone(), None)));

            let a = path.join("a");
            File::create(&a).unwrap();

            take_events!(fs, 1);

            let entry = lookup_entry!(fs, a);

            assert!(entry.is_some());
            {
                let _fs = fs.lock().expect("couldn't lock fs");
                let _entries = &_fs.entries;
                let _entries = _entries.borrow();
                match _entries.get(entry.unwrap()).unwrap().borrow().deref() {
                    Entry::File { .. } => {}
                    _ => panic!("wrong entry type"),
                };
            }

            let old = path.join("a.old");
            copy(&a, &old).unwrap();
            remove_file(&a).unwrap();

            take_events!(fs, 2);

            let entry = lookup_entry!(fs, a);
            assert!(entry.is_none());

            let entry = lookup_entry!(fs, old);

            assert!(entry.is_some());
            {
                let _fs = fs.lock().expect("couldn't lock fs");
                let _entries = &_fs.entries;
                let _entries = _entries.borrow();
                match _entries.get(entry.unwrap()).unwrap().borrow().deref() {
                    Entry::File { .. } => {}
                    _ => panic!("wrong entry type"),
                };
            }

            File::create(&a).unwrap();

            take_events!(fs, 1);

            let entry = lookup_entry!(fs, a);
            assert!(entry.is_some());
            let _fs = fs.lock().expect("couldn't lock fs");
            let _entries = &_fs.entries;
            let _entries = _entries.borrow();
            match _entries.get(entry.unwrap()).unwrap().borrow().deref() {
                Entry::File { .. } => {}
                _ => panic!("wrong entry type"),
            };
        });
    }

    // Creates a plain old dir
    #[test]
    fn filesystem_create_dir() {
        run_test(|| {
            let tempdir = TempDir::new().unwrap();
            let path = tempdir.path().to_path_buf();

            let fs = Arc::new(Mutex::new(new_fs::<()>(path.clone(), None)));

            take_events!(fs, 1);

            assert!(lookup_entry!(fs, path).is_some());
        });
    }

    // Creates a plain old file
    #[test]
    fn filesystem_create_file() {
        run_test(|| {
            let tempdir = TempDir::new().unwrap();
            let path = tempdir.path().to_path_buf();

            let fs = Arc::new(Mutex::new(new_fs::<()>(path.clone(), None)));

            File::create(path.join("insert.log")).unwrap();
            take_events!(fs, 1);

            assert!(lookup_entry!(fs, path.join("insert.log")).is_some());
        });
    }

    // Creates a symlink
    #[test]
    fn filesystem_create_symlink() {
        run_test(|| {
            let tempdir = TempDir::new().unwrap();
            let path = tempdir.path().to_path_buf();

            let fs = Arc::new(Mutex::new(new_fs::<()>(path.clone(), None)));

            let a = path.join("a");
            let b = path.join("b");
            create_dir(&a).unwrap();
            symlink(&a, &b).unwrap();

            take_events!(fs, 1);

            let entry = lookup_entry!(fs, a);
            assert!(entry.is_some());
            let entry2 = lookup_entry!(fs, b);
            assert!(entry.is_some());
            let _fs = fs.lock().expect("couldn't lock fs");
            let _entries = &_fs.entries;
            let _entries = _entries.borrow();
            match _entries.get(entry.unwrap()).unwrap().borrow().deref() {
                Entry::Dir { .. } => {}
                _ => panic!("wrong entry type"),
            };

            match _entries.get(entry2.unwrap()).unwrap().borrow().deref() {
                Entry::Symlink { link, .. } => {
                    assert_eq!(*link, a);
                }
                _ => panic!("wrong entry type"),
            };
        });
    }

    // Creates a hardlink
    #[test]
    fn filesystem_create_hardlink() {
        run_test(|| {
            let tempdir = TempDir::new().unwrap();
            let path = tempdir.path().to_path_buf();

            let fs = Arc::new(Mutex::new(new_fs::<()>(path.clone(), None)));

            let file_path = path.join("insert.log");
            let hard_path = path.join("hard.log");
            File::create(file_path.clone()).unwrap();
            hard_link(&file_path, &hard_path).unwrap();

            take_events!(fs, 2);

            let entry = lookup_entry!(fs, file_path).unwrap();
            let entry2 = lookup_entry!(fs, hard_path);
            let real_watch_descriptor;
            let _fs = fs.lock().expect("couldn't lock fs");
            let _entries = &_fs.entries;
            let _entries = _entries.borrow();
            let _entry = _entries.get(entry).unwrap();
            let _entry = _entry.borrow();
            match _entry.deref() {
                Entry::File { wd, .. } => {
                    real_watch_descriptor = wd;
                }
                _ => panic!("wrong entry type"),
            };

            assert!(entry2.is_some());
            match _entries.get(entry2.unwrap()).unwrap().borrow().deref() {
                Entry::File { ref wd, .. } => assert_eq!(wd, real_watch_descriptor),
                _ => panic!("wrong entry type"),
            };
        });
    }

    // Deletes a directory
    #[test]
    fn filesystem_delete_filled_dir() {
        run_test(|| {
            let tempdir = TempDir::new().unwrap();
            let path = tempdir.path().to_path_buf();

            let file_path = path.join("file.log");
            let sym_path = path.join("sym.log");
            let hard_path = path.join("hard.log");
            File::create(file_path.clone()).unwrap();
            symlink(&file_path, &sym_path).unwrap();
            hard_link(&file_path, &hard_path).unwrap();

            let fs = Arc::new(Mutex::new(new_fs::<()>(path.clone(), None)));

            assert!(lookup_entry!(fs, path).is_some());
            assert!(lookup_entry!(fs, file_path).is_some());
            assert!(lookup_entry!(fs, sym_path).is_some());
            assert!(lookup_entry!(fs, hard_path).is_some());

            tempdir.close().unwrap();
            take_events!(fs, 7);

            assert!(lookup_entry!(fs, path).is_none());
            assert!(lookup_entry!(fs, file_path).is_none());
            assert!(lookup_entry!(fs, sym_path).is_none());
            assert!(lookup_entry!(fs, hard_path).is_none());
        });
    }

    // Deletes a file
    #[test]
    fn filesystem_delete_file() {
        run_test(|| {
            let tempdir = TempDir::new().unwrap();
            let path = tempdir.path().to_path_buf();

            let file_path = path.join("file");
            File::create(file_path.clone()).unwrap();

            let fs = Arc::new(Mutex::new(new_fs::<()>(path, None)));

            assert!(lookup_entry!(fs, file_path).is_some());

            remove_file(&file_path).unwrap();
            take_events!(fs, 2);

            assert!(lookup_entry!(fs, file_path).is_none());
        });
    }

    // Deletes a symlink
    #[test]
    fn filesystem_delete_symlink() {
        run_test(|| {
            let tempdir = TempDir::new().unwrap();
            let path = tempdir.path().to_path_buf();

            let a = path.join("a");
            let b = path.join("b");
            create_dir(&a).unwrap();
            symlink(&a, &b).unwrap();

            let fs = Arc::new(Mutex::new(new_fs::<()>(path, None)));

            remove_dir_all(&b).unwrap();
            take_events!(fs, 1);

            assert!(lookup_entry!(fs, a).is_some());
            assert!(lookup_entry!(fs, b).is_none());
        });
    }

    /// Deletes a symlink that points to a not tracked directory
    #[test]
    fn filesystem_delete_symlink_to_untracked_dir() -> io::Result<()> {
        let tempdir = TempDir::new()?;
        let tempdir2 = TempDir::new()?.into_path();
        let path = tempdir.path().to_path_buf();

        let real_dir_path = tempdir2.join("real_dir_sample");
        let symlink_path = path.join("symlink_sample");
        create_dir(&real_dir_path)?;
        symlink(&real_dir_path, &symlink_path)?;

        let fs = Arc::new(Mutex::new(new_fs::<()>(path, None)));
        assert!(lookup_entry!(fs, symlink_path).is_some());
        assert!(lookup_entry!(fs, real_dir_path).is_some());

        remove_dir_all(&symlink_path)?;
        take_events!(fs, 1);

        assert!(lookup_entry!(fs, symlink_path).is_none());
        assert!(lookup_entry!(fs, real_dir_path).is_none());
        Ok(())
    }

    // Deletes the pointee of a symlink
    #[test]
    fn filesystem_delete_symlink_pointee() {
        run_test(|| {
            let tempdir = TempDir::new().unwrap();
            let path = tempdir.path().to_path_buf();

            let a = path.join("a");
            let b = path.join("b");
            create_dir(&a).unwrap();
            symlink(&a, &b).unwrap();

            let fs = Arc::new(Mutex::new(new_fs::<()>(path, None)));

            remove_dir_all(&a).unwrap();
            take_events!(fs, 1);

            assert!(lookup_entry!(fs, a).is_none());
            assert!(lookup_entry!(fs, b).is_some());
        });
    }

    // Deletes a hardlink
    #[test]
    fn filesystem_delete_hardlink() {
        run_test(|| {
            let tempdir = TempDir::new().unwrap();
            let path = tempdir.path().to_path_buf();

            let a = path.join("a");
            let b = path.join("b");
            File::create(a.clone()).unwrap();
            hard_link(&a, &b).unwrap();

            let fs = Arc::new(Mutex::new(new_fs::<()>(path, None)));

            assert!(lookup_entry!(fs, a).is_some());
            assert!(lookup_entry!(fs, b).is_some());

            remove_file(&b).unwrap();
            take_events!(fs, 3);

            assert!(lookup_entry!(fs, a).is_some());
            assert!(lookup_entry!(fs, b).is_none());
        });
    }

    // Deletes the pointee of a hardlink (not totally accurate since we're not deleting the inode
    // entry, but what evs)
    #[test]
    fn filesystem_delete_hardlink_pointee() {
        run_test(|| {
            let tempdir = TempDir::new().unwrap();
            let path = tempdir.path().to_path_buf();

            let a = path.join("a");
            let b = path.join("b");
            File::create(a.clone()).unwrap();
            hard_link(&a, &b).unwrap();

            let fs = Arc::new(Mutex::new(new_fs::<()>(path, None)));

            remove_file(&a).unwrap();
            take_events!(fs, 3);

            assert!(lookup_entry!(fs, a).is_none());
            assert!(lookup_entry!(fs, b).is_some());
        });
    }

    // Moves a directory within the watched directory
    #[test]
    fn filesystem_move_dir_internal() {
        run_test(|| {
            let tempdir = TempDir::new().unwrap();
            let path = tempdir.path().to_path_buf();

            let old_dir_path = path.join("old");
            let new_dir_path = path.join("new");
            let file_path = old_dir_path.join("file.log");
            let sym_path = old_dir_path.join("sym.log");
            let hard_path = old_dir_path.join("hard.log");
            create_dir(&old_dir_path).unwrap();
            File::create(file_path.clone()).unwrap();
            symlink(&file_path, &sym_path).unwrap();
            hard_link(&file_path, &hard_path).unwrap();

            let fs = Arc::new(Mutex::new(new_fs::<()>(path, None)));

            rename(&old_dir_path, &new_dir_path).unwrap();
            take_events!(fs, 4);

            assert!(lookup_entry!(fs, old_dir_path).is_none());
            assert!(lookup_entry!(fs, file_path).is_none());
            assert!(lookup_entry!(fs, sym_path).is_none());
            assert!(lookup_entry!(fs, hard_path).is_none());

            let entry = lookup_entry!(fs, new_dir_path);
            assert!(entry.is_some());

            let entry2 = lookup_entry!(fs, new_dir_path.join("file.log"));
            assert!(entry2.is_some());

            let entry3 = lookup_entry!(fs, new_dir_path.join("hard.log"));
            assert!(entry3.is_some());

            let entry4 = lookup_entry!(fs, new_dir_path.join("sym.log"));
            assert!(entry4.is_some());

            let _fs = fs.lock().expect("couldn't lock fs");
            let _entries = &_fs.entries;
            let _entries = _entries.borrow();
            match _entries.get(entry.unwrap()).unwrap().borrow().deref() {
                Entry::Dir { .. } => {}
                _ => panic!("wrong entry type"),
            };

            match _entries.get(entry2.unwrap()).unwrap().borrow().deref() {
                Entry::File { .. } => {}
                _ => panic!("wrong entry type"),
            };

            match _entries.get(entry3.unwrap()).unwrap().borrow().deref() {
                Entry::File { .. } => {}
                _ => panic!("wrong entry type"),
            };

            match _entries.get(entry4.unwrap()).unwrap().borrow().deref() {
                Entry::Symlink { link, .. } => {
                    // symlinks don't update so this link is bad
                    assert_eq!(*link, file_path);
                }
                _ => panic!("wrong entry type"),
            };
        });
    }

    // Moves a directory out
    #[test]
    fn filesystem_move_dir_out() {
        run_test(|| {
            let tempdir = TempDir::new().unwrap();
            let path = tempdir.path().to_path_buf();

            let old_dir_path = path.join("old");
            let new_dir_path = path.join("new");
            let file_path = old_dir_path.join("file.log");
            let sym_path = old_dir_path.join("sym.log");
            let hard_path = old_dir_path.join("hard.log");
            create_dir(&old_dir_path).unwrap();
            File::create(file_path.clone()).unwrap();
            symlink(&file_path, &sym_path).unwrap();
            hard_link(&file_path, &hard_path).unwrap();

            let fs = Arc::new(Mutex::new(new_fs::<()>(old_dir_path.clone(), None)));

            rename(&old_dir_path, &new_dir_path).unwrap();
            take_events!(fs, 1);

            assert!(lookup_entry!(fs, new_dir_path).is_none());
            assert!(lookup_entry!(fs, new_dir_path.join("file.log")).is_none());
            assert!(lookup_entry!(fs, new_dir_path.join("hard.log")).is_none());
            assert!(lookup_entry!(fs, new_dir_path.join("sym.log")).is_none());
        });
    }

    // Moves a directory in
    #[test]
    fn filesystem_move_dir_in() {
        run_test(|| {
            let old_tempdir = TempDir::new().unwrap();
            let old_path = old_tempdir.path().to_path_buf();

            let new_tempdir = TempDir::new().unwrap();
            let new_path = new_tempdir.path().to_path_buf();

            let old_dir_path = old_path.join("old");
            let new_dir_path = new_path.join("new");
            let file_path = old_dir_path.join("file.log");
            let sym_path = old_dir_path.join("sym.log");
            let hard_path = old_dir_path.join("hard.log");
            create_dir(&old_dir_path).unwrap();
            File::create(file_path.clone()).unwrap();
            symlink(&file_path, &sym_path).unwrap();
            hard_link(&file_path, &hard_path).unwrap();

            let fs = Arc::new(Mutex::new(new_fs::<()>(new_path, None)));

            assert!(lookup_entry!(fs, old_dir_path).is_none());
            assert!(lookup_entry!(fs, new_dir_path).is_none());
            assert!(lookup_entry!(fs, file_path).is_none());
            assert!(lookup_entry!(fs, sym_path).is_none());
            assert!(lookup_entry!(fs, hard_path).is_none());

            rename(&old_dir_path, &new_dir_path).unwrap();
            take_events!(fs, 2);

            let entry = lookup_entry!(fs, new_dir_path);
            assert!(entry.is_some());

            let entry2 = lookup_entry!(fs, new_dir_path.join("file.log"));
            assert!(entry2.is_some());

            let entry3 = lookup_entry!(fs, new_dir_path.join("hard.log"));
            assert!(entry3.is_some());

            let entry4 = lookup_entry!(fs, new_dir_path.join("sym.log"));
            assert!(entry4.is_some());

            let _fs = fs.lock().expect("couldn't lock fs");
            let _entries = &_fs.entries;
            let _entries = _entries.borrow();
            match _entries.get(entry.unwrap()).unwrap().borrow().deref() {
                Entry::Dir { .. } => {}
                _ => panic!("wrong entry type"),
            };

            match _entries.get(entry2.unwrap()).unwrap().borrow().deref() {
                Entry::File { .. } => {}
                _ => panic!("wrong entry type"),
            };

            match _entries.get(entry3.unwrap()).unwrap().borrow().deref() {
                Entry::File { .. } => {}
                _ => panic!("wrong entry type"),
            };

            match _entries.get(entry4.unwrap()).unwrap().borrow().deref() {
                Entry::Symlink { link, .. } => {
                    // symlinks don't update so this link is bad
                    assert_eq!(*link, file_path);
                }
                _ => panic!("wrong entry type"),
            };
        });
    }

    // Moves a file within the watched directory
    #[test]
    fn filesystem_move_file_internal() {
        run_test(|| {
            let tempdir = TempDir::new().unwrap();
            let path = tempdir.path().to_path_buf();

            let fs = Arc::new(Mutex::new(new_fs::<()>(path.clone(), None)));

            let file_path = path.join("insert.log");
            let new_path = path.join("new.log");
            File::create(file_path.clone()).unwrap();
            rename(&file_path, &new_path).unwrap();

            take_events!(fs, 1);

            let entry = lookup_entry!(fs, file_path);
            assert!(entry.is_none());

            let entry = lookup_entry!(fs, new_path);
            assert!(entry.is_some());
            let _fs = fs.lock().expect("couldn't lock fs");
            let _entries = &_fs.entries;
            let _entries = _entries.borrow();
            match _entries.get(entry.unwrap()).unwrap().borrow().deref() {
                Entry::File { .. } => {}
                _ => panic!("wrong entry type"),
            };
        });
    }

    // Moves a file out of the watched directory
    #[test]
    fn filesystem_move_file_out() {
        run_test(|| {
            let tempdir = TempDir::new().unwrap();
            let path = tempdir.path().to_path_buf();

            let watch_path = path.join("watch");
            let other_path = path.join("other");
            create_dir(&watch_path).unwrap();
            create_dir(&other_path).unwrap();

            let file_path = watch_path.join("inside.log");
            let move_path = other_path.join("outside.log");
            File::create(file_path.clone()).unwrap();

            let fs = Arc::new(Mutex::new(new_fs::<()>(watch_path, None)));

            rename(&file_path, &move_path).unwrap();

            take_events!(fs, 2);

            let entry = lookup_entry!(fs, file_path);
            assert!(entry.is_none());

            let entry = lookup_entry!(fs, move_path);
            assert!(entry.is_none());
        });
    }

    // Moves a file into the watched directory
    #[test]
    fn filesystem_move_file_in() {
        run_test(|| {
            let tempdir = TempDir::new().unwrap();
            let path = tempdir.path().to_path_buf();

            let watch_path = path.join("watch");
            let other_path = path.join("other");
            create_dir(&watch_path).unwrap();
            create_dir(&other_path).unwrap();

            let file_path = other_path.join("inside.log");
            let move_path = watch_path.join("outside.log");
            File::create(file_path.clone()).unwrap();

            let fs = Arc::new(Mutex::new(new_fs::<()>(watch_path, None)));

            rename(&file_path, &move_path).unwrap();
            File::create(file_path.clone()).unwrap();

            take_events!(fs, 1);

            let entry = lookup_entry!(fs, file_path);
            assert!(entry.is_none());

            let entry = lookup_entry!(fs, move_path);
            assert!(entry.is_some());
            let _fs = fs.lock().expect("couldn't lock fs");
            let _entries = &_fs.entries;
            let _entries = _entries.borrow();
            match _entries.get(entry.unwrap()).unwrap().borrow().deref() {
                Entry::File { .. } => {}
                _ => panic!("wrong entry type"),
            };
        });
    }

    // Moves a file out of the watched directory
    #[test]
    fn filesystem_move_symlink_file_out() {
        run_test(|| {
            let tempdir = TempDir::new().unwrap();
            let path = tempdir.path().to_path_buf();

            let watch_path = path.join("watch");
            let other_path = path.join("other");
            create_dir(&watch_path).unwrap();
            create_dir(&other_path).unwrap();

            let file_path = other_path.join("inside.log");
            let move_path = other_path.join("outside.tmp");
            let sym_path = watch_path.join("sym.log");
            File::create(file_path.clone()).unwrap();
            symlink(&file_path, &sym_path).unwrap();

            let fs = Arc::new(Mutex::new(new_fs::<()>(watch_path, None)));

            rename(&file_path, &move_path).unwrap();

            take_events!(fs, 3);

            let entry = lookup_entry!(fs, sym_path);
            assert!(entry.is_some());

            let entry = lookup_entry!(fs, file_path);
            assert!(entry.is_none());

            let entry = lookup_entry!(fs, move_path);
            assert!(entry.is_none());
        });
    }

    // Watch symlink target that is excluded
    #[test]
    fn filesystem_watch_symlink_w_excluded_target() {
        run_test(|| {
            let tempdir = TempDir::new().unwrap();
            let path = tempdir.path().to_path_buf();

            let mut rules = Rules::new();
            rules.add_inclusion(GlobRule::new("*.log").unwrap());
            rules.add_inclusion(
                GlobRule::new(&*format!("{}{}", tempdir.path().to_str().unwrap(), "*")).unwrap(),
            );
            rules.add_exclusion(GlobRule::new("*.tmp").unwrap());

            let file_path = path.join("test.tmp");
            let sym_path = path.join("test.log");
            File::create(file_path.clone()).unwrap();

            let fs = Arc::new(Mutex::new(new_fs::<()>(path, Some(rules))));

            let entry = lookup_entry!(fs, file_path);
            assert!(entry.is_none());

            symlink(&file_path, &sym_path).unwrap();

            take_events!(fs, 1);

            let entry = lookup_entry!(fs, sym_path);
            assert!(entry.is_some());

            let entry = lookup_entry!(fs, file_path);
            assert!(entry.is_some());
        });
    }

    #[test]
    fn filesystem_resolve_valid_paths() {
        run_test(|| {
            let tempdir = TempDir::new().unwrap();
            let path = tempdir.path().to_path_buf();

            let test_dir_path = path.join("testdir");
            let nested_dir_path = path.join("nested");
            let local_symlink_path = path.join("local_symlink.log");
            let remote_symlink_path = test_dir_path.join("remote_symlink.log");
            let nested_symlink_path = nested_dir_path.join("nested_symlink.log");
            let double_nested_symlink_path = nested_dir_path.join("double_nested_symlink.log");
            let file_path = path.join("file.log");

            File::create(file_path.clone()).unwrap();
            create_dir(&test_dir_path).unwrap();
            create_dir(&nested_dir_path).unwrap();
            symlink(&file_path, &remote_symlink_path).unwrap();
            symlink(&file_path, &local_symlink_path).unwrap();
            symlink(&remote_symlink_path, &nested_symlink_path).unwrap();
            symlink(&nested_symlink_path, &double_nested_symlink_path).unwrap();

            let fs = Arc::new(Mutex::new(new_fs::<()>(path, None)));
            let entry = lookup_entry!(fs, file_path).unwrap();

            let _fs = fs.lock().expect("failed to lock fs");
            let entries = _fs.entries.borrow();
            let resolved_paths = _fs.resolve_valid_paths(
                _fs.entries.borrow().get(entry).unwrap().borrow().deref(),
                &entries,
            );

            assert!(resolved_paths
                .iter()
                .any(|other| other.to_str() == local_symlink_path.to_str()));
            assert!(resolved_paths
                .iter()
                .any(|other| other.to_str() == remote_symlink_path.to_str()));
            assert!(resolved_paths
                .iter()
                .any(|other| other.to_str() == file_path.to_str()));
            assert!(resolved_paths
                .iter()
                .any(|other| other.to_str() == nested_symlink_path.to_str()));
            assert!(resolved_paths
                .iter()
                .any(|other| other.to_str() == double_nested_symlink_path.to_str()));
        });
    }
}

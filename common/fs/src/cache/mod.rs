use crate::cache::entry::{Entry, EntryPtr};
use crate::cache::event::Event;
use crate::cache::watch::{WatchEvent, Watcher};
use crate::rule::{GlobRule, Rules, Status};
use hashbrown::hash_map::Entry as HashMapEntry;
use hashbrown::HashMap;
use inotify::WatchDescriptor;
use metrics::Metrics;
use std::cell::RefCell;
use std::ffi::OsString;
use std::fmt;
use std::fs::read_dir;
use std::fs::OpenOptions;
use std::iter::FromIterator;
use std::ops::{Deref, DerefMut};
use std::path::{Component, PathBuf};
use std::ptr::NonNull;
use std::rc::Rc;

pub mod entry;
pub mod event;
mod watch;

type Children<T> = HashMap<OsString, Box<Entry<T>>>;
type Symlinks<T> = HashMap<PathBuf, Vec<EntryPtr<T>>>;
type WatchDescriptors<T> = HashMap<WatchDescriptor, Vec<EntryPtr<T>>>;

pub struct FileSystem<T> {
    watcher: Watcher,
    root: Box<Entry<T>>,

    symlinks: Rc<RefCell<Symlinks<T>>>,
    watch_descriptors: Rc<RefCell<WatchDescriptors<T>>>,

    master_rules: Rules,
    initial_dir_rules: Rules,

    initial_events: Vec<Event<T>>,
}

impl<T: Default> FileSystem<T> {
    pub fn new(inital_dirs: Vec<PathBuf>, rules: Rules) -> Self {
        let mut watcher = Watcher::new().expect("unable to initialize inotify");

        let root = Box::new(Entry::Dir {
            name: "/".into(),
            parent: None,
            children: Children::new(),
            wd: watcher.watch("/").expect("unable to watch /"),
        });

        let mut initial_dir_rules = Rules::new();
        for path in inital_dirs.iter() {
            append_rules(&mut initial_dir_rules, path.clone());
        }

        let mut fs = Self {
            root,
            symlinks: Rc::new(RefCell::new(Symlinks::new())),
            watch_descriptors: Rc::new(RefCell::new(WatchDescriptors::new())),
            master_rules: rules,
            initial_dir_rules,
            watcher,
            initial_events: Vec::new(),
        };

        let root = EntryPtr::from(fs.root.deref_mut());
        fs.register(root);

        for dir in inital_dirs.iter() {
            let mut path_cpy = dir.clone();
            loop {
                if !path_cpy.exists() {
                    path_cpy.pop();
                } else {
                    fs.insert(&path_cpy, &mut |_, _| {});
                    break;
                }
            }

            for path in recursive_scan(dir) {
                fs.insert(&path, &mut |fs_ref, event| {
                    match event {
                        Event::New(entry) => fs_ref.initial_events.push(Event::Initialize(entry)),
                        _ => panic!("unexpected event in initialization"),
                    };
                });
            }
        }

        fs
    }

    pub fn read_events<F: FnMut(&mut FileSystem<T>, Event<T>)>(&mut self, mut callback: &mut F) {
        if !self.initial_events.is_empty() {
            for event in std::mem::replace(&mut self.initial_events, Vec::new()) {
                callback(self, event);
            }
        }

        let mut buf = [0u8; 4096];
        let events = match self.watcher.read_events(&mut buf) {
            Ok(events) => events,
            Err(e) => {
                error!("error reading from watcher: {}", e);
                return;
            }
        };

        for event in events {
            self.process(event, &mut callback);
        }
    }

    // handles inotify events and may produce Event(s) that are return upstream through sender
    fn process<F: FnMut(&mut FileSystem<T>, Event<T>)>(
        &mut self,
        event: WatchEvent,
        mut callback: &mut F,
    ) {
        Metrics::fs().increment_events();

        debug!("handling inotify event {:#?}", event);

        match event {
            WatchEvent::Create { wd, name } | WatchEvent::MovedTo { wd, name, .. } => {
                self.process_create(&wd, name, &mut callback);
            }
            WatchEvent::Modify { wd } => {
                self.process_modify(&wd, &mut callback);
            }
            WatchEvent::Delete { wd, name } | WatchEvent::MovedFrom { wd, name, .. } => {
                self.process_delete(&wd, name, &mut callback);
            }
            WatchEvent::Move {
                from_wd,
                from_name,
                to_wd,
                to_name,
            } => {
                // directories can't have hard links so we can expect just one entry for these watch
                // descriptors
                let from_path = match self.watch_descriptors.borrow().get(&from_wd) {
                    Some(entries) => {
                        if entries.is_empty() {
                            error!("got move event where from watch descriptors maps to no entries: {:?}", from_wd);
                            return;
                        }

                        let mut path = self.resolve_direct_path(unsafe { entries[0].as_ref() });
                        path.push(from_name.clone());
                        path
                    }
                    None => {
                        error!(
                            "got move event where from is an untracked watch descriptor: {:?}",
                            from_wd
                        );
                        return;
                    }
                };

                let to_path = match self.watch_descriptors.borrow().get(&to_wd) {
                    Some(entries) => {
                        if entries.is_empty() {
                            error!("got move event where to watch descriptors maps to no entries: {:?}", to_wd);
                            return;
                        }

                        let mut path = self.resolve_direct_path(unsafe { entries[0].as_ref() });
                        path.push(to_name.clone());
                        path
                    }
                    None => {
                        error!(
                            "got move event where to is an untracked watch descriptor: {:?}",
                            to_wd
                        );
                        return;
                    }
                };

                let is_to_path_ok = self.passes(to_path.to_str().unwrap());
                let is_from_path_ok = self.passes(from_path.to_str().unwrap());

                if is_to_path_ok && is_from_path_ok {
                    self.process_move(&from_wd, from_name, &to_wd, to_name, &mut callback);
                } else if is_to_path_ok {
                    self.process_create(&to_wd, to_name, &mut callback);
                } else if is_from_path_ok {
                    self.process_delete(&from_wd, from_name, &mut callback);
                }
            }
            WatchEvent::Overflow => panic!("overflowed kernel queue!"),
        };
    }

    fn process_create<F: FnMut(&mut FileSystem<T>, Event<T>)>(
        &mut self,
        watch_descriptor: &WatchDescriptor,
        name: OsString,
        callback: &mut F,
    ) {
        // directories can't be a hard link so we're guaranteed the watch descriptor maps to one
        // entry
        let entry_ptr = match self.watch_descriptors.borrow().get(watch_descriptor) {
            Some(entries) => entries[0],
            None => {
                error!(
                    "got create event for untracked watch descriptor: {:?}",
                    watch_descriptor
                );
                return;
            }
        };

        let entry = unsafe { entry_ptr.as_ref() };
        let mut path = self.resolve_direct_path(entry);
        path.push(name);

        if let Some(new_entry) = self.insert(&path, callback) {
            if let Entry::Dir { .. } = unsafe { new_entry.as_ref() } {
                for new_path in recursive_scan(&path) {
                    self.insert(&new_path, callback);
                }
            }
        }
    }

    fn process_modify<F: FnMut(&mut FileSystem<T>, Event<T>)>(
        &mut self,
        watch_descriptor: &WatchDescriptor,
        callback: &mut F,
    ) {
        if let Some(entries) = self
            .watch_descriptors
            .clone()
            .borrow()
            .get(watch_descriptor)
        {
            for entry_ptr in entries.iter() {
                callback(self, Event::Write(*entry_ptr));
            }
        } else {
            error!(
                "got modify event for untracked watch descriptor: {:?}",
                watch_descriptor
            );
        }
    }

    fn process_delete<F: FnMut(&mut FileSystem<T>, Event<T>)>(
        &mut self,
        watch_descriptor: &WatchDescriptor,
        name: OsString,
        callback: &mut F,
    ) {
        // directories can't be a hard link so we're guaranteed the watch descriptor maps to one
        // entry
        let entry_ptr = match self.watch_descriptors.borrow().get(watch_descriptor) {
            Some(entries) => entries[0],
            None => {
                error!(
                    "got delete event for untracked watch descriptor: {:?}",
                    watch_descriptor
                );
                return;
            }
        };

        let entry = unsafe { entry_ptr.as_ref() };
        let mut path = self.resolve_direct_path(entry);
        path.push(name);

        self.remove(&path, callback);
    }

    fn process_move<F: FnMut(&mut FileSystem<T>, Event<T>)>(
        &mut self,
        from_watch_descriptor: &WatchDescriptor,
        from_name: OsString,
        to_watch_descriptor: &WatchDescriptor,
        to_name: OsString,
        callback: &mut F,
    ) {
        // directories can't be a hard link so we're guaranteed the watch descriptor maps to one
        // entry
        let from_entry_ptr = match self.watch_descriptors.borrow().get(from_watch_descriptor) {
            Some(entries) => entries[0],
            None => {
                error!(
                    "got move event for untracked watch descriptor: {:?}",
                    from_watch_descriptor
                );
                return;
            }
        };
        let to_entry_ptr = match self.watch_descriptors.borrow().get(to_watch_descriptor) {
            Some(entries) => entries[0],
            None => {
                error!(
                    "got move event for untracked watch descriptor: {:?}",
                    to_watch_descriptor
                );
                return;
            }
        };

        let from_entry = unsafe { from_entry_ptr.as_ref() };
        let to_entry = unsafe { to_entry_ptr.as_ref() };

        let mut from_path = self.resolve_direct_path(from_entry);
        from_path.push(from_name);
        let mut to_path = self.resolve_direct_path(to_entry);
        to_path.push(to_name);

        // the entry is expected to exist
        self.rename(&from_path, &to_path, callback).unwrap();
    }

    pub fn resolve_direct_path(&self, mut entry: &Entry<T>) -> PathBuf {
        let mut components = Vec::new();

        loop {
            components.push(entry.name().to_str().unwrap().to_string());

            entry = match entry.parent() {
                Some(entry) => unsafe { &*entry.as_ptr() },
                None => break,
            }
        }

        components.reverse();
        components.into_iter().collect()
    }

    pub fn resolve_valid_paths(&self, entry: &Entry<T>) -> Vec<PathBuf> {
        let mut paths = Vec::new();
        self.resolve_valid_paths_helper(entry, &mut paths, Vec::new());
        paths
    }

    fn resolve_valid_paths_helper(
        &self,
        entry: &Entry<T>,
        paths: &mut Vec<PathBuf>,
        mut components: Vec<String>,
    ) {
        let symlinks = self.symlinks.clone();

        let mut base_components: Vec<String> = into_components(&self.resolve_direct_path(entry))
            .iter()
            .map(|x| x.to_str().unwrap().into())
            .collect(); // build all the components of the path leading up to the current entry
        base_components.append(&mut components); // add components already discovered from previous recursive step
        let path: PathBuf = base_components.iter().collect();
        if self.is_initial_dir_target(path.to_str().unwrap()) {
            // only want paths that fall in our watch window
            paths.push(path); // condense components of path that lead to the true entry into a PathBuf
        }

        let raw_components = base_components.as_slice();
        for i in 0..raw_components.len() - components.len() {
            // only need to iterate components up to current entry
            let current_path: PathBuf = raw_components[0..=i].to_vec().into_iter().collect();

            if let Some(symlinks) = symlinks.borrow().get(&current_path) {
                // check if path has a symlink to it
                let symlink_components = raw_components[(i + 1)..].to_vec();
                for symlink_ptr in symlinks.iter() {
                    let symlink = unsafe { symlink_ptr.as_ref() };
                    self.resolve_valid_paths_helper(symlink, paths, symlink_components.clone());
                }
            }
        }
    }

    fn insert<F: FnMut(&mut FileSystem<T>, Event<T>)>(
        &mut self,
        path: &PathBuf,
        callback: &mut F,
    ) -> Option<EntryPtr<T>> {
        if !self.passes(path.to_str().unwrap()) {
            info!("ignoring {:?}", path);
            return None;
        }

        if !(path.exists() || path.read_link().is_ok()) {
            warn!("attempted to insert non existant path {:?}", path);
            return None;
        }

        let parent = self.create_dir(&path.parent().unwrap().into())?;
        let parent = self.follow_links(parent)?;

        // We only need the last component, the parents are already inserted.
        let component = into_components(path).pop()?;

        // If the path is a dir, we can use create_dir to do the insert
        if path.is_dir() {
            return self.create_dir(path);
        }

        let children = unsafe { (&mut *parent.as_ptr()).children_mut().unwrap() };

        // Ok = symlink, Err = real path

        Some(match children.entry(component.clone()) {
            HashMapEntry::Occupied(v) => EntryPtr::from((*v.into_mut()).deref()),
            HashMapEntry::Vacant(v) => match path.read_link() {
                Ok(real) => {
                    let wd = match self.watcher.watch(path) {
                        Ok(wd) => wd,
                        Err(e) => {
                            error!("error watching {:?}: ", e);
                            return None;
                        }
                    };

                    let symlink = Box::new(Entry::Symlink {
                        name: component,
                        parent,
                        link: real.clone(),
                        wd,
                        rules: into_rules(real.clone()),
                    });

                    self.register(EntryPtr::from(symlink.deref()));

                    if self.insert(&real, callback).is_none() {
                        debug!(
                            "inserting symlink {:?} which points to invalid path {:?}",
                            path, real
                        );
                    }

                    callback(self, Event::New(EntryPtr::from(symlink.deref())));
                    EntryPtr::from((*v.insert(symlink)).deref())
                }
                Err(_) => {
                    let wd = match self.watcher.watch(path) {
                        Ok(wd) => wd,
                        Err(e) => {
                            error!("error watching {:?}: ", e);
                            return None;
                        }
                    };

                    let file = Box::new(Entry::File {
                        name: component,
                        parent,
                        wd,
                        data: T::default(),
                        file_handle: OpenOptions::new().read(true).open(path).unwrap(),
                    });

                    self.register(EntryPtr::from(file.deref()));

                    callback(self, Event::New(EntryPtr::from(file.deref())));
                    EntryPtr::from((*v.insert(file)).deref())
                }
            },
        })
    }

    fn register(&mut self, entry_ptr: EntryPtr<T>) {
        let entry = unsafe { entry_ptr.as_ref() };
        let path = self.resolve_direct_path(entry);

        self.watch_descriptors
            .borrow_mut()
            .entry(entry.watch_descriptor().clone())
            .or_insert(Vec::new())
            .push(entry_ptr);

        if let Entry::Symlink { link, .. } = entry {
            self.symlinks
                .borrow_mut()
                .entry(link.clone())
                .or_insert(Vec::new())
                .push(entry_ptr);
        }

        info!("watching {:?}", path);
    }

    fn unregister(&mut self, entry_ptr: EntryPtr<T>) {
        let entry = unsafe { entry_ptr.as_ref() };
        let path = self.resolve_direct_path(entry);

        let mut watch_descriptors = self.watch_descriptors.borrow_mut();

        let wd = entry.watch_descriptor().clone();
        let entries = match watch_descriptors.get_mut(&wd) {
            Some(v) => v,
            None => {
                error!("attempted to remove untracked watch descriptor {:?}", wd);
                return;
            }
        };

        entries.retain(|other| *other != entry_ptr);
        if entries.is_empty() {
            watch_descriptors.remove(&wd);
            match self.watcher.unwatch(wd) {
                _ => {}
            }; // TODO: Handle this error case
        }

        if let Entry::Symlink { link, .. } = entry {
            let mut symlinks = self.symlinks.borrow_mut();

            let entries = match symlinks.get_mut(link) {
                Some(v) => v,
                None => {
                    error!("attempted to remove untracked symlink {:?}", path);
                    return;
                }
            };

            entries.retain(|other| *other != entry_ptr);
            if entries.is_empty() {
                symlinks.remove(link);
            }
        }

        info!("unwatching {:?}", path);
    }

    fn remove<F: FnMut(&mut FileSystem<T>, Event<T>)>(
        &mut self,
        path: &PathBuf,
        callback: &mut F,
    ) -> Option<Box<Entry<T>>> {
        let mut parent = self.lookup(&path.parent()?.into())?;
        let component = into_components(path).pop()?;

        unsafe {
            parent
                .as_mut()
                .children_mut()
                .unwrap() // parents are always dirs
                .remove(&component)
                .map(|entry| {
                    self.drop_entry(EntryPtr::from(entry.deref()), callback);
                    entry
                })
        }
    }

    fn drop_entry<F: FnMut(&mut FileSystem<T>, Event<T>)>(
        &mut self,
        entry_ptr: EntryPtr<T>,
        callback: &mut F,
    ) {
        self.unregister(entry_ptr);
        let entry = unsafe { entry_ptr.as_ref() };
        match entry {
            Entry::Dir { children, .. } => {
                for (_, child) in children {
                    self.drop_entry(EntryPtr::from(child.deref()), callback);
                }
            }
            Entry::Symlink { ref link, .. } => {
                // This is a hacky way to check if there are any remaining
                // symlinks pointing to `link`
                if !self.passes(link.to_str().unwrap()) {
                    self.remove(&link, callback);
                }

                callback(self, Event::Delete(entry_ptr));
            }
            Entry::File { .. } => {
                callback(self, Event::Delete(entry_ptr));
            }
        };
    }

    // `from` is the path from where the file or dir used to live
    // `to is the path to where the file or dir now lives
    // e.g from = /var/log/syslog and to = /var/log/syslog.1.log
    fn rename<F: FnMut(&mut FileSystem<T>, Event<T>)>(
        &mut self,
        from: &PathBuf,
        to: &PathBuf,
        callback: &mut F,
    ) -> Option<EntryPtr<T>> {
        let mut new_parent = self.create_dir(&to.parent().unwrap().into()).unwrap();
        let entry = match self.lookup(from) {
            Some(entry) => unsafe { &mut *entry.as_ptr() },
            None => {
                return self.insert(to, callback);
            }
        };

        let new_name = into_components(to).pop()?;
        let old_name = entry.name().clone();

        let mut entry = unsafe {
            entry
                .parent()
                .expect("cannot move /")
                .as_mut()
                .children_mut()
                .expect("expected entry to be a drectory")
                .remove(&old_name)
                .unwrap()
        };

        entry.set_parent(new_parent.clone());
        entry.set_name(new_name.clone());
        let entry_ptr = EntryPtr::from(entry.deref());

        unsafe {
            new_parent
                .as_mut()
                .children_mut()
                .expect("expected entry to be a directory")
                .insert(new_name, entry);
        }

        Some(entry_ptr)
    }

    // Creates all entries for a directory.
    // If one of the entries already exists, it is skipped over.
    // The returns a linked list of all entries.
    fn create_dir(&mut self, path: &PathBuf) -> Option<EntryPtr<T>> {
        let mut entry = EntryPtr::from(self.root.deref());

        let components = into_components(path);

        // If the path has no parents return root as the parent.
        // This commonly occurs with paths in the filesystem root, e.g /some.file or /somedir
        if components.is_empty() {
            return Some(entry);
        }

        for (i, component) in components.iter().enumerate().skip(1) {
            let current_path = PathBuf::from_iter(&components[0..=i]);

            let children = unsafe {
                (&mut *entry.as_ptr())
                    .children_mut()
                    .expect("expected entry to be a directory")
            };

            let new_entry = match children.entry(component.clone()) {
                HashMapEntry::Occupied(v) => {
                    let entry_ref = v.into_mut().deref_mut();
                    match entry_ref {
                        Entry::Symlink { link, .. } => self.lookup(link)?,
                        _ => EntryPtr::from(entry_ref),
                    }
                }
                HashMapEntry::Vacant(v) => match current_path.read_link() {
                    Ok(real) => {
                        let wd = match self.watcher.watch(&current_path) {
                            Ok(wd) => wd,
                            Err(e) => {
                                error!("error watching {:?}: ", e);
                                return None;
                            }
                        };

                        let symlink = Box::new(Entry::Symlink {
                            name: component.clone(),
                            parent: entry,
                            link: real.clone(),
                            wd,
                            rules: into_rules(real.clone()),
                        });

                        self.register(EntryPtr::from(symlink.deref()));

                        v.insert(symlink);
                        match self.create_dir(&real) {
                            Some(v) => v,
                            None => {
                                panic!("unable to create symlink directory for entry {:?}", &real)
                            }
                        }
                    }
                    Err(_) => {
                        let wd = match self.watcher.watch(&current_path) {
                            Ok(wd) => wd,
                            Err(e) => {
                                error!("error watching {:?}: ", e);
                                return None;
                            }
                        };

                        let dir = Box::new(Entry::Dir {
                            name: component.clone(),
                            parent: Some(entry),
                            children: HashMap::new(),
                            wd,
                        });

                        self.register(NonNull::from(dir.deref()));
                        NonNull::from((*v.insert(dir)).deref())
                    }
                },
            };

            entry = new_entry;
        }

        Some(entry)
    }

    // Returns the entry that represents the supplied path.
    // If the path is not represented and therefor has no entry then None is return.
    pub fn lookup(&mut self, path: &PathBuf) -> Option<EntryPtr<T>> {
        let mut parent = EntryPtr::from(self.root.deref());
        let mut components = into_components(path);
        // remove the first component because it will always be the root
        components.remove(0);

        // If the path has no components there is nothing to look up.
        if components.is_empty() {
            return None;
        }

        let last_component = components.pop()?;

        for component in components {
            parent = unsafe {
                self.follow_links(parent)?
                    .as_mut()
                    .children_mut()
                    .expect("expected directory entry")
                    .get_mut(&component)
                    .map(|entry| EntryPtr::from((*entry).deref()))
                    .and_then(|entry| self.follow_links(entry))?
            };
        }

        unsafe {
            parent
                .as_mut()
                .children_mut()
                .expect("expected directory entry")
                .get_mut(&last_component)
                .map(|entry| EntryPtr::from((*entry).deref()))
        }
    }

    fn follow_links(&mut self, mut entry: EntryPtr<T>) -> Option<EntryPtr<T>> {
        unsafe {
            while let Some(link) = entry.as_mut().link() {
                entry = self.lookup(link)?;
            }
            Some(entry)
        }
    }

    fn is_symlink_target(&self, path: &str) -> bool {
        for (_, symlink_ptrs) in self.symlinks.borrow().iter() {
            for symlink_ptr in symlink_ptrs.iter() {
                let symlink = unsafe { (*symlink_ptr).as_ref() };
                match symlink {
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
            }
        }

        false
    }

    fn is_initial_dir_target(&self, path: &str) -> bool {
        if let Status::Ok = self.initial_dir_rules.passes(path) {
            if let Status::Ok = self.master_rules.passes(path) {
                return true;
            }
        }

        false
    }

    // a helper for checking if a path passes exclusion/inclusion rules
    fn passes(&self, path: &str) -> bool {
        self.is_initial_dir_target(path) || self.is_symlink_target(path)
    }
}

// conditionally implement std::fmt::Debug if the underlying type T implements it
impl<T: fmt::Debug> fmt::Debug for FileSystem<T> {
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
            Component::Normal(path) => Some(path.to_os_string()),
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
    use std::fs::{copy, create_dir, hard_link, remove_dir_all, remove_file, rename, File};
    use std::os::unix::fs::symlink;
    use std::panic;
    use tempdir::TempDir;

    lazy_static! {
        static ref LOGGER: () = env_logger::init();
    }

    fn new_fs<T: Default>(path: PathBuf, rules: Option<Rules>) -> FileSystem<T> {
        let rules = rules.unwrap_or_else(|| {
            let mut rules = Rules::new();
            rules.add_inclusion(GlobRule::new(r"**").unwrap());
            rules
        });
        FileSystem::new(vec![path], rules)
    }

    fn run_test<T: FnOnce() -> () + panic::UnwindSafe>(test: T) {
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
            let tempdir = TempDir::new("filesystem").unwrap();
            let path = tempdir.path().to_path_buf();

            let mut fs = new_fs::<()>(path.clone(), None);

            let a = path.join("a");
            File::create(&a).unwrap();

            fs.read_events(&mut |_, _| {});

            let entry = fs.lookup(&a);
            assert!(entry.is_some());
            match unsafe { entry.unwrap().as_ref() } {
                Entry::File { .. } => {}
                _ => panic!("wrong entry type"),
            }

            let old = path.join("a.old");
            rename(&a, &old).unwrap();

            fs.read_events(&mut |_, _| {});

            let entry = fs.lookup(&a);
            assert!(entry.is_none());

            let entry = fs.lookup(&old);
            assert!(entry.is_some());
            match unsafe { entry.unwrap().as_ref() } {
                Entry::File { .. } => {}
                _ => panic!("wrong entry type"),
            }

            File::create(&a).unwrap();

            fs.read_events(&mut |_, _| {});

            let entry = fs.lookup(&a);
            assert!(entry.is_some());
            match unsafe { entry.unwrap().as_ref() } {
                Entry::File { .. } => {}
                _ => panic!("wrong entry type"),
            }
        });
    }

    // Simulates the `create_copy` log rotation strategy
    #[test]
    fn filesystem_rotate_create_copy() {
        run_test(|| {
            let tempdir = TempDir::new("filesystem").unwrap();
            let path = tempdir.path().to_path_buf();

            let mut fs = new_fs::<()>(path.clone(), None);

            let a = path.join("a");
            File::create(&a).unwrap();

            fs.read_events(&mut |_, _| {});

            let entry = fs.lookup(&a);
            assert!(entry.is_some());
            match unsafe { entry.unwrap().as_ref() } {
                Entry::File { .. } => {}
                _ => panic!("wrong entry type"),
            }

            let old = path.join("a.old");
            copy(&a, &old).unwrap();
            remove_file(&a).unwrap();

            fs.read_events(&mut |_, _| {});

            let entry = fs.lookup(&a);
            assert!(entry.is_none());

            let entry = fs.lookup(&old);
            assert!(entry.is_some());
            match unsafe { entry.unwrap().as_ref() } {
                Entry::File { .. } => {}
                _ => panic!("wrong entry type"),
            }

            File::create(&a).unwrap();

            fs.read_events(&mut |_, _| {});

            let entry = fs.lookup(&a);
            assert!(entry.is_some());
            match unsafe { entry.unwrap().as_ref() } {
                Entry::File { .. } => {}
                _ => panic!("wrong entry type"),
            }
        });
    }

    // Creates a plain old dir
    #[test]
    fn filesystem_create_dir() {
        run_test(|| {
            let tempdir = TempDir::new("filesystem").unwrap();
            let path = tempdir.path().to_path_buf();

            let mut fs = new_fs::<()>(path.clone(), None);

            fs.read_events(&mut |_, _| {});

            assert!(fs.lookup(&path).is_some());
        });
    }

    // Creates a plain old file
    #[test]
    fn filesystem_create_file() {
        run_test(|| {
            let tempdir = TempDir::new("filesystem").unwrap();
            let path = tempdir.path().to_path_buf();

            let mut fs = new_fs::<()>(path.clone(), None);

            File::create(path.join("insert.log")).unwrap();
            fs.read_events(&mut |_, _| {});

            assert!(fs.lookup(&path.join("insert.log")).is_some());
        });
    }

    // Creates a symlink
    #[test]
    fn filesystem_create_symlink() {
        run_test(|| {
            let tempdir = TempDir::new("filesystem").unwrap();
            let path = tempdir.path().to_path_buf();

            let mut fs = new_fs::<()>(path.clone(), None);

            let a = path.join("a");
            let b = path.join("b");
            create_dir(&a).unwrap();
            symlink(&a, &b).unwrap();

            fs.read_events(&mut |_, _| {});

            let entry = fs.lookup(&a);
            assert!(entry.is_some());
            match unsafe { entry.unwrap().as_ref() } {
                Entry::Dir { .. } => {}
                _ => panic!("wrong entry type"),
            }

            let entry = fs.lookup(&b);
            assert!(entry.is_some());
            match unsafe { entry.unwrap().as_ref() } {
                Entry::Symlink { link, .. } => {
                    assert_eq!(*link, a);
                }
                _ => panic!("wrong entry type"),
            }
        });
    }

    // Creates a hardlink
    #[test]
    fn filesystem_create_hardlink() {
        run_test(|| {
            let tempdir = TempDir::new("filesystem").unwrap();
            let path = tempdir.path().to_path_buf();

            let mut fs = new_fs::<()>(path.clone(), None);

            let file_path = path.join("insert.log");
            let hard_path = path.join("hard.log");
            File::create(file_path.clone()).unwrap();
            hard_link(&file_path, &hard_path).unwrap();

            fs.read_events(&mut |_, _| {});

            let entry = fs.lookup(&file_path).unwrap();
            let real_watch_descriptor;
            match unsafe { entry.as_ref() } {
                Entry::File { wd, .. } => {
                    real_watch_descriptor = wd;
                }
                _ => panic!("wrong entry type"),
            }

            let entry = fs.lookup(&hard_path);
            assert!(entry.is_some());
            match unsafe { entry.unwrap().as_ref() } {
                Entry::File { wd, .. } => assert_eq!(wd, real_watch_descriptor),
                _ => panic!("wrong entry type"),
            }
        });
    }

    // Deletes a directory
    #[test]
    fn filesystem_delete_filled_dir() {
        run_test(|| {
            let tempdir = TempDir::new("filesystem").unwrap();
            let path = tempdir.path().to_path_buf();

            let file_path = path.join("file.log");
            let sym_path = path.join("sym.log");
            let hard_path = path.join("hard.log");
            File::create(file_path.clone()).unwrap();
            symlink(&file_path, &sym_path).unwrap();
            hard_link(&file_path, &hard_path).unwrap();

            let mut fs = new_fs::<()>(path.clone(), None);

            assert!(fs.lookup(&path).is_some());
            assert!(fs.lookup(&file_path).is_some());
            assert!(fs.lookup(&sym_path).is_some());
            assert!(fs.lookup(&hard_path).is_some());

            tempdir.close().unwrap();
            fs.read_events(&mut |_, _| {});

            assert!(fs.lookup(&path).is_none());
            assert!(fs.lookup(&file_path).is_none());
            assert!(fs.lookup(&sym_path).is_none());
            assert!(fs.lookup(&hard_path).is_none());
        });
    }

    // Deletes a file
    #[test]
    fn filesystem_delete_file() {
        run_test(|| {
            let tempdir = TempDir::new("filesystem").unwrap();
            let path = tempdir.path().to_path_buf();

            let file_path = path.join("file");
            File::create(file_path.clone()).unwrap();

            let mut fs = new_fs::<()>(path, None);

            assert!(fs.lookup(&file_path).is_some());

            remove_file(&file_path).unwrap();
            fs.read_events(&mut |_, _| {});

            assert!(fs.lookup(&file_path).is_none());
        });
    }

    // Deletes a symlink
    #[test]
    fn filesystem_delete_symlink() {
        run_test(|| {
            let tempdir = TempDir::new("filesystem").unwrap();
            let path = tempdir.path().to_path_buf();

            let a = path.join("a");
            let b = path.join("b");
            create_dir(&a).unwrap();
            symlink(&a, &b).unwrap();

            let mut fs = new_fs::<()>(path, None);

            remove_dir_all(&b).unwrap();
            fs.read_events(&mut |_, _| {});

            assert!(fs.lookup(&a).is_some());
            assert!(fs.lookup(&b).is_none());
        });
    }

    // Deletes the pointee of a symlink
    #[test]
    fn filesystem_delete_symlink_pointee() {
        run_test(|| {
            let tempdir = TempDir::new("filesystem").unwrap();
            let path = tempdir.path().to_path_buf();

            let a = path.join("a");
            let b = path.join("b");
            create_dir(&a).unwrap();
            symlink(&a, &b).unwrap();

            let mut fs = new_fs::<()>(path, None);

            remove_dir_all(&a).unwrap();
            fs.read_events(&mut |_, _| {});

            assert!(fs.lookup(&a).is_none());
            assert!(fs.lookup(&b).is_some());
        });
    }

    // Deletes a hardlink
    #[test]
    fn filesystem_delete_hardlink() {
        run_test(|| {
            let tempdir = TempDir::new("filesystem").unwrap();
            let path = tempdir.path().to_path_buf();

            let a = path.join("a");
            let b = path.join("b");
            File::create(a.clone()).unwrap();
            hard_link(&a, &b).unwrap();

            let mut fs = new_fs::<()>(path, None);

            assert!(fs.lookup(&a).is_some());
            assert!(fs.lookup(&b).is_some());

            remove_file(&b).unwrap();
            fs.read_events(&mut |_, _| {});

            assert!(fs.lookup(&a).is_some());
            assert!(fs.lookup(&b).is_none());
        });
    }

    // Deletes the pointee of a hardlink (not totally accurate since we're not deleting the inode
    // entry, but what evs)
    #[test]
    fn filesystem_delete_hardlink_pointee() {
        run_test(|| {
            let tempdir = TempDir::new("filesystem").unwrap();
            let path = tempdir.path().to_path_buf();

            let a = path.join("a");
            let b = path.join("b");
            File::create(a.clone()).unwrap();
            hard_link(&a, &b).unwrap();

            let mut fs = new_fs::<()>(path, None);

            remove_file(&a).unwrap();
            fs.read_events(&mut |_, _| {});

            assert!(fs.lookup(&a).is_none());
            assert!(fs.lookup(&b).is_some());
        });
    }

    // Moves a directory within the watched directory
    #[test]
    fn filesystem_move_dir_internal() {
        run_test(|| {
            let tempdir = TempDir::new("filesystem").unwrap();
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

            let mut fs = new_fs::<()>(path, None);

            rename(&old_dir_path, &new_dir_path).unwrap();
            fs.read_events(&mut |_, _| {});

            assert!(fs.lookup(&old_dir_path).is_none());
            assert!(fs.lookup(&file_path).is_none());
            assert!(fs.lookup(&sym_path).is_none());
            assert!(fs.lookup(&hard_path).is_none());

            let entry = fs.lookup(&new_dir_path);
            assert!(entry.is_some());
            match unsafe { entry.unwrap().as_ref() } {
                Entry::Dir { .. } => {}
                _ => panic!("wrong entry type"),
            }

            let entry = fs.lookup(&new_dir_path.join("file.log"));
            assert!(entry.is_some());
            match unsafe { entry.unwrap().as_ref() } {
                Entry::File { .. } => {}
                _ => panic!("wrong entry type"),
            }

            let entry = fs.lookup(&new_dir_path.join("hard.log"));
            assert!(entry.is_some());
            match unsafe { entry.unwrap().as_ref() } {
                Entry::File { .. } => {}
                _ => panic!("wrong entry type"),
            }

            let entry = fs.lookup(&new_dir_path.join("sym.log"));
            assert!(entry.is_some());
            match unsafe { entry.unwrap().as_ref() } {
                Entry::Symlink { link, .. } => {
                    // symlinks don't update so this link is bad
                    assert_eq!(*link, file_path);
                }
                _ => panic!("wrong entry type"),
            }
        });
    }

    // Moves a directory out
    #[test]
    fn filesystem_move_dir_out() {
        run_test(|| {
            let tempdir = TempDir::new("filesystem").unwrap();
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

            let mut fs = new_fs::<()>(old_dir_path.clone(), None);

            rename(&old_dir_path, &new_dir_path).unwrap();
            fs.read_events(&mut |_, _| {});

            assert!(fs.lookup(&new_dir_path).is_none());
            assert!(fs.lookup(&new_dir_path.join("file.log")).is_none());
            assert!(fs.lookup(&new_dir_path.join("hard.log")).is_none());
            assert!(fs.lookup(&new_dir_path.join("sym.log")).is_none());
        });
    }

    // Moves a directory in
    #[test]
    fn filesystem_move_dir_in() {
        run_test(|| {
            let tempdir = TempDir::new("filesystem").unwrap();
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

            let mut fs = new_fs::<()>(new_dir_path.clone(), None);

            assert!(fs.lookup(&old_dir_path).is_none());
            assert!(fs.lookup(&new_dir_path).is_none());
            assert!(fs.lookup(&file_path).is_none());
            assert!(fs.lookup(&sym_path).is_none());
            assert!(fs.lookup(&hard_path).is_none());

            rename(&old_dir_path, &new_dir_path).unwrap();
            fs.read_events(&mut |_, _| {});

            let entry = fs.lookup(&new_dir_path);
            assert!(entry.is_some());
            match unsafe { entry.unwrap().as_ref() } {
                Entry::Dir { .. } => {}
                _ => panic!("wrong entry type"),
            }

            let entry = fs.lookup(&new_dir_path.join("file.log"));
            assert!(entry.is_some());
            match unsafe { entry.unwrap().as_ref() } {
                Entry::File { .. } => {}
                _ => panic!("wrong entry type"),
            }

            let entry = fs.lookup(&new_dir_path.join("hard.log"));
            assert!(entry.is_some());
            match unsafe { entry.unwrap().as_ref() } {
                Entry::File { .. } => {}
                _ => panic!("wrong entry type"),
            }

            let entry = fs.lookup(&new_dir_path.join("sym.log"));
            assert!(entry.is_some());
            match unsafe { entry.unwrap().as_ref() } {
                Entry::Symlink { link, .. } => {
                    // symlinks don't update so this link is bad
                    assert_eq!(*link, file_path);
                }
                _ => panic!("wrong entry type"),
            }
        });
    }

    // Moves a file within the watched directory
    #[test]
    fn filesystem_move_file_internal() {
        run_test(|| {
            let tempdir = TempDir::new("filesystem").unwrap();
            let path = tempdir.path().to_path_buf();

            let mut fs = new_fs::<()>(path.clone(), None);

            let file_path = path.join("insert.log");
            let new_path = path.join("new.log");
            File::create(file_path.clone()).unwrap();
            rename(&file_path, &new_path).unwrap();

            fs.read_events(&mut |_, _| {});

            let entry = fs.lookup(&file_path);
            assert!(entry.is_none());

            let entry = fs.lookup(&new_path);
            assert!(entry.is_some());
            match unsafe { entry.unwrap().as_ref() } {
                Entry::File { .. } => {}
                _ => panic!("wrong entry type"),
            }
        });
    }

    // Moves a file out of the watched directory
    #[test]
    fn filesystem_move_file_out() {
        run_test(|| {
            let tempdir = TempDir::new("filesystem").unwrap();
            let path = tempdir.path().to_path_buf();

            let watch_path = path.join("watch");
            let other_path = path.join("other");
            create_dir(&watch_path).unwrap();
            create_dir(&other_path).unwrap();

            let file_path = watch_path.join("inside.log");
            let move_path = other_path.join("outside.log");
            File::create(file_path.clone()).unwrap();

            let mut fs = new_fs::<()>(watch_path, None);

            rename(&file_path, &move_path).unwrap();

            fs.read_events(&mut |_, _| {});

            let entry = fs.lookup(&file_path);
            assert!(entry.is_none());

            let entry = fs.lookup(&move_path);
            assert!(entry.is_none());
        });
    }

    // Moves a file into the watched directory
    #[test]
    fn filesystem_move_file_in() {
        run_test(|| {
            let tempdir = TempDir::new("filesystem").unwrap();
            let path = tempdir.path().to_path_buf();

            let watch_path = path.join("watch");
            let other_path = path.join("other");
            create_dir(&watch_path).unwrap();
            create_dir(&other_path).unwrap();

            let file_path = other_path.join("inside.log");
            let move_path = watch_path.join("outside.log");
            File::create(file_path.clone()).unwrap();

            let mut fs = new_fs::<()>(watch_path, None);

            rename(&file_path, &move_path).unwrap();
            File::create(file_path.clone()).unwrap();

            fs.read_events(&mut |_, _| {});

            let entry = fs.lookup(&file_path);
            assert!(entry.is_none());

            let entry = fs.lookup(&move_path);
            assert!(entry.is_some());
            match unsafe { entry.unwrap().as_ref() } {
                Entry::File { .. } => {}
                _ => panic!("wrong entry type"),
            }
        });
    }

    // Moves a file out of the watched directory
    #[test]
    fn filesystem_move_symlink_file_out() {
        run_test(|| {
            let tempdir = TempDir::new("filesystem").unwrap();
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

            let mut fs = new_fs::<()>(watch_path, None);

            rename(&file_path, &move_path).unwrap();

            fs.read_events(&mut |_, _| {});

            let entry = fs.lookup(&sym_path);
            assert!(entry.is_some());

            let entry = fs.lookup(&file_path);
            assert!(entry.is_none());

            let entry = fs.lookup(&move_path);
            assert!(entry.is_none());
        });
    }

    // Watch symlink target that is excluded
    #[test]
    fn filesystem_watch_symlink_w_excluded_target() {
        run_test(|| {
            let tempdir = TempDir::new("filesystem").unwrap();
            let path = tempdir.path().to_path_buf();

            let mut rules = Rules::new();
            rules.add_inclusion(GlobRule::new("*.log").unwrap());
            rules.add_inclusion(GlobRule::new("/tmp/filesystem.*").unwrap());
            rules.add_exclusion(GlobRule::new("*.tmp").unwrap());

            let file_path = path.join("test.tmp");
            let sym_path = path.join("test.log");
            File::create(file_path.clone()).unwrap();

            let mut fs = new_fs::<()>(path, Some(rules));

            let entry = fs.lookup(&file_path);
            assert!(entry.is_none());

            symlink(&file_path, &sym_path).unwrap();

            fs.read_events(&mut |_, _| {});

            let entry = fs.lookup(&sym_path);
            assert!(entry.is_some());

            let entry = fs.lookup(&file_path);
            assert!(entry.is_some());
        });
    }

    #[test]
    fn filesystem_resolve_valid_paths() {
        run_test(|| {
            let tempdir = TempDir::new("filesystem").unwrap();
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

            let mut fs = new_fs::<()>(path, None);

            let entry = unsafe { &*(fs.lookup(&file_path).unwrap()).as_ptr() };
            let resolved_paths = fs.resolve_valid_paths(entry);

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

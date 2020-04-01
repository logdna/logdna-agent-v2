use crate::rule::{GlobRule, Rules, Status};
use hashbrown::hash_map::Entry as HashMapEntry;
use hashbrown::HashMap;
use inotify::{EventMask, Inotify, WatchDescriptor, WatchMask};
use metrics::Metrics;
use std::cell::RefCell;
use std::ffi::OsString;
use std::fmt;
use std::fs::read_dir;
use std::fs::{File, OpenOptions};
use std::ops::DerefMut;
use std::path::{Component, PathBuf};
use std::ptr;
use std::rc::Rc;

type Children<T> = HashMap<OsString, Box<Entry<T>>>;
type Symlinks<T> = Rc<RefCell<HashMap<PathBuf, Vec<*mut Entry<T>>>>>;
type WatchDescriptors<T> = Rc<RefCell<HashMap<WatchDescriptor, Vec<*mut Entry<T>>>>>;

macro_rules! deref {
    ($e:expr) => {
        match unsafe { $e.as_ref() } {
            Some(v) => v,
            None => panic!("entry pointer {:?} points to invalid location", $e),
        }
    };
}

macro_rules! deref_mut {
    ($e:expr) => {
        match unsafe { $e.as_mut() } {
            Some(v) => v,
            None => panic!("entry pointer {:?} points to invalid location", $e),
        }
    };
}

#[derive(Debug)]
pub enum Entry<T> {
    File {
        name: OsString,
        parent: *mut Entry<T>,
        watch_descriptor: Option<WatchDescriptor>,
        data: Option<T>,
        file_handle: File,
    },
    Dir {
        name: OsString,
        parent: *mut Entry<T>,
        children: Children<T>,
        watch_descriptor: Option<WatchDescriptor>,
    },
    Symlink {
        name: OsString,
        parent: *mut Entry<T>,
        link: PathBuf,
        watch_descriptor: Option<WatchDescriptor>,
        rules: Rules,
    },
}

impl<T> Entry<T> {
    fn name(&self) -> &OsString {
        match self {
            Entry::File { name, .. } | Entry::Dir { name, .. } | Entry::Symlink { name, .. } => {
                name
            }
        }
    }

    fn parent(&self) -> *mut Entry<T> {
        match self {
            Entry::File { parent, .. }
            | Entry::Dir { parent, .. }
            | Entry::Symlink { parent, .. } => *parent,
        }
    }

    fn set_name(&mut self, new_name: OsString) {
        match self {
            Entry::File { name, .. } | Entry::Dir { name, .. } | Entry::Symlink { name, .. } => {
                *name = new_name
            }
        }
    }

    fn set_parent(&mut self, new_parent: *mut Entry<T>) {
        match self {
            Entry::File { parent, .. }
            | Entry::Dir { parent, .. }
            | Entry::Symlink { parent, .. } => *parent = new_parent,
        }
    }

    fn link(&self) -> Option<&PathBuf> {
        match self {
            Entry::Symlink { link, .. } => Some(link),
            _ => None,
        }
    }

    fn children(&self) -> Option<&Children<T>> {
        match self {
            Entry::Dir { children, .. } => Some(children),
            _ => None,
        }
    }

    fn children_mut(&mut self) -> Option<&mut Children<T>> {
        match self {
            Entry::Dir { children, .. } => Some(children),
            _ => None,
        }
    }

    fn watch_descriptor(&self) -> Option<&WatchDescriptor> {
        match self {
            Entry::Dir {
                watch_descriptor, ..
            }
            | Entry::Symlink {
                watch_descriptor, ..
            }
            | Entry::File {
                watch_descriptor, ..
            } => watch_descriptor.as_ref(),
        }
    }

    fn watch_descriptor_mut(&mut self) -> Option<&mut WatchDescriptor> {
        match self {
            Entry::Dir {
                watch_descriptor, ..
            }
            | Entry::Symlink {
                watch_descriptor, ..
            }
            | Entry::File {
                watch_descriptor, ..
            } => watch_descriptor.as_mut(),
        }
    }

    fn set_watch_descriptor(&mut self, new_watch_descriptor: Option<WatchDescriptor>) {
        match self {
            Entry::Dir {
                watch_descriptor, ..
            }
            | Entry::Symlink {
                watch_descriptor, ..
            }
            | Entry::File {
                watch_descriptor, ..
            } => {
                *watch_descriptor = new_watch_descriptor;
            }
        }
    }

    pub fn data(&self) -> Option<&T> {
        match self {
            Entry::Dir { .. } | Entry::Symlink { .. } => None,
            Entry::File { data, .. } => data.as_ref(),
        }
    }

    pub fn data_mut(&mut self) -> Option<&mut T> {
        match self {
            Entry::Dir { .. } | Entry::Symlink { .. } => None,
            Entry::File { data, .. } => data.as_mut(),
        }
    }

    pub fn file_handle(&self) -> Option<&File> {
        match self {
            Entry::Dir { .. } | Entry::Symlink { .. } => None,
            Entry::File { file_handle, .. } => Some(file_handle),
        }
    }

    pub fn set_data(&mut self, new_data: Option<T>) {
        if let Entry::File { data, .. } = self {
            *data = new_data;
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum InotifyProxyEvent {
    Create {
        wd: WatchDescriptor,
        name: OsString,
    },
    Modify {
        wd: WatchDescriptor,
    },
    Delete {
        wd: WatchDescriptor,
        name: OsString,
    },
    Move {
        from_wd: WatchDescriptor,
        from_name: OsString,
        to_wd: WatchDescriptor,
        to_name: OsString,
    },
    MovedFrom {
        wd: WatchDescriptor,
        name: OsString,
        cookie: u32,
    },
    MovedTo {
        wd: WatchDescriptor,
        name: OsString,
        cookie: u32,
    },
    Overflow,
}

/// Represents a filesystem event
#[derive(Debug)]
pub enum Event<T> {
    /// A file was created initialized
    Initialize(*mut Entry<T>),
    /// A new file was created
    New(*mut Entry<T>),
    /// A file was written too
    Write(*mut Entry<T>),
    /// A file was deleted
    Delete(*mut Entry<T>),
}

pub struct FileSystem<T> {
    root: Box<Entry<T>>,
    symlinks: Symlinks<T>,
    watched_dirs: Vec<PathBuf>,
    watch_descriptors: WatchDescriptors<T>,
    master_rules: Rules,
    initial_dir_rules: Rules,
    inotify: Inotify,
    initial_events: Vec<Event<T>>,
}

impl<T> FileSystem<T> {
    pub fn new(watched_dirs: Vec<PathBuf>, rules: Rules) -> Self {
        let mut root = Box::new(Entry::Dir {
            name: "/".into(),
            parent: ptr::null_mut(),
            children: Children::new(),
            watch_descriptor: None,
        });
        // we have to do this because we want to use fs.create_watch, but that needs a mutable
        // reference to the root node, but we move that node into the file system. to get around
        // this we hack around the borrow check by converting a mutable reference to a mutable
        // pointer and back into a mutable reference
        let root_ref = deref_mut!(root.deref_mut() as *mut Entry<T>);
        let mut initial_dir_rules = Rules::new();
        for path in watched_dirs.iter() {
            append_rules(&mut initial_dir_rules, path.clone());
        }

        let mut fs = Self {
            root,
            symlinks: Symlinks::new(RefCell::new(HashMap::new())),
            watched_dirs,
            watch_descriptors: WatchDescriptors::new(RefCell::new(HashMap::new())),
            master_rules: rules,
            initial_dir_rules,
            inotify: match Inotify::init() {
                Ok(v) => v,
                Err(e) => panic!("unable to initialize inotify: {:?}", e),
            },
            initial_events: Vec::new(),
        };

        fs.create_watch(&PathBuf::from(r"/"), root_ref);

        for dir in fs.watched_dirs.clone().iter() {
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
        // TODO: Review this blocking, currently assuming there's only every 1 source
        let events = match self.inotify.read_events(&mut buf) {
            Ok(events) => events.peekable(),
            Err(e) => {
                error!("error reading from inotify fd: {}", e);
                return;
            }
        };

        let mut final_events: Vec<InotifyProxyEvent> = Vec::new();
        for event in events {
            if event.mask.contains(EventMask::MOVED_FROM) {
                let mut found_match = false;
                for final_event in final_events.iter_mut() {
                    match final_event {
                        InotifyProxyEvent::MovedTo { wd, name, cookie } => {
                            if *cookie == event.cookie {
                                *final_event = InotifyProxyEvent::Move {
                                    from_wd: event.wd.clone(),
                                    from_name: event.name.unwrap().to_os_string(),
                                    to_wd: wd.clone(),
                                    to_name: name.clone(),
                                };
                                found_match = true;
                                break;
                            }
                        }
                        _ => continue,
                    };
                }

                if !found_match {
                    final_events.push(InotifyProxyEvent::MovedFrom {
                        wd: event.wd.clone(),
                        name: event.name.unwrap().to_os_string(),
                        cookie: event.cookie,
                    });
                }
            } else if event.mask.contains(EventMask::MOVED_TO) {
                let mut found_match = false;
                for final_event in final_events.iter_mut() {
                    match final_event {
                        InotifyProxyEvent::MovedFrom { wd, name, cookie } => {
                            if *cookie == event.cookie {
                                *final_event = InotifyProxyEvent::Move {
                                    from_wd: wd.clone(),
                                    from_name: name.clone(),
                                    to_wd: event.wd.clone(),
                                    to_name: event.name.unwrap().to_os_string(),
                                };
                                found_match = true;
                                break;
                            }
                        }
                        _ => continue,
                    };
                }

                if !found_match {
                    final_events.push(InotifyProxyEvent::MovedTo {
                        wd: event.wd.clone(),
                        name: event.name.unwrap().to_os_string(),
                        cookie: event.cookie,
                    });
                }
            } else if event.mask.contains(EventMask::CREATE) {
                final_events.push(InotifyProxyEvent::Create {
                    wd: event.wd.clone(),
                    name: event.name.unwrap().to_os_string(),
                });
            } else if event.mask.contains(EventMask::DELETE) {
                final_events.push(InotifyProxyEvent::Delete {
                    wd: event.wd.clone(),
                    name: event.name.unwrap().to_os_string(),
                });
            } else if event.mask.contains(EventMask::MODIFY) {
                final_events.push(InotifyProxyEvent::Modify {
                    wd: event.wd.clone(),
                });
            } else if event.mask.contains(EventMask::Q_OVERFLOW) {
                final_events.push(InotifyProxyEvent::Overflow);
            }
        }

        for final_event in final_events {
            let event = match final_event {
                InotifyProxyEvent::MovedTo { wd, name, .. } => InotifyProxyEvent::Create {
                    wd: wd.clone(),
                    name: name.clone(),
                },
                InotifyProxyEvent::MovedFrom { wd, name, .. } => InotifyProxyEvent::Delete {
                    wd: wd.clone(),
                    name: name.clone(),
                },
                _ => final_event,
            };
            self.process(event, &mut callback);
        }
    }

    // handles inotify events and may produce Event(s) that are return upstream through sender
    fn process<F: FnMut(&mut FileSystem<T>, Event<T>)>(
        &mut self,
        event: InotifyProxyEvent,
        mut callback: &mut F,
    ) {
        Metrics::fs().increment_events();

        debug!("handling inotify event {:#?}", event);

        match event {
            InotifyProxyEvent::Create { wd, name } => {
                self.process_create(&wd, name, &mut callback);
            }
            InotifyProxyEvent::Modify { wd } => {
                self.process_modify(&wd, &mut callback);
            }
            InotifyProxyEvent::Delete { wd, name } => {
                self.process_delete(&wd, name, &mut callback);
            }
            InotifyProxyEvent::Move {
                from_wd,
                from_name,
                to_wd,
                to_name,
            } => {
                // directories can't have hard links so we can expect just one entry for these watch
                // descriptors
                let from_path = match self.watch_descriptors.borrow().get(&from_wd) {
                    Some(entries) => {
                        if entries.len() == 0 {
                            error!("got move event where from watch descriptors maps to no entries: {:?}", from_wd);
                            return;
                        }

                        let mut path = self.resolve_direct_path(deref!(entries[0]));
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
                        if entries.len() == 0 {
                            error!("got move event where to watch descriptors maps to no entries: {:?}", to_wd);
                            return;
                        }

                        let mut path = self.resolve_direct_path(deref!(entries[0]));
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
            InotifyProxyEvent::Overflow => panic!("overflowed kernel queue!"),
            _ => {
                error!("got unexpected proxy event: {:?}", event);
            }
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
            Some(entries) => entries[0].clone(),
            None => {
                error!(
                    "got create event for untracked watch descriptor: {:?}",
                    watch_descriptor
                );
                return;
            }
        };

        let entry = deref!(entry_ptr);
        let mut path = self.resolve_direct_path(entry).clone();
        path.push(name.clone());

        if let Some(new_entry) = self.insert(&path, callback) {
            if let Entry::Dir { .. } = deref!(new_entry) {
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
                callback(self, Event::Write(entry_ptr.clone()));
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
            Some(entries) => entries[0].clone(),
            None => {
                error!(
                    "got delete event for untracked watch descriptor: {:?}",
                    watch_descriptor
                );
                return;
            }
        };

        let entry = deref!(entry_ptr);
        let mut path = self.resolve_direct_path(entry).clone();
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

        let from_entry = deref!(from_entry_ptr);
        let to_entry = deref!(to_entry_ptr);

        let mut from_path = self.resolve_direct_path(from_entry).clone();
        from_path.push(from_name);
        let mut to_path = self.resolve_direct_path(to_entry).clone();
        to_path.push(to_name);

        // the entry is expected to exist
        self.rename(&from_path, &to_path, callback).unwrap();
    }

    pub fn resolve_direct_path(&self, mut entry: &Entry<T>) -> PathBuf {
        let mut components = Vec::new();

        loop {
            components.push(entry.name().to_str().unwrap().to_string());

            entry = match unsafe { entry.parent().as_ref() } {
                Some(entry) => entry,
                None => break,
            }
        }

        components.reverse();
        components.into_iter().collect()
    }

    pub fn resolve_all_paths(&self, entry: &Entry<T>) -> Vec<PathBuf> {
        let mut resolved_paths = Vec::new();

        let symlinks = self.symlinks.clone();
        let mut path_builders = Vec::new();
        path_builders.push((Vec::new(), entry));
        loop {
            let (mut components, entry) = match path_builders.pop() {
                Some(tuple) => tuple,
                None => break,
            };

            components.reverse();
            let mut base_components: Vec<String> =
                into_components(&self.resolve_direct_path(entry))
                    .iter()
                    .map(|x| x.to_str().unwrap().into())
                    .collect();
            base_components.append(&mut components);

            if base_components.len() > 3 {
                let raw_components = base_components.as_slice();
                for i in 2..raw_components.len() {
                    let mut current_path_builder = Vec::new();
                    current_path_builder.extend_from_slice(&raw_components[0..=i]);
                    let current_path = PathBuf::from(current_path_builder.join(r"/"));

                    if let HashMapEntry::Occupied(symlinks) =
                        symlinks.borrow_mut().entry(current_path)
                    {
                        let mut symlink_path_builder = Vec::new();
                        symlink_path_builder.extend_from_slice(&raw_components[(i + 1)..]);
                        symlink_path_builder.reverse();

                        for symlink_ptr in symlinks.get() {
                            let symlink = deref_mut!(*symlink_ptr);
                            path_builders.push((symlink_path_builder.clone(), symlink));
                        }
                    }
                }
            }

            resolved_paths.push(base_components.into_iter().collect());
        }

        resolved_paths
    }

    fn insert<F: FnMut(&mut FileSystem<T>, Event<T>)>(
        &mut self,
        path: &PathBuf,
        callback: &mut F,
    ) -> Option<*mut Entry<T>> {
        if !self.passes(path.to_str().unwrap()) {
            info!("ignoring {:?}", path);
            return None;
        }
        if !(path.exists() || path.read_link().is_ok()) {
            warn!("attempted to insert non existant path {:?}", path);
            return None;
        }

        let parent = deref_mut!(self.create_dir(&path.parent().unwrap().into())?);

        let parent = self.follow_links(parent)?;

        // We only need the last component, the parents are already inserted.
        let component = into_components(path).pop()?;

        // If the path is a dir, we can use create_dir to do the insert
        if path.is_dir() {
            return self.create_dir(path);
        }

        let children = deref_mut!(parent).children_mut().unwrap();

        // Ok = symlink, Err = real path

        Some(
            match children.entry(component.clone()) {
                HashMapEntry::Occupied(v) => v.into_mut(),
                HashMapEntry::Vacant(v) => match path.read_link() {
                    Ok(real) => {
                        let mut symlink = Box::new(Entry::Symlink {
                            name: component,
                            parent: parent as *mut _,
                            link: real.clone(),
                            watch_descriptor: None,
                            rules: into_rules(real.clone()),
                        });
                        self.create_watch(path, symlink.deref_mut());

                        self.symlinks
                            .borrow_mut()
                            .entry(real.clone())
                            .or_insert(Vec::new())
                            .push(symlink.deref_mut());

                        if let None = self.insert(&real, callback) {
                            debug!(
                                "inserting symlink {:?} which points to invalid path {:?}",
                                path, real
                            );
                        }

                        callback(self, Event::New(symlink.deref_mut() as *mut _));
                        v.insert(symlink)
                    }
                    Err(_) => {
                        let mut file = Box::new(Entry::File {
                            name: component,
                            parent,
                            watch_descriptor: None,
                            data: None,
                            file_handle: OpenOptions::new().read(true).open(path).unwrap(),
                        });
                        self.create_watch(path, file.deref_mut());

                        callback(self, Event::New(file.deref_mut() as *mut _));
                        v.insert(file)
                    }
                },
            }
            .deref_mut(),
        )
    }

    fn create_watch(&mut self, path: &PathBuf, entry: &mut Entry<T>) {
        let watch_descriptor = match self.inotify.add_watch(path, watch_mask(path)) {
            Ok(v) => v,
            Err(e) => {
                error!("unable to watch {:?}: {:?}", path, e);
                return;
            }
        };

        entry.set_watch_descriptor(Some(watch_descriptor.clone()));
        self.watch_descriptors
            .borrow_mut()
            .entry(watch_descriptor)
            .or_insert(Vec::new())
            .push(entry as *mut _);
        info!("watching {:?}", path);
    }

    fn remove_watch(&mut self, real_entry: &mut Entry<T>) {
        let watch_descriptor = match real_entry.watch_descriptor() {
            Some(v) => v.clone(),
            None => {
                warn!("unable to remove watch descriptor from entry {:?}: no watch descriptor attached", self.resolve_direct_path(real_entry));
                return;
            }
        };

        let mut watch_descriptors = self.watch_descriptors.borrow_mut();
        let entries = match watch_descriptors.get_mut(&watch_descriptor) {
            Some(v) => v,
            None => {
                error!(
                    "attempted to remove untracked watch descriptor {:?}",
                    &watch_descriptor
                );
                return;
            }
        };

        entries.retain(|entry_ptr| *entry_ptr != real_entry as *mut _);

        if entries.len() == 0 {
            watch_descriptors.remove(&watch_descriptor);
        }
        info!("unwatching {:?}", self.resolve_direct_path(real_entry));
    }

    fn remove<F: FnMut(&mut FileSystem<T>, Event<T>)>(
        &mut self,
        path: &PathBuf,
        callback: &mut F,
    ) -> Option<Box<Entry<T>>> {
        let parent = self.lookup(&path.parent()?.into())?;
        let component = into_components(path).pop()?;
        parent
            .children_mut()
            .unwrap() // this is safe since parent is the path of a parent dir so a valid lookup should return a Dir entry
            .remove(&component)
            .map(|mut entry| {
                self.walk_children(&mut entry.deref_mut(), callback);
                self.drop_entry(&mut entry.deref_mut(), callback);
                entry
            })
    }

    fn walk_children<F: FnMut(&mut FileSystem<T>, Event<T>)>(
        &mut self,
        mut entry: &mut Entry<T>,
        callback: &mut F,
    ) {
        let entry_ptr = entry as *mut _;
        match entry.deref_mut() {
            Entry::Dir { children, .. } => {
                for (_, child) in children.iter_mut() {
                    self.walk_children(child.deref_mut(), callback);
                }
            }
            Entry::Symlink { .. } | Entry::File { .. } => {
                callback(self, Event::Delete(entry_ptr));
            }
        }
    }

    fn drop_entry<F: FnMut(&mut FileSystem<T>, Event<T>)>(
        &mut self,
        mut entry: &mut Entry<T>,
        callback: &mut F,
    ) {
        let entry_ptr = entry as *mut _;
        self.remove_watch(entry);
        match entry.deref_mut() {
            Entry::Dir { children, .. } => {
                for (_, child) in children.iter_mut() {
                    self.drop_entry(child.deref_mut(), callback);
                }
            }
            Entry::Symlink { ref link, .. } => {
                // Make the lifetime of this mutable reference to self explicit
                {
                    let mut symlinks = self.symlinks.borrow_mut();
                    if let Some(entries) = symlinks.get_mut(link) {
                        entries.retain(|e| *e != entry_ptr);
                        if entries.len() == 0 {
                            symlinks.remove(link);
                        }
                    }
                }

                if !self.passes(link.to_str().unwrap()) {
                    self.remove(&link, callback);
                }
            }
            _ => {}
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
    ) -> Option<*mut Entry<T>> {
        let new_parent = self.create_dir(&to.parent().unwrap().into()).unwrap();
        let entry = match self.lookup(from) {
            Some(v) => v,
            None => {
                return self.insert(to, callback);
            }
        };
        let new_name = into_components(to).pop()?;
        let old_name = entry.name().clone();
        let mut entry = deref_mut!(entry.parent())
            .children_mut()
            .expect("expected entry to be a directory")
            .remove(&old_name)
            .unwrap();

        entry.set_parent(new_parent.clone());
        entry.set_name(new_name.clone());
        let entry_ptr = entry.deref_mut() as *mut _;

        deref_mut!(new_parent)
            .children_mut()
            .expect("expected entry to be a directory")
            .insert(new_name, entry);
        Some(entry_ptr)
    }

    // Creates all entries for a directory.
    // If one of the entries already exists, it is skipped over.
    // The returns a linked list of all entries.
    fn create_dir(&mut self, path: &PathBuf) -> Option<*mut Entry<T>> {
        let mut entry = self.root.deref_mut() as *mut _;

        let mut components = into_components(path);
        // remove the first component because it will always be the root
        components.remove(0);

        // If the path has no parents return root as the parent.
        // This commonly occurs with paths in the filesystem root, e.g /some.file or /somedir
        if components.len() == 0 {
            return Some(entry);
        }

        let components_slice = components.as_slice();
        for (i, component) in components.iter().enumerate() {
            let children = deref_mut!(entry)
                .children_mut()
                .expect("expected entry to be a directory");
            let mut current_path = PathBuf::from(r"/");
            for slice in (&components_slice[0..=i]).iter() {
                current_path.push(slice);
            }

            let new_entry = match children.entry(component.clone()) {
                HashMapEntry::Occupied(v) => {
                    let entry_ref = v.into_mut().deref_mut();
                    match entry_ref {
                        Entry::Symlink { link, .. } => self.lookup(link)?,
                        _ => entry_ref as *mut _,
                    }
                }
                HashMapEntry::Vacant(v) => match current_path.read_link() {
                    Ok(real) => {
                        let mut symlink = Box::new(Entry::Symlink {
                            name: component.clone(),
                            parent: entry,
                            link: real.clone(),
                            watch_descriptor: None,
                            rules: into_rules(real.clone()),
                        });
                        self.create_watch(&current_path, symlink.deref_mut());

                        self.symlinks
                            .borrow_mut()
                            .entry(real.clone())
                            .or_insert(Vec::new())
                            .push(symlink.deref_mut());

                        v.insert(symlink);
                        match self.create_dir(&real) {
                            Some(v) => v,
                            None => {
                                panic!("unable to create symlink directory for entry {:?}", &real)
                            }
                        }
                    }
                    Err(_) => {
                        let mut current_path = PathBuf::from(r"/");
                        for slice in (&components_slice[0..=i]).iter() {
                            current_path.push(slice.to_str().expect("invalid unicode in path"));
                        }

                        let mut dir = Box::new(Entry::Dir {
                            name: component.clone(),
                            parent: entry,
                            children: HashMap::new(),
                            watch_descriptor: None,
                        });
                        self.create_watch(&current_path, dir.deref_mut());
                        v.insert(dir).as_mut()
                    }
                },
            };

            entry = new_entry;
        }

        Some(entry)
    }

    // Returns the entry that represents the supplied path.
    // If the path is not represented and therefor has no entry then None is return.
    pub fn lookup(&mut self, path: &PathBuf) -> Option<&mut Entry<T>> {
        let mut parent = self.root.deref_mut() as *mut Entry<T>;
        let mut components = into_components(path);
        // remove the first component because it will always be the root
        components.remove(0);

        // If the path has no components there is nothing to look up.
        if components.len() == 0 {
            return None;
        }

        let last_component = components.pop()?;

        for component in components {
            parent = deref_mut!(self.follow_links(parent)?)
                .children_mut()
                .expect("expected directory entry")
                .get_mut(&component)
                .map(|entry| entry.deref_mut() as *mut Entry<T>)
                .and_then(|entry| self.follow_links(entry))?;
        }

        deref_mut!(parent)
            .children_mut()
            .expect("expected directory entry")
            .get_mut(&last_component)
            .map(|entry| entry.deref_mut())
    }

    fn follow_links(&mut self, mut entry: *mut Entry<T>) -> Option<*mut Entry<T>> {
        unsafe {
            while let Some(link) = entry.as_mut().and_then(|e| e.link()) {
                entry = self.lookup(link)?;
            }
            Some(entry)
        }
    }

    fn is_symlink_target(&self, path: &str) -> bool {
        for (_, symlink_ptrs) in self.symlinks.borrow().iter() {
            for symlink_ptr in symlink_ptrs.iter() {
                let symlink = deref_mut!(*symlink_ptr);
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

        return false;
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
        builder.field("watched_dirs", &&self.watched_dirs);
        builder.field("watch_descriptors", &&self.watch_descriptors);
        builder.finish()
    }
}

// returns the watch mask depending on if a path is a file or dir
fn watch_mask(path: &PathBuf) -> WatchMask {
    if path.is_file() {
        WatchMask::MODIFY | WatchMask::DONT_FOLLOW
    } else {
        WatchMask::CREATE
            | WatchMask::DELETE
            | WatchMask::DONT_FOLLOW
            | WatchMask::MOVED_TO
            | WatchMask::MOVED_FROM
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
fn append_rules(rules: &mut Rules, mut path: PathBuf) -> () {
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

    fn new_fs<T>(mut path: PathBuf, rules: Option<Rules>) -> FileSystem<T> {
        let mut rules = rules.unwrap_or_else(|| {
            let mut rules = Rules::new();
            rules.add_inclusion(GlobRule::new(r"**").unwrap());
            rules
        });
        FileSystem::new(vec![path], rules)
    }

    fn run_test<T: FnOnce() -> () + panic::UnwindSafe>(test: T) -> () {
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
            match entry.unwrap() {
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
            match entry.unwrap() {
                Entry::File { .. } => {}
                _ => panic!("wrong entry type"),
            }

            File::create(&a).unwrap();

            fs.read_events(&mut |_, _| {});

            let entry = fs.lookup(&a);
            assert!(entry.is_some());
            match entry.unwrap() {
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
            match entry.unwrap() {
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
            match entry.unwrap() {
                Entry::File { .. } => {}
                _ => panic!("wrong entry type"),
            }

            File::create(&a).unwrap();

            fs.read_events(&mut |_, _| {});

            let entry = fs.lookup(&a);
            assert!(entry.is_some());
            match entry.unwrap() {
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
            match entry.unwrap() {
                Entry::Dir { .. } => {}
                _ => panic!("wrong entry type"),
            }

            let entry = fs.lookup(&b);
            assert!(entry.is_some());
            match entry.unwrap() {
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

            let entry = fs.lookup(&file_path);
            assert!(entry.is_some());
            let mut real_watch_descriptor = None;
            match entry.unwrap() {
                Entry::File {
                    watch_descriptor, ..
                } => {
                    real_watch_descriptor = watch_descriptor.clone();
                }
                _ => panic!("wrong entry type"),
            }

            let entry = fs.lookup(&hard_path);
            assert!(entry.is_some());
            match entry.unwrap() {
                Entry::File {
                    watch_descriptor, ..
                } => assert_eq!(
                    watch_descriptor.clone().unwrap(),
                    real_watch_descriptor.unwrap()
                ),
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

            let mut fs = new_fs::<()>(path.clone(), None);

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

            let mut fs = new_fs::<()>(path.clone(), None);

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

            let mut fs = new_fs::<()>(path.clone(), None);

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

            let mut fs = new_fs::<()>(path.clone(), None);

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

            let mut fs = new_fs::<()>(path.clone(), None);

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

            let mut fs = new_fs::<()>(path.clone(), None);

            rename(&old_dir_path, &new_dir_path);
            fs.read_events(&mut |_, _| {});

            assert!(fs.lookup(&old_dir_path).is_none());
            assert!(fs.lookup(&file_path).is_none());
            assert!(fs.lookup(&sym_path).is_none());
            assert!(fs.lookup(&hard_path).is_none());

            let entry = fs.lookup(&new_dir_path);
            assert!(entry.is_some());
            match entry.unwrap() {
                Entry::Dir { .. } => {}
                _ => panic!("wrong entry type"),
            }

            let entry = fs.lookup(&new_dir_path.join("file.log"));
            assert!(entry.is_some());
            match entry.unwrap() {
                Entry::File { .. } => {}
                _ => panic!("wrong entry type"),
            }

            let entry = fs.lookup(&new_dir_path.join("hard.log"));
            assert!(entry.is_some());
            match entry.unwrap() {
                Entry::File { .. } => {}
                _ => panic!("wrong entry type"),
            }

            let entry = fs.lookup(&new_dir_path.join("sym.log"));
            assert!(entry.is_some());
            match entry.unwrap() {
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

            rename(&old_dir_path, &new_dir_path);
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

            rename(&old_dir_path, &new_dir_path);
            fs.read_events(&mut |_, _| {});

            let entry = fs.lookup(&new_dir_path);
            assert!(entry.is_some());
            match entry.unwrap() {
                Entry::Dir { .. } => {}
                _ => panic!("wrong entry type"),
            }

            let entry = fs.lookup(&new_dir_path.join("file.log"));
            assert!(entry.is_some());
            match entry.unwrap() {
                Entry::File { .. } => {}
                _ => panic!("wrong entry type"),
            }

            let entry = fs.lookup(&new_dir_path.join("hard.log"));
            assert!(entry.is_some());
            match entry.unwrap() {
                Entry::File { .. } => {}
                _ => panic!("wrong entry type"),
            }

            let entry = fs.lookup(&new_dir_path.join("sym.log"));
            assert!(entry.is_some());
            match entry.unwrap() {
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
            rename(&file_path, &new_path);

            fs.read_events(&mut |_, _| {});

            let entry = fs.lookup(&file_path);
            assert!(entry.is_none());

            let entry = fs.lookup(&new_path);
            assert!(entry.is_some());
            match entry.unwrap() {
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

            let mut fs = new_fs::<()>(watch_path.clone(), None);

            rename(&file_path, &move_path);

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

            let mut fs = new_fs::<()>(watch_path.clone(), None);

            rename(&file_path, &move_path);
            File::create(file_path.clone()).unwrap();

            fs.read_events(&mut |_, _| {});

            let entry = fs.lookup(&file_path);
            assert!(entry.is_none());

            let entry = fs.lookup(&move_path);
            assert!(entry.is_some());
            match entry.unwrap() {
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

            let mut fs = new_fs::<()>(watch_path.clone(), None);

            rename(&file_path, &move_path);

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

            let mut fs = new_fs::<()>(path.clone(), Some(rules));

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
    fn filesystem_resolve_all_paths() {
        run_test(|| {
            let tempdir = TempDir::new("filesystem").unwrap();
            let path = tempdir.path().to_path_buf();

            let test_dir_path = path.join("testdir");
            let local_symlink_path = path.join("local_symlink.log");
            let remote_symlink_path = test_dir_path.join("remote_symlink.log");
            let file_path = path.join("file.log");

            File::create(file_path.clone()).unwrap();
            create_dir(&test_dir_path).unwrap();
            symlink(&file_path, &remote_symlink_path).unwrap();
            symlink(&file_path, &local_symlink_path).unwrap();

            let mut fs = new_fs::<()>(path.clone(), None);

            let entry =
                unsafe { (fs.lookup(&file_path).unwrap() as *mut Entry<()>).as_ref() }.unwrap();
            let resolved_paths = fs.resolve_all_paths(entry);

            assert!(resolved_paths
                .iter()
                .any(|other| other.to_str() == local_symlink_path.to_str()));
            assert!(resolved_paths
                .iter()
                .any(|other| other.to_str() == remote_symlink_path.to_str()));
            assert!(resolved_paths
                .iter()
                .any(|other| other.to_str() == file_path.to_str()));
        });
    }

    // #[test]
    fn filesystem_crash_test() {
        run_test(|| {
            let mut fs = new_fs::<()>("/var/log/".into(), None);
            loop {
                fs.read_events(&mut |_, _| {});
            }
            // exec sudo logrotate -f /etc/logrotate.conf and watch things burn lol
        });
    }
}

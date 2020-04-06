use inotify::{EventMask, Inotify, WatchDescriptor, WatchMask};
use std::ffi::OsString;
use std::io;
use std::path::Path;

#[derive(Debug, Clone, PartialEq)]
pub enum WatchEvent {
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

pub struct Watcher {
    inotify: Inotify,
}

impl Watcher {
    pub fn new() -> io::Result<Self> {
        Ok(Self {
            inotify: Inotify::init()?,
        })
    }

    pub fn watch<P: AsRef<Path>>(&mut self, path: P) -> io::Result<WatchDescriptor> {
        self.inotify
            .add_watch(path.as_ref(), watch_mask(path.as_ref()))
    }

    pub fn unwatch(&mut self, wd: WatchDescriptor) -> io::Result<()> {
        self.inotify.rm_watch(wd)
    }

    pub fn read_events(&mut self, buffer: &mut [u8]) -> io::Result<Vec<WatchEvent>> {
        let mut events = Vec::new();
        for raw_event in self.inotify.read_events(buffer)? {
            if raw_event.mask.contains(EventMask::MOVED_FROM) {
                let mut found_match = false;
                for event in events.iter_mut() {
                    match event {
                        WatchEvent::MovedTo { wd, name, cookie } => {
                            if *cookie == raw_event.cookie {
                                *event = WatchEvent::Move {
                                    from_wd: raw_event.wd.clone(),
                                    from_name: raw_event.name.unwrap().to_os_string(),
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
                    events.push(WatchEvent::MovedFrom {
                        wd: raw_event.wd.clone(),
                        name: raw_event.name.unwrap().to_os_string(),
                        cookie: raw_event.cookie,
                    });
                }
            } else if raw_event.mask.contains(EventMask::MOVED_TO) {
                let mut found_match = false;
                for event in events.iter_mut() {
                    match event {
                        WatchEvent::MovedFrom { wd, name, cookie } => {
                            if *cookie == raw_event.cookie {
                                *event = WatchEvent::Move {
                                    from_wd: wd.clone(),
                                    from_name: name.clone(),
                                    to_wd: raw_event.wd.clone(),
                                    to_name: raw_event.name.unwrap().to_os_string(),
                                };
                                found_match = true;
                                break;
                            }
                        }
                        _ => continue,
                    };
                }

                if !found_match {
                    events.push(WatchEvent::MovedTo {
                        wd: raw_event.wd.clone(),
                        name: raw_event.name.unwrap().to_os_string(),
                        cookie: raw_event.cookie,
                    });
                }
            } else if raw_event.mask.contains(EventMask::CREATE) {
                events.push(WatchEvent::Create {
                    wd: raw_event.wd.clone(),
                    name: raw_event.name.unwrap().to_os_string(),
                });
            } else if raw_event.mask.contains(EventMask::DELETE) {
                events.push(WatchEvent::Delete {
                    wd: raw_event.wd.clone(),
                    name: raw_event.name.unwrap().to_os_string(),
                });
            } else if raw_event.mask.contains(EventMask::MODIFY) {
                events.push(WatchEvent::Modify {
                    wd: raw_event.wd.clone(),
                });
            } else if raw_event.mask.contains(EventMask::Q_OVERFLOW) {
                events.push(WatchEvent::Overflow);
            }
        }
        Ok(events)
    }
}

// returns the watch mask depending on if a path is a file or dir
fn watch_mask<P: AsRef<Path>>(path: P) -> WatchMask {
    if path.as_ref().is_file() {
        WatchMask::MODIFY | WatchMask::DONT_FOLLOW
    } else {
        WatchMask::CREATE
            | WatchMask::DELETE
            | WatchMask::DONT_FOLLOW
            | WatchMask::MOVED_TO
            | WatchMask::MOVED_FROM
    }
}

use crate::cache::entry::Entry;
use crate::cache::event::Event;
use crate::cache::FileSystem;
use crate::rule::Rules;
use http::types::body::LineBuilder;
use metrics::Metrics;
use std::cell::RefCell;
use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::PathBuf;
use std::rc::Rc;

/// Tails files on a filesystem by inheriting events from a Watcher
pub struct Tailer {
    // tracks the offset (bytes from the beginning of the file we have read) of file(s)
    fs: Rc<RefCell<FileSystem<u64>>>,
}

impl Tailer {
    /// Creates new instance of Tailer
    pub fn new(watched_dirs: Vec<PathBuf>, rules: Rules) -> Self {
        let fs = FileSystem::new(watched_dirs, rules);
        Self {
            fs: Rc::new(RefCell::new(fs)),
        }
    }
    /// Runs the main logic of the tailer, this can only be run once so Tailer is consumed
    pub fn process<F>(&mut self, callback: &mut F)
    where
        F: FnMut(Vec<LineBuilder>),
    {
        self.fs.clone().borrow_mut().read_events(&mut |fs, event| {
            match event {
                Event::Initialize(mut entry_ptr) => {
                    // will initiate a file to it's current length
                    let entry = unsafe { entry_ptr.as_mut() };
                    let path = fs.resolve_direct_path(entry);
                    match entry {
                        Entry::File { ref mut data, .. } => {
                            let mut len = path.metadata().map(|m| m.len()).unwrap_or(0);
                            if len < 8192 { len = 0 }
                            info!("initialized {:?} with offset {}", path, len,);
                            *data = len;
                        }
                        _ => {}
                    };
                }
                Event::New(mut entry_ptr) => {
                    Metrics::fs().increment_creates();
                    // similar to initiate but sets the offset to 0
                    let entry = unsafe { entry_ptr.as_mut() };
                    let paths = fs.resolve_valid_paths(entry);
                    if paths.len() == 0 {
                        return;
                    }

                    match entry {
                        Entry::File { ref mut data, file_handle, .. } => {
                            info!("added {:?}", paths[0]);
                            *data = 0;
                            self.tail(file_handle, &paths, data, callback);
                        }
                        _ => {}
                    };
                }
                Event::Write(mut entry_ptr) => {
                    Metrics::fs().increment_writes();
                    let entry = unsafe { entry_ptr.as_mut() };
                    let paths = fs.resolve_valid_paths(entry);
                    if paths.len() == 0 {
                        return;
                    }

                    if let Entry::File { ref mut data, file_handle, .. } = entry {
                        self.tail(file_handle, &paths, data, callback);
                    }
                }
                Event::Delete(mut entry_ptr) => {
                    Metrics::fs().increment_deletes();
                    let mut entry = unsafe { entry_ptr.as_mut() };
                    let paths = fs.resolve_valid_paths(entry);
                    if paths.len() == 0 {
                        return;
                    }

                    if let Entry::Symlink { link, .. } = entry {
                        if let Some(real_entry) = fs.lookup(link) {
                            entry = unsafe { &mut *real_entry.as_ptr() };
                        } else {
                            error!("can't wrap up deleted symlink - pointed to file / directory doesn't exist: {:?}", paths[0]);
                        }
                    }

                    if let Entry::File { ref mut data, file_handle, .. } = entry {
                        self.tail(file_handle, &paths, data, callback);
                    }
                }
            };
        });
    }

    // tail a file for new line(s)
    fn tail<F>(
        &mut self,
        file_handle: &File,
        paths: &Vec<PathBuf>,
        offset: &mut u64,
        callback: &mut F,
    ) where
        F: FnMut(Vec<LineBuilder>),
    {
        // get the file len
        let len = match file_handle.metadata().map(|m| m.len()) {
            Ok(v) => v,
            Err(e) => {
                error!("unable to stat {:?}: {:?}", &paths[0], e);
                return;
            }
        };

        // if we are at the end of the file there's no work to do
        if *offset == len {
            return;
        }
        // open the file, create a reader
        let mut reader = BufReader::new(file_handle);
        // if the offset is greater than the file's len
        // it's very likely a truncation occurred
        if *offset > len {
            info!("{:?} was truncated from {} to {}", &paths[0], *offset, len);
            *offset = if len < 8192 { 0 } else { len };
            return;
        }
        // seek to the offset, this creates the "tailing" effect
        if let Err(e) = reader.seek(SeekFrom::Start(*offset)) {
            error!("error seeking {:?}", e);
            return;
        }

        loop {
            let mut raw_line = Vec::new();
            // read until a new line returning the line length
            let line_len = match reader.read_until(b'\n', &mut raw_line) {
                Ok(v) => v as u64,
                Err(e) => {
                    error!("error reading from file {:?}: {:?}", &paths[0], e);
                    return;
                }
            };
            // try to parse the raw data as utf8
            // if that fails replace invalid chars with blank chars
            // see String::from_utf8_lossy docs
            let mut line = String::from_utf8(raw_line)
                .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).to_string());
            // if the line doesn't end with a new line we might have read in the middle of a write
            // so we return in this case
            if !line.ends_with('\n') {
                Metrics::fs().increment_partial_reads();
                return;
            }
            // remove the trailing new line
            line.pop();
            // increment the offset
            *offset += line_len;
            // send the line upstream, safe to unwrap
            debug!("tailer sendings lines for {:?}", paths);
            callback(
                paths
                    .iter()
                    .map(|path| {
                        Metrics::fs().increment_lines();
                        Metrics::fs().add_bytes(line_len);
                        LineBuilder::new()
                            .line(line.clone())
                            .file(path.to_str().unwrap_or("").to_string())
                    })
                    .collect(),
            );
        }
    }
}

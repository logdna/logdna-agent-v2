use crate::cache::entry::Entry;
use crate::cache::event::Event;
pub use crate::cache::DirPathBuf;
use crate::cache::FileSystem;
use crate::rule::Rules;
use http::types::body::LineBuilder;
use metrics::Metrics;
use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::ops::{Deref, DerefMut};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use futures::{Stream, StreamExt};

use thiserror::Error;

#[derive(Clone, std::fmt::Debug)]
pub enum Lookback {
    Start,
    SmallFiles,
    None,
}

#[derive(Error, Debug)]
pub enum ParseLookbackError {
    #[error("Unknown lookback strategy: {0}")]
    Unknown(String),
}

impl std::str::FromStr for Lookback {
    type Err = ParseLookbackError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s
            .to_lowercase()
            .split_whitespace()
            .collect::<String>()
            .as_str()
        {
            "start" => Ok(Lookback::Start),
            "smallfiles" => Ok(Lookback::SmallFiles),
            "none" => Ok(Lookback::None),
            _ => Err(ParseLookbackError::Unknown(s.into())),
        }
    }
}

impl Default for Lookback {
    fn default() -> Self {
        Lookback::SmallFiles
    }
}

/// Tails files on a filesystem by inheriting events from a Watcher
pub struct Tailer {
    lookback_config: Lookback,
    fs_cache: Arc<Mutex<FileSystem<u64>>>,
}

impl Tailer {
    /// Creates new instance of Tailer
    pub fn new(watched_dirs: Vec<DirPathBuf>, rules: Rules, lookback_config: Lookback) -> Self {
        Self {
            lookback_config,
            fs_cache: Arc::new(Mutex::new(FileSystem::new(watched_dirs, rules))),
        }
    }
    /// Runs the main logic of the tailer, this can only be run once so Tailer is consumed
    pub fn process<'a>(
        &mut self,
        buf: &'a mut [u8],
    ) -> Result<impl Stream<Item = Vec<LineBuilder>> + 'a, std::io::Error> {
        let events = {
            match FileSystem::stream_events(self.fs_cache.clone(), buf) {
                Ok(event) => event,
                Err(e) => {
                    warn!("tailer stream raised exception: {:?}", e);
                    return Err(e);
                }
            }
        };

        Ok(events.map({
            let fs = self.fs_cache.clone();
            let lookback_config = self.lookback_config.clone();
            move |event| {
                let mut final_lines = Vec::new();

                let fs = fs.lock().expect("Couldn't lock fs");
                match event {
                    Event::Initialize(entry_ptr) => {
                        // will initiate a file to it's current length
                        if let Some(entry) = fs.entries.borrow().get(entry_ptr){
                            let path = fs.resolve_direct_path(&entry.borrow(), &fs.entries.borrow());
                            debug!("Initialise Event");

                            if let Entry::File { ref mut data, .. } = entry.borrow_mut().deref_mut() {
                                *data = match lookback_config {
                                    Lookback::Start => {
                                        info!("initialized {:?} with offset {}", path, 0);
                                        0
                                    },
                                    Lookback::SmallFiles => {
                                        let mut len = path.metadata().map(|m| m.len()).unwrap_or(0);
                                        if len < 8192 {
                                            info!("initialized {:?} with len {} offset {}", path, len, 0);
                                            len = 0;
                                        } else{
                                            info!("initialized {:?} with offset {}", path, len);
                                        }
                                        len
                                    },
                                    Lookback::None => {
                                        let len = path.metadata().map(|m| m.len()).unwrap_or(0);
                                        info!("initialized {:?} with offset {}", path, len);
                                        len
                                    }
                                }
                            }
                        };
                    }
                    Event::New(entry_ptr) => {
                        Metrics::fs().increment_creates();
                        // similar to initiate but sets the offset to 0
                        if let Some(entry) = fs.entries.borrow().get(entry_ptr){
                            let paths = fs.resolve_valid_paths(&entry.borrow(), &fs.entries.borrow());
                            debug!("New Event");
                            if !paths.is_empty() {
                                if let Entry::File {
                                    ref mut data,
                                    file_handle,
                                    ..
                                } = entry.borrow_mut().deref_mut()
                                {
                                    info!("added {:?}", paths[0]);
                                    *data = 0;
                                    if let Some(mut lines) = Tailer::tail(file_handle, &paths, data) {
                                        final_lines.append(&mut lines);
                                    }
                                }
                            }
                        }


                    }
                    Event::Write(entry_ptr) => {
                        Metrics::fs().increment_writes();
                        if let Some(entry) = fs.entries.borrow().get(entry_ptr){
                            let paths = fs.resolve_valid_paths(&entry.borrow(), &fs.entries.borrow());
                            debug!("Write Event");
                            if !paths.is_empty() {

                                if let  Entry::File {
                                    ref mut data,
                                    file_handle,
                                    ..
                                } = entry.borrow_mut().deref_mut()
                                {
                                    if let Some(mut lines) = Tailer::tail(file_handle, &paths, data) {
                                        final_lines.append(&mut lines);
                                    }
                                }

                            }
                        }
                    }
                    Event::Delete(entry_ptr) => {
                        Metrics::fs().increment_deletes();
                        {
                            let entries = fs.entries.borrow();
                            if let Some(mut entry) = entries.get(entry_ptr){
                                let paths = fs.resolve_valid_paths(&entry.borrow(), &entries);
                                debug!("Delete Event");
                                if !paths.is_empty() {
                                    if let Entry::Symlink { link, .. } = entry.borrow().deref() {
                                        if let Ok(Some(real_entry)) = fs.lookup(link, &entries) {
                                            if let Some(r_entry) = entries.get(real_entry) {
                                                entry = r_entry
                                            }
                                        } else {
                                            info!("can't wrap up deleted symlink - pointed to file / directory doesn't exist: {:?}", paths[0]);
                                        }
                                    }

                                    if let Entry::File {
                                        ref mut data,
                                        file_handle,
                                        ..
                                    } = entry.borrow_mut().deref_mut()
                                    {
                                        if let Some(mut lines) = Tailer::tail(file_handle, &paths, data) {
                                            final_lines.append(&mut lines);
                                        }
                                    }
                                }
                            }
                        }
                        {
                            // At this point, the entry should not longer be used
                            // and removed from the map to allow the file handle to be dropped.
                            // In case following events contain this entry key, it
                            // should be ignored by the Tailer (all branches MUST contain
                            // if Some(..) = entries.get(key) clauses)
                            let mut entries = fs.entries.borrow_mut();
                            if entries.remove(entry_ptr).is_some() {
                                debug!(
                                    "Entry was removed from the map, new length: {}",
                                    entries.len());
                            }
                        }
                    }
                };
                futures::stream::iter(final_lines)
            }}).flatten())
    }

    // tail a file for new line(s)
    fn tail(
        file_handle: &File,
        paths: &[PathBuf],
        offset: &mut u64,
    ) -> Option<Vec<Vec<LineBuilder>>> {
        // get the file len
        let len = match file_handle.metadata().map(|m| m.len()) {
            Ok(v) => v,
            Err(e) => {
                error!("unable to stat {:?}: {:?}", &paths[0], e);
                return None;
            }
        };

        // if we are at the end of the file there's no work to do
        if *offset == len {
            return None;
        }
        // open the file, create a reader
        let mut reader = BufReader::new(file_handle);
        // if the offset is greater than the file's len
        // it's very likely a truncation occurred
        if *offset > len {
            info!("{:?} was truncated from {} to {}", &paths[0], *offset, len);
            *offset = if len < 8192 { 0 } else { len };
            return None;
        }
        // seek to the offset, this creates the "tailing" effect
        if let Err(e) = reader.seek(SeekFrom::Start(*offset)) {
            error!("error seeking {:?}", e);
            return None;
        }

        let mut line_groups = Vec::new();

        loop {
            let mut raw_line = Vec::new();
            // read until a new line returning the line length
            let line_len = match reader.read_until(b'\n', &mut raw_line) {
                Ok(v) => v as u64,
                Err(e) => {
                    error!("error reading from file {:?}: {:?}", &paths[0], e);
                    break;
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
                break;
            }
            // remove the trailing new line
            line.pop();
            // increment the offset
            *offset += line_len;
            // send the line upstream, safe to unwrap
            debug!("tailer sendings lines for {:?}", paths);
            line_groups.push(
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

        if line_groups.is_empty() {
            None
        } else {
            Some(line_groups)
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::rule::{GlobRule, Rules};
    use crate::test::LOGGER;
    use std::convert::TryInto;
    use std::io::Write;
    use std::panic;
    use tempfile::tempdir;
    use tokio::stream::StreamExt;

    macro_rules! take_events {
        ( $x:expr, $y: expr ) => {{
            {
                futures::StreamExt::collect::<Vec<_>>(futures::StreamExt::take($x, $y))
            }
        }};
    }

    fn run_test<T: FnOnce() + panic::UnwindSafe>(test: T) {
        #![allow(unused_must_use, clippy::clone_on_copy)]
        LOGGER.clone();
        let result = panic::catch_unwind(|| {
            test();
        });

        assert!(result.is_ok())
    }

    #[test]
    fn none_lookback() {
        run_test(|| {
            tokio_test::block_on(async {
                let mut rules = Rules::new();
                rules.add_inclusion(GlobRule::new(r"**").unwrap());

                let log_lines = "This is a test log line";
                debug!("{}", log_lines.as_bytes().len());
                let dir = tempdir().expect("Couldn't create temp dir...");

                let file_path = dir.path().join("test.log");

                let mut file = File::create(&file_path).expect("Couldn't create temp log file...");

                let line_write_count = (8192 / (log_lines.as_bytes().len() + 1)) + 2;
                (0..line_write_count).for_each(|_| {
                    writeln!(file, "{}", log_lines).expect("Couldn't write to temp log file...")
                });
                file.sync_all().expect("Failed to sync file");

                let mut tailer = Tailer::new(
                    vec![dir
                        .path()
                        .try_into()
                        .unwrap_or_else(|_| panic!("{:?} is not a directory!", dir.path()))],
                    rules,
                    Lookback::None,
                );
                let mut buf = [0u8; 4096];

                let stream = tailer
                    .process(&mut buf)
                    .expect("failed to read events")
                    .timeout(std::time::Duration::from_millis(500));

                let write_files = async move {
                    tokio::time::delay_for(tokio::time::Duration::from_millis(250)).await;
                    writeln!(file, "{}", log_lines).expect("Couldn't write to temp log file...");
                    file.sync_all().expect("Failed to sync file");
                };
                let (_, events) =
                    futures::join!(tokio::spawn(write_files), take_events!(stream, 2));
                let events = events.iter().flatten().collect::<Vec<_>>();
                assert_eq!(events.len(), 1);
                debug!("{:?}, {:?}", events.len(), &events);
            });
        });
    }
    #[test]
    fn smallfiles_lookback() {
        run_test(|| {
            tokio_test::block_on(async {
                let mut rules = Rules::new();
                rules.add_inclusion(GlobRule::new(r"**").unwrap());

                let log_lines = "This is a test log line";
                debug!("{}", log_lines.as_bytes().len());
                let dir = tempdir().expect("Couldn't create temp dir...");

                let file_path = dir.path().join("test.log");

                let mut file = File::create(&file_path).expect("Couldn't create temp log file...");

                let line_write_count = (8192 / (log_lines.as_bytes().len() + 1)) + 2;
                (0..line_write_count).for_each(|_| {
                    writeln!(file, "{}", log_lines).expect("Couldn't write to temp log file...")
                });
                file.sync_all().expect("Failed to sync file");

                let mut tailer = Tailer::new(
                    vec![dir
                        .path()
                        .try_into()
                        .unwrap_or_else(|_| panic!("{:?} is not a directory!", dir.path()))],
                    rules,
                    Lookback::SmallFiles,
                );
                let mut buf = [0u8; 4096];

                let stream = tailer
                    .process(&mut buf)
                    .expect("failed to read events")
                    .timeout(std::time::Duration::from_millis(500));

                let write_files = async move {
                    tokio::time::delay_for(tokio::time::Duration::from_millis(250)).await;
                    writeln!(file, "{}", log_lines).expect("Couldn't write to temp log file...");
                    file.sync_all().expect("Failed to sync file");
                };
                let (_, events) =
                    futures::join!(tokio::spawn(write_files), take_events!(stream, 2));
                let events = events.iter().flatten().collect::<Vec<_>>();
                assert_eq!(events.len(), 1);
                debug!("{:?}, {:?}", events.len(), &events);
            });
        });
    }

    #[test]
    fn start_lookback() {
        run_test(|| {
            tokio_test::block_on(async {
                let mut rules = Rules::new();
                rules.add_inclusion(GlobRule::new(r"**").unwrap());

                let log_lines = "This is a test log line";
                let dir = tempdir().expect("Couldn't create temp dir...");

                let file_path = dir.path().join("test.log");

                let mut file = File::create(&file_path).expect("Couldn't create temp log file...");
                let line_write_count = (8192 / (log_lines.as_bytes().len() + 1)) + 1;
                (0..line_write_count).for_each(|_| {
                    writeln!(file, "{}", log_lines).expect("Couldn't write to temp log file...")
                });
                file.sync_all().expect("Failed to sync file");

                let mut tailer = Tailer::new(
                    vec![dir
                        .path()
                        .try_into()
                        .unwrap_or_else(|_| panic!("{:?} is not a directory!", dir.path()))],
                    rules,
                    Lookback::Start,
                );

                let mut buf = [0u8; 4096];

                let stream = tailer
                    .process(&mut buf)
                    .expect("failed to read events")
                    .timeout(std::time::Duration::from_millis(500));

                let write_files = async move {
                    tokio::time::delay_for(tokio::time::Duration::from_millis(250)).await;
                    (0..5).for_each(|_| {
                        writeln!(file, "{}", log_lines)
                            .expect("Couldn't write to temp log file...");
                        file.sync_all().expect("Failed to sync file");
                    });
                };
                let (_, events) = futures::join!(
                    tokio::spawn(write_files),
                    take_events!(stream, line_write_count + 4 + 1)
                );
                let events = events.iter().flatten().collect::<Vec<_>>();
                debug!("{:?}, {:?}", events.len(), &events);
                assert_eq!(events.len(), line_write_count + 5);
                debug!("{:?}, {:?}", events.len(), &events);
            })
        })
    }
}

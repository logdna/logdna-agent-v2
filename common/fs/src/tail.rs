use crate::cache::entry::Entry;
use crate::cache::event::Event;
use crate::cache::tailed_file::LazyLineSerializer;
pub use crate::cache::DirPathBuf;
use crate::cache::{EntryKey, Error as CacheError, FileSystem, EVENT_STREAM_BUFFER_COUNT};
use crate::rule::Rules;
use metrics::Metrics;
use state::FileId;
use std::collections::HashMap;
use std::ops::{Deref, DerefMut};
use std::path::Path;

use std::sync::Arc;
use tokio::sync::Mutex;

use futures::{Stream, StreamExt};

use std::fmt;
use thiserror::Error;

#[derive(Clone, std::fmt::Debug, PartialEq)]
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

impl fmt::Display for Lookback {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Lookback::Start => "start",
                Lookback::SmallFiles => "smallfiles",
                Lookback::None => "none",
            }
        )
    }
}

impl Default for Lookback {
    fn default() -> Self {
        Lookback::None
    }
}

type SyncHashMap<K, V> = Arc<Mutex<HashMap<K, V>>>;

/// Tails files on a filesystem by inheriting events from a Watcher
pub struct Tailer {
    lookback_config: Lookback,
    fs_cache: Arc<Mutex<FileSystem>>,
    initial_offsets: Option<HashMap<FileId, u64>>,
    event_times: SyncHashMap<EntryKey, (usize, chrono::DateTime<chrono::Utc>)>,
}

impl Tailer {
    /// Creates new instance of Tailer
    pub fn new(
        watched_dirs: Vec<DirPathBuf>,
        rules: Rules,
        lookback_config: Lookback,
        initial_offsets: Option<HashMap<FileId, u64>>,
    ) -> Self {
        Self {
            lookback_config,
            fs_cache: Arc::new(Mutex::new(FileSystem::new(watched_dirs, rules))),
            initial_offsets,
            event_times: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn get_file_for_path(fs: &FileSystem, next_path: &std::path::Path) -> Option<EntryKey> {
        let entries = fs.entries.borrow();
        let mut next_path = next_path;
        loop {
            let next_entry_key = fs.lookup(next_path, &entries)?;
            match entries.get(next_entry_key) {
                Some(Entry::Symlink { link, .. }) => next_path = link,
                Some(Entry::File { .. }) => return Some(next_entry_key),
                _ => break,
            }
        }
        None
    }

    async fn get_initial_offset(
        target: &Path,
        fs: &FileSystem,
        initial_offsets: Option<HashMap<FileId, u64>>,
        lookback_config: Lookback,
    ) -> Option<(EntryKey, u64)> {
        fn _lookup_offset(
            initial_offsets: &HashMap<FileId, u64>,
            key: &FileId,
            path: &Path,
        ) -> Option<u64> {
            if let Some(offset) = initial_offsets.get(key).copied() {
                debug!("Got offset {} from state using key {:?}", offset, path);
                Some(offset)
            } else {
                None
            }
        }
        let entry_key = fs.lookup(target, &fs.entries.borrow())?;
        let entries = fs.entries.borrow();
        let entry = &entries.get(entry_key)?;
        let path = fs.resolve_direct_path(entry, &fs.entries.borrow());
        if let Entry::File { data, .. } = entry {
            let inode: FileId = { (&data.borrow().deref().get_inode().await).into() };
            Some((
                entry_key,
                match lookback_config {
                    Lookback::Start => match initial_offsets.as_ref() {
                        Some(initial_offsets) => {
                            _lookup_offset(initial_offsets, &inode, &path).unwrap_or(0)
                        }
                        None => 0,
                    },
                    Lookback::SmallFiles => {
                        // Check the actual file len
                        let file_len = path.metadata().map(|m| m.len()).unwrap_or(0);
                        let smallfiles_offset = if file_len < 8192 { 0 } else { file_len };

                        match initial_offsets.as_ref() {
                            Some(initial_offsets) => _lookup_offset(initial_offsets, &inode, &path)
                                .unwrap_or(smallfiles_offset),
                            None => {
                                debug!(
                                    "Smallfiles lookback {} from len using key {:?}",
                                    file_len, path
                                );
                                smallfiles_offset
                            }
                        }
                    }
                    Lookback::None => path.metadata().map(|m| m.len()).unwrap_or(0),
                },
            ))
        } else {
            None
        }
    }

    async fn handle_event(
        event: Event,
        initial_offsets: Option<HashMap<FileId, u64>>,
        lookback_config: Lookback,
        fs: &FileSystem,
    ) -> Option<impl Stream<Item = LazyLineSerializer>> {
        match event {
            Event::Initialize(entry_ptr) => {
                debug!("Initialize Event");
                // will initiate a file to it's current length
                let entries = fs.entries.borrow();
                let entry = entries.get(entry_ptr)?;
                let path = fs.resolve_direct_path(entry, &fs.entries.borrow());
                match entry {
                    Entry::File { name, data, .. } => {
                        // If the file's passes the rules tail it
                        info!("initialize event for file {:?}, target {:?}", name, path);
                        let (_, offset) =
                            Tailer::get_initial_offset(&path, fs, initial_offsets, lookback_config)
                                .await?;
                        data.borrow_mut()
                            .deref_mut()
                            .seek(offset)
                            .await
                            .unwrap_or_else(|e| error!("error seeking {:?}", e));
                        info!("initialized {:?} with offset {}", name, offset);

                        if fs.is_initial_dir_target(&path) {
                            return data.borrow_mut().tail(vec![path]).await;
                        }
                    }
                    Entry::Symlink { name, link, .. } => {
                        let sym_path = path;
                        let final_target = Tailer::get_file_for_path(fs, link)?;
                        info!(
                            "initialize event for symlink {:?}, target {:?}, final target {:?}",
                            name, link, final_target
                        );

                        let entries = &fs.entries.borrow();
                        let path = fs
                            .resolve_direct_path(entries.get(final_target)?, &fs.entries.borrow());

                        let (entry_key, offset) =
                            Tailer::get_initial_offset(&path, fs, initial_offsets, lookback_config)
                                .await?;
                        if let Entry::File { data, .. } = &entries.get(entry_key)? {
                            info!(
                                "initialized symlink {:?} as {:?} with offset {}",
                                name, final_target, offset
                            );
                            let mut data = data.borrow_mut();
                            data.deref_mut()
                                .seek(offset)
                                .await
                                .unwrap_or_else(|e| error!("error seeking {:?}", e));
                            return data.tail(vec![sym_path]).await;
                        }
                    }
                    _ => (),
                }
            }
            Event::New(entry_ptr) => {
                Metrics::fs().increment_creates();
                debug!("New Event");
                // similar to initiate but sets the offset to 0
                let entries = fs.entries.borrow();
                let entry = entries.get(entry_ptr)?;
                let paths = fs.resolve_valid_paths(entry, &entries);
                if paths.is_empty() {
                    return None;
                }
                if let Entry::File { data, .. } = entry {
                    info!("added {:?}", paths[0]);
                    return data.borrow_mut().tail(paths.clone()).await;
                }
            }
            Event::Write(entry_ptr) => {
                Metrics::fs().increment_writes();
                debug!("Write Event");
                let entries = fs.entries.borrow();
                let entry = entries.get(entry_ptr)?;
                let paths = fs.resolve_valid_paths(entry, &entries);
                if paths.is_empty() {
                    return None;
                }

                if let Entry::File { data, .. } = entry {
                    return data.borrow_mut().deref_mut().tail(paths).await;
                }
            }
            Event::Delete(entry_ptr) => {
                Metrics::fs().increment_deletes();
                debug!("Delete Event");
                let ret = {
                    let entries = fs.entries.borrow();
                    let mut entry = entries.get(entry_ptr)?;
                    let paths = fs.resolve_valid_paths(entry, &entries);
                    if paths.is_empty() {
                        None
                    } else {
                        if let Entry::Symlink { link, .. } = entry {
                            if let Some(real_entry) = fs.lookup(link, &entries) {
                                if let Some(r_entry) = entries.get(real_entry) {
                                    entry = r_entry
                                }
                            } else {
                                info!("can't wrap up deleted symlink - pointed to file / directory doesn't exist: {:?}", paths[0]);
                            }
                        }

                        if let Entry::File { data, .. } = entry {
                            data.borrow_mut().deref_mut().tail(paths).await
                        } else {
                            None
                        }
                    }
                };
                {
                    // At this point, the entry should not longer be used
                    // and removed from the map to allow the file handle to be dropped.
                    // In case following events contain this entry key, it
                    // should be ignored by the Tailer (all branches MUST contain
                    // if Some(..) = entries.get(key) clauses)
                    let mut entries = fs.entries.borrow_mut();
                    if entries.remove(entry_ptr).is_some() {
                        info!(
                            "Removed file information, currently tracking {} files and directories",
                            entries.len()
                        );
                    }
                }
                return ret;
            }
        };
        None
    }

    /// Runs the main logic of the tailer, this can only be run once so Tailer is consumed
    pub fn process<'a>(
        &mut self,
        buf: &'a mut [u8],
    ) -> Result<impl Stream<Item = Result<LazyLineSerializer, CacheError>> + 'a, std::io::Error>
    {
        let events = {
            match FileSystem::stream_events(self.fs_cache.clone(), buf) {
                Ok(events) => events,
                Err(e) => {
                    warn!("tailer stream raised exception: {:?}", e);
                    return Err(e);
                }
            }
        };

        debug!("Tailer starting with lookback: {:?}", self.lookback_config);

        Ok(events
            .enumerate()
            .then({
                let fs = self.fs_cache.clone();
                let lookback_config = self.lookback_config.clone();
                let initial_offsets = self.initial_offsets.clone();
                let event_times = self.event_times.clone();

                move |(event_idx, (event_result, event_time))| {
                    let fs = fs.clone();
                    let lookback_config = lookback_config.clone();
                    let initial_offsets = initial_offsets.clone();
                    let event_times = event_times.clone();

                    async move {
                        match event_result {
                            Err(err) => {
                                let fuck = futures::stream::iter(vec![Err(err)]);
                                let fuck = fuck.boxed();
                                Some(fuck)
                            }
                            Ok(event) => {
                                // debounce events
                                // check event_time, if it's before the previous one
                                let key_and_previous_event_time = match event {
                                    Event::Write(key) => {
                                        let event_times = event_times.lock().await;
                                        Some((key, event_times.get(&key).cloned()))
                                    }
                                    _ => None,
                                };

                                // Need to check if the event is within buffer_length
                                if let Some((_, Some((prev_event_idx, previous_event_time)))) =
                                    key_and_previous_event_time
                                {
                                    // We've already processed this event, skip tailing
                                    if previous_event_time >= event_time
                                        && event_idx < (prev_event_idx + EVENT_STREAM_BUFFER_COUNT)
                                    {
                                        debug!("skipping already processed events");
                                        Metrics::fs().increment_writes();
                                        return None;
                                    }
                                }

                                let line = Tailer::handle_event(
                                    event,
                                    initial_offsets,
                                    lookback_config,
                                    fs.lock().await.deref(),
                                )
                                .await;

                                // TODO: Comment this
                                let line = line.map(|x| x.map(|y| Ok(y)).boxed());

                                if let Some((key, _)) = key_and_previous_event_time {
                                    let mut event_times = event_times.lock().await;
                                    let new_event_time = chrono::offset::Utc::now();
                                    event_times.insert(key, (event_idx, new_event_time));
                                }
                                line
                            }
                        }
                    }
                }
            })
            .filter_map(|x| async move { x })
            .flatten())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::rule::{GlobRule, Rules};
    use crate::test::LOGGER;
    use std::convert::TryInto;
    use std::fs::File;
    use std::io::Write;
    use std::panic;
    use tempfile::tempdir;
    use tokio_stream::StreamExt;

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
                    None,
                );
                let mut buf = [0u8; 4096];

                let stream = tailer
                    .process(&mut buf)
                    .expect("failed to read events")
                    .timeout(std::time::Duration::from_millis(500));

                let write_files = async move {
                    tokio::time::sleep(tokio::time::Duration::from_millis(250)).await;
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

                let log_lines1 = "This is a test log line";
                debug!("{}", log_lines1.as_bytes().len());
                let dir = tempdir().expect("Couldn't create temp dir...");

                let file_path = dir.path().join("test.log");

                let mut file = File::create(&file_path).expect("Couldn't create temp log file...");

                let line_write_count = (8192 / (log_lines1.as_bytes().len() + 1)) + 2;
                (0..line_write_count).for_each(|_| {
                    writeln!(file, "{}", log_lines1).expect("Couldn't write to temp log file...")
                });
                file.sync_all().expect("Failed to sync file");

                let mut tailer = Tailer::new(
                    vec![dir
                        .path()
                        .try_into()
                        .unwrap_or_else(|_| panic!("{:?} is not a directory!", dir.path()))],
                    rules,
                    Lookback::SmallFiles,
                    None,
                );
                let mut buf = [0u8; 4096];

                let stream = tailer
                    .process(&mut buf)
                    .expect("failed to read events")
                    .timeout(std::time::Duration::from_millis(500));

                let write_files = async move {
                    tokio::time::sleep(tokio::time::Duration::from_millis(250)).await;

                    let log_lines2 = "This is a test log line2";
                    writeln!(file, "{}", log_lines2).expect("Couldn't write to temp log file...");
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
                let line_write_count = (8_388_608 / (log_lines.as_bytes().len() + 1)) + 1;
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
                    None,
                );

                let mut buf = [0u8; 4096];

                let stream = tailer
                    .process(&mut buf)
                    .expect("failed to read events")
                    .timeout(std::time::Duration::from_millis(500));

                let write_files = async move {
                    tokio::time::sleep(tokio::time::Duration::from_millis(250)).await;
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

use crate::cache::entry::Entry;
use crate::cache::event::Event;
use crate::cache::tailed_file::LazyLineSerializer;
pub use crate::cache::DirPathBuf;
use crate::cache::{EntryKey, FileSystem};
use crate::rule::Rules;
use metrics::Metrics;
use state::FileId;
use std::collections::HashMap;
use std::ops::{Deref, DerefMut};
use std::path::Path;
use std::sync::{Arc, Mutex};

use futures::{Stream, StreamExt};

use std::fmt;
use std::time::Duration;
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
        Lookback::SmallFiles
    }
}

/// Tails files on a filesystem by inheriting events from a Watcher
pub struct Tailer {
    lookback_config: Lookback,
    fs_cache: Arc<Mutex<FileSystem>>,
    initial_offsets: Option<HashMap<FileId, u64>>,
}

impl Tailer {
    /// Creates new instance of Tailer
    pub fn new(
        watched_dirs: Vec<DirPathBuf>,
        rules: Rules,
        lookback_config: Lookback,
        initial_offsets: Option<HashMap<FileId, u64>>,
        event_delay: Duration,
    ) -> Self {
        Self {
            lookback_config,
            fs_cache: Arc::new(Mutex::new(FileSystem::new(
                watched_dirs,
                rules,
                event_delay,
            ))),
            initial_offsets,
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
        let path = entry.path();
        if let Entry::File { data, .. } = entry {
            let inode: FileId = { (&data.borrow().deref().get_inode().await).into() };
            Some((
                entry_key,
                match lookback_config {
                    Lookback::Start => match initial_offsets.as_ref() {
                        Some(initial_offsets) => {
                            _lookup_offset(&initial_offsets, &inode, &path).unwrap_or(0)
                        }
                        None => 0,
                    },
                    Lookback::SmallFiles => {
                        // Check the actual file len
                        let file_len = path.metadata().map(|m| m.len()).unwrap_or(0);
                        let smallfiles_offset = if file_len < 8192 { 0 } else { file_len };

                        match initial_offsets.as_ref() {
                            Some(initial_offsets) => {
                                _lookup_offset(&initial_offsets, &inode, &path)
                                    .unwrap_or(smallfiles_offset)
                            }
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
                let path = entry.path().to_path_buf();
                match entry {
                    Entry::File { data, .. } => {
                        let path_display = entry.path().display();
                        // If the file's passes the rules tail it
                        info!("initialize event for file {}", path_display);
                        let (_, offset) = Tailer::get_initial_offset(
                            &path,
                            &fs,
                            initial_offsets,
                            lookback_config,
                        )
                        .await?;
                        data.borrow_mut()
                            .deref_mut()
                            .seek(offset)
                            .await
                            .unwrap_or_else(|e| error!("error seeking {:?}", e));
                        info!("initialized {} with offset {}", path_display, offset);

                        if fs.is_initial_dir_target(&path) {
                            return data.borrow_mut().tail(vec![path]).await;
                        }
                    }
                    Entry::Symlink { link, .. } => {
                        let sym_path = path;
                        let final_target = Tailer::get_file_for_path(&fs, link)?;

                        let entries = &fs.entries.borrow();
                        let path = entries.get(final_target)?.path();

                        info!(
                            "initialize event for symlink {}, final target {}",
                            sym_path.display(),
                            path.display()
                        );

                        let (entry_key, offset) = Tailer::get_initial_offset(
                            &path,
                            &fs,
                            initial_offsets,
                            lookback_config,
                        )
                        .await?;
                        if let Entry::File { data, .. } = &entries.get(entry_key)? {
                            info!(
                                "initialized symlink {:?} as {} with offset {}",
                                sym_path.file_name(),
                                path.display(),
                                offset
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
                let path = entry.path().to_path_buf();
                if let Entry::File { data, .. } = entry {
                    info!("added {:?}", path);
                    return data.borrow_mut().tail(vec![path]).await;
                }
            }
            Event::Write(entry_ptr) => {
                Metrics::fs().increment_writes();
                debug!("Write Event");
                let entries = fs.entries.borrow();
                let entry = entries.get(entry_ptr)?;
                let path = entry.path().to_path_buf();

                if let Entry::File { data, .. } = entry {
                    return data.borrow_mut().deref_mut().tail(vec![path]).await;
                }
            }
            Event::Delete(entry_ptr) => {
                Metrics::fs().increment_deletes();
                debug!("Delete Event");
                let ret = {
                    let entries = fs.entries.borrow();
                    let mut entry = entries.get(entry_ptr)?;
                    let path = entry.path().to_path_buf();

                    if let Entry::Symlink { link, .. } = entry {
                        if let Some(real_entry) = fs.lookup(link, &entries) {
                            if let Some(r_entry) = entries.get(real_entry) {
                                entry = r_entry
                            }
                        } else {
                            info!("can't wrap up deleted symlink - pointed to file / directory doesn't exist: {:?}", path);
                        }
                    }

                    if let Entry::File { data, .. } = entry {
                        data.borrow_mut().deref_mut().tail(vec![path]).await
                    } else {
                        None
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
    pub fn process<'a>(&mut self) -> impl Stream<Item = LazyLineSerializer> + 'a {
        let events = FileSystem::stream_events(self.fs_cache.clone());

        debug!("Tailer starting with lookback: {:?}", self.lookback_config);
        events
            .then({
                let fs = self.fs_cache.clone();
                let lookback_config = self.lookback_config.clone();
                let initial_offsets = self.initial_offsets.clone();
                move |event| {
                    let fs = fs.clone();
                    let lookback_config = lookback_config.clone();
                    let initial_offsets = initial_offsets.clone();
                    async move {
                        Tailer::handle_event(
                            event,
                            initial_offsets,
                            lookback_config,
                            &fs.lock().expect("Couldn't lock fs"),
                        )
                        .await
                    }
                }
            })
            .filter_map(|x| async move { x })
            .flatten()
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::rule::{GlobRule, Rules};
    use pin_utils::pin_mut;
    use std::convert::TryInto;
    use std::fs::File;
    use std::io::Write;
    use std::panic;
    use tempfile::tempdir;
    use tokio_stream::StreamExt;

    static DELAY: Duration = Duration::from_millis(200);

    macro_rules! take_events {
        ($x: expr) => {{
            tokio::time::sleep(DELAY * 2).await;
            tokio::time::sleep(Duration::from_millis(20)).await;
            let mut events = Vec::new();
            loop {
                tokio::select! {
                    Some(item) = $x.next() => {
                        events.push(item);
                    }
                    _ = tokio::time::sleep(Duration::from_millis(300)) => {
                        break;
                    }
                }
            }
            events
        }};
    }

    fn run_test<T: FnOnce() + panic::UnwindSafe>(test: T) {
        let _ = env_logger::Builder::from_default_env().try_init();
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

                let line_write_count = (400 / (log_lines.as_bytes().len() + 1)) + 2;
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
                    DELAY,
                );

                let stream = tailer.process();
                pin_mut!(stream);

                let events = take_events!(stream);
                assert_eq!(events.len(), 0);

                tokio::time::sleep(tokio::time::Duration::from_millis(250)).await;
                writeln!(file, "{}", log_lines).expect("Couldn't write to temp log file...");
                file.sync_all().expect("Failed to sync file");

                let events = take_events!(stream);
                assert_eq!(events.len(), 1);
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
                    DELAY,
                );

                let stream = tailer.process();
                pin_mut!(stream);

                let events = take_events!(stream);
                assert_eq!(events.len(), 0);

                tokio::time::sleep(Duration::from_millis(500)).await;

                let log_lines2 = "This is a test log line2";
                writeln!(file, "{}", log_lines2).expect("Couldn't write to temp log file...");
                file.sync_all().expect("Failed to sync file");

                let events = take_events!(stream);
                assert_eq!(events.len(), 1);
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
                    DELAY,
                );

                let stream = tailer.process();
                pin_mut!(stream);

                let events = take_events!(stream);
                assert!(events.len() >= line_write_count);

                (0..5).for_each(|_| {
                    writeln!(file, "{}", log_lines).expect("Couldn't write to temp log file...");
                    file.sync_all().expect("Failed to sync file");
                });

                let events = take_events!(stream);
                assert_eq!(events.len(), 5);
            })
        })
    }
}

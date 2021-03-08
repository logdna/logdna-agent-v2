use crate::cache::entry::Entry;
use crate::cache::event::Event;
use crate::cache::tailed_file::LazyLineSerializer;
pub use crate::cache::DirPathBuf;
use crate::cache::FileSystem;
use crate::rule::Rules;
use metrics::Metrics;
use std::ops::DerefMut;
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
    fs_cache: Arc<Mutex<FileSystem>>,
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
    ) -> Result<impl Stream<Item = LazyLineSerializer> + 'a, std::io::Error> {
        let events = {
            match FileSystem::stream_events(self.fs_cache.clone(), buf) {
                Ok(event) => event,
                Err(e) => {
                    warn!("tailer stream raised exception: {:?}", e);
                    return Err(e);
                }
            }
        };

        Ok(events.then({
            let fs = self.fs_cache.clone();
            let lookback_config = self.lookback_config.clone();
            move |event|{

                let fs = fs.clone();
                let lookback_config = lookback_config.clone();
                async move {
                    let fs = fs.lock().expect("Couldn't lock fs");
                    match event {
                        Event::Initialize(entry_ptr) => {
                            debug!("Initialise Event");
                            // will initiate a file to it's current length
                            if let Some(entry) = fs.entries.borrow().get(entry_ptr){
                                let path = fs.resolve_direct_path(&entry, &fs.entries.borrow());

                                if let Entry::File { data, .. } = entry {
                                    match lookback_config {
                                        Lookback::Start => {
                                            info!("initialized {:?} with offset {}", path, 0);
                                            data.borrow_mut().deref_mut().seek(0).await.unwrap_or_else(|e| error!("error seeking {:?}", e))
                                        },
                                        Lookback::SmallFiles => {
                                            let mut len = path.metadata().map(|m| m.len()).unwrap_or(0);
                                            if len < 8192 {
                                                info!("initialized {:?} with len {} offset {}", path, len, 0);
                                                len = 0;
                                            } else{
                                                info!("initialized {:?} with offset {}", path, len);
                                            }
                                            data.borrow_mut().deref_mut().seek(len).await.unwrap_or_else(|e| error!("error seeking {:?}", e))
                                        },
                                        Lookback::None => {
                                            let len = path.metadata().map(|m| m.len()).unwrap_or(0);
                                            info!("initialized {:?} with offset {}", path, len);
                                            data.borrow_mut().deref_mut().seek(len).await.unwrap_or_else(|e| error!("error seeking {:?}", e))
                                        }
                                    }
                                    data.borrow_mut().tail(vec![path]).await
                                } else {
                                    None
                                }
                            } else {
                                None
                            }

                        }
                        Event::New(entry_ptr) => {
                            Metrics::fs().increment_creates();
                            debug!("New Event");
                            // similar to initiate but sets the offset to 0
                            if let Some(entry) = fs.entries.borrow().get(entry_ptr){
                                let paths = fs.resolve_valid_paths(&entry, &fs.entries.borrow());
                                if !paths.is_empty() {
                                    if let Entry::File {
                                        data,
                                        ..
                                    } = entry
                                    {
                                        info!("added {:?}", paths[0]);
                                        data.borrow_mut().tail(paths.clone()).await
                                    }
                                    else {
                                        None
                                    }

                                } else {
                                    None
                                }
                            } else {
                                None
                            }

                        }
                        Event::Write(entry_ptr) => {
                            Metrics::fs().increment_writes();
                            debug!("Write Event");
                            if let Some(entry) = fs.entries.borrow().get(entry_ptr){
                                let paths = fs.resolve_valid_paths(&entry, &fs.entries.borrow());
                                if !paths.is_empty() {

                                    if let  Entry::File {
                                        data,
                                        ..
                                    } = entry
                                    {
                                        data.borrow_mut().deref_mut().tail(paths).await
                                    } else {
                                        None
                                    }

                                } else {
                                    None
                                }
                            } else {
                                None
                            }

                        }
                        Event::Delete(entry_ptr) => {
                            Metrics::fs().increment_deletes();
                            debug!("Delete Event");
                            let ret = {
                                let entries = fs.entries.borrow();
                                if let Some(mut entry) = entries.get(entry_ptr){
                                    let paths = fs.resolve_valid_paths(&entry, &entries);
                                    if !paths.is_empty() {
                                        if let Entry::Symlink { link, .. } = entry {
                                            if let Some(real_entry) = fs.lookup(link, &entries) {
                                                if let Some(r_entry) = entries.get(real_entry) {
                                                    entry = r_entry
                                                }
                                            } else {
                                                info!("can't wrap up deleted symlink - pointed to file / directory doesn't exist: {:?}", paths[0]);
                                            }
                                    }

                                    if let Entry::File {
                                        data,
                                        ..
                                    } = entry
                                        {
                                            data.borrow_mut().deref_mut().tail(paths).await
                                        } else {
                                            None
                                        }
                                    } else {
                                        None
                                    }
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
                                        "Removed file information, currently tracking {} files",
                                        entries.len());
                                }
                            }
                            ret
                    }
                }
            }}}).filter_map(|x|async move {x}).flatten())
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
                );
                let mut buf = [0u8; 4096];

                let stream = tailer
                    .process(&mut buf)
                    .expect("failed to read events")
                    .timeout(std::time::Duration::from_millis(500));

                let write_files = async move {
                    tokio::time::delay_for(tokio::time::Duration::from_millis(250)).await;

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

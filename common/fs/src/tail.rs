use crate::cache::entry::EntryKind;
use crate::cache::event::Event;
use crate::cache::tailed_file::LazyLineSerializer;
use crate::cache::{EntryKey, Error as CacheError, FileSystem};
use types::lookback::Lookback;
use types::rule::Rules;

use metrics::Metrics;
use state::{FileId, SpanVec};
use std::collections::HashMap;
use std::ops::{Deref, DerefMut};
use tracing::{debug, info, warn};
use types::dir_path::DirPathBuf;

use futures::{ready, Future, Stream, StreamExt};

use std::time::{Duration, Instant};

use pin_project_lite::pin_project;
use std::pin::Pin;
use std::task::{Context, Poll};

use state::{FileOffsetFlushHandle, FileOffsetWriteHandle};

/// Tails files on a filesystem by inheriting events from a Watcher
pub struct Tailer {
    fs_cache: std::rc::Rc<std::cell::RefCell<FileSystem>>,
}

fn get_file_for_path(fs: &FileSystem, next_path: &std::path::Path) -> Option<EntryKey> {
    let entries = fs.entries.borrow();
    let mut next_path_opt = Some(next_path);
    while let Some(next_path) = next_path_opt {
        let next_entry_key = fs.lookup(next_path, &entries)?;
        match entries.get(next_entry_key).map(|entry| entry.kind()) {
            Some(EntryKind::Symlink { ref target, .. }) => next_path_opt = target.as_option_deref(),
            Some(EntryKind::File { .. }) => return Some(next_entry_key),
            _ => break,
        }
    }
    None
}

#[allow(clippy::await_holding_refcell_ref)]
async fn handle_event(
    event: Event,
    fs: &FileSystem,
) -> Option<impl Stream<Item = LazyLineSerializer>> {
    match event {
        Event::Initialize(entry_ptr) => {
            debug!("Initialize Event");
            // will initiate a file to it's current length
            let entries = fs.entries.borrow();
            let entry = entries.get(entry_ptr)?;
            let paths = fs.resolve_valid_paths(entry, &entries);
            match entry.kind() {
                EntryKind::File { data, .. } => {
                    let path_display = entry.path().display();
                    // If the file's passes the rules tail it
                    info!("initialize event for file {}", path_display);
                    if fs.is_initial_dir_target(entry.path()) {
                        return data.borrow_mut().tail(&paths).await;
                    }
                }
                EntryKind::Symlink { target, .. } => {
                    let sym_path = entry.path();
                    if let Some(target) = target.as_option() {
                        let final_target = get_file_for_path(fs, target)?;

                        let entries = &fs.entries.borrow();
                        let final_entry = entries.get(final_target)?;
                        let paths = fs.resolve_valid_paths(final_entry, entries);
                        debug!("paths: {:#?}", paths);
                        let path = final_entry.path();

                        info!(
                            "initialize event for symlink {}, final target {}",
                            sym_path.display(),
                            path.display()
                        );

                        let entry_key = get_file_for_path(fs, path)?;
                        if let EntryKind::File { data, .. } = &entries.get(entry_key)?.kind() {
                            info!(
                                "initialized symlink {:?} as {:?}",
                                sym_path.display(),
                                final_target
                            );
                            let mut data = data.borrow_mut();
                            return data.tail(&paths).await;
                        }
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
            if let EntryKind::File { data, .. } = entry.kind() {
                info!("added {:?}", paths[0]);
                return data.borrow_mut().tail(&paths).await;
            }
        }
        Event::RetryWrite((entry_ptr, _)) => {
            Metrics::fs().increment_writes();
            debug!("RetryWrite Event");
            let entries = fs.entries.borrow();
            let entry = entries.get(entry_ptr)?;

            let paths = fs.resolve_valid_paths(entry, &entries);
            if paths.is_empty() {
                return None;
            }

            if let EntryKind::File { data, .. } = entry.kind() {
                return data.borrow_mut().deref_mut().tail(&paths).await;
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

            if let EntryKind::File { data, .. } = entry.kind() {
                return data.borrow_mut().deref_mut().tail(&paths).await;
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
                    if let EntryKind::Symlink { target, .. } = entry.kind() {
                        if let Some(target) = target.as_option() {
                            if let Some(real_entry) = fs.lookup(target, &entries) {
                                if let Some(r_entry) = entries.get(real_entry) {
                                    entry = r_entry
                                }
                            } else {
                                info!("can't wrap up deleted symlink - pointed to file / directory doesn't exist: {:?}", paths);
                            }
                        }
                    }
                    if let EntryKind::File { data, .. } = entry.kind() {
                        let mut tailed_file = data.borrow_mut();
                        tailed_file.deref_mut().tail(&paths).await
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
//TODO: evaluate how to best handle this clippy error
#[allow(clippy::await_holding_refcell_ref)]
pub fn process(
    state: Tailer,
) -> Result<impl Stream<Item = Result<LazyLineSerializer, CacheError>>, std::io::Error> {
    let events = {
        match FileSystem::stream_events(state.fs_cache.clone()) {
            Ok(events) => events,
            Err(e) => {
                warn!("tailer stream raised exception: {:?}", e);
                return Err(e);
            }
        }
    };

    Ok(events
        .then({
            let fs = state.fs_cache;

            move |event_result| {
                let fs = fs.clone();

                async move {
                    match event_result {
                        Err(err) => Some(futures::stream::iter(vec![Err(err)]).left_stream()),
                        Ok(event) => {
                            let line = handle_event(event, fs.borrow().deref()).await;

                            line.map(|option_val| option_val.map(Ok).right_stream())
                        }
                    }
                }
            }
        })
        .filter_map(|x| async move { x })
        .flatten())
}

impl Tailer {
    /// Creates new instance of Tailer
    pub fn new(
        watched_dirs: Vec<DirPathBuf>,
        rules: Rules,
        lookback_config: Lookback,
        initial_offsets: Option<HashMap<FileId, SpanVec>>,
        fo_state_handles: Option<(FileOffsetWriteHandle, FileOffsetFlushHandle)>,
        deletion_ack_sender: async_channel::Sender<Vec<std::path::PathBuf>>,
        retry_event_delay: Option<std::time::Duration>,
    ) -> Self {
        Self {
            fs_cache: std::rc::Rc::new(std::cell::RefCell::new(FileSystem::new(
                watched_dirs,
                lookback_config,
                initial_offsets.unwrap_or_default(),
                rules,
                fo_state_handles,
                deletion_ack_sender,
                retry_event_delay,
            ))),
        }
    }
}

pin_project! {
    #[must_use = "streams do nothing unless polled"]
    pub struct RestartingTailer<P, C, F, S: Stream, Fut> {
        params: P,
        restart: C,
        f: F,
        #[pin]
        stream: S,
        #[pin]
        pending: Option<Fut>,
        #[pin]
        start_time: Option<Instant>,
        restart_interval: Duration,  // zero - no restarts
    }
}

impl<P, C, F, S: Stream, T, Fut> RestartingTailer<P, C, F, S, Fut>
where
    C: Fn(&T) -> bool,
    F: FnMut(&P) -> Fut,
    S: Stream<Item = T>,
    Fut: Future<Output = S>,
{
    pub async fn new(params: P, restart: C, mut f: F, restart_interval: Duration) -> Self {
        let stream = f(&params).await;
        Self {
            params,
            restart,
            f,
            stream,
            pending: None,
            start_time: Some(Instant::now()),
            restart_interval,
        }
    }
}

impl<P, C, F, S: Stream, T, Fut> Stream for RestartingTailer<P, C, F, S, Fut>
where
    C: Fn(&T) -> bool,
    F: FnMut(&P) -> Fut,
    S: Stream<Item = T>,
    Fut: Future<Output = S>,
{
    type Item = T;

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.stream.size_hint()
    }

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<T>> {
        let mut this = self.project();

        Poll::Ready(loop {
            // check for periodic restart
            if let Some(start_time) = this.start_time.as_mut().as_pin_mut() {
                if this.restart_interval.as_secs() > 0
                    && start_time.elapsed().as_secs() > this.restart_interval.as_secs()
                {
                    this.start_time.set(Some(Instant::now()));
                    let stream_fut = (this.f)(this.params);
                    this.pending.set(Some(stream_fut));
                    info!(
                        "restarting stream, interval={}",
                        this.restart_interval.as_secs()
                    );
                }
            }
            if let Some(p) = this.pending.as_mut().as_pin_mut() {
                let stream = ready!(p.poll(cx));
                this.pending.set(None);
                this.stream.set(stream);
            } else if let Some(value) = ready!(this.stream.as_mut().poll_next(cx)) {
                if (this.restart)(&value) {
                    let stream_fut = (this.f)(this.params);
                    this.pending.set(Some(stream_fut));
                } else {
                    break Some(value);
                }
            } else {
                break None;
            }
        })
    }
}

#[cfg(test)]
mod test {
    use super::*;
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    use pin_utils::pin_mut;
    use std::cell::Cell;
    use std::convert::TryInto;
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    use std::fs::File;
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    use std::io::Write;
    use std::panic;
    use std::rc::Rc;
    use std::time::Duration;
    use tempfile::tempdir;
    use tokio_stream::StreamExt;
    use types::rule::{RuleDef, Rules};

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    static DELAY: Duration = Duration::from_millis(200);

    #[cfg(any(target_os = "windows", target_os = "linux"))]
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

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    fn run_test<T: FnOnce() + panic::UnwindSafe>(test: T) {
        let _ = env_logger::Builder::from_default_env().try_init();
        let result = panic::catch_unwind(|| {
            test();
        });

        assert!(result.is_ok())
    }

    #[test_log::test]
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    fn none_lookback() {
        run_test(|| {
            tokio_test::block_on(async {
                let mut rules = Rules::new();
                rules.add_inclusion(RuleDef::glob_rule(r"**").unwrap());

                let log_lines = "This is a test log line";
                tracing::debug!("{}", log_lines.len());
                let dir = tempdir().expect("Couldn't create temp dir...");

                let file_path = dir.path().join("test.log");

                let mut file = File::create(&file_path).expect("Couldn't create temp log file...");

                let line_write_count = (400 / (log_lines.len() + 1)) + 2;
                (0..line_write_count).for_each(|_| {
                    writeln!(file, "{log_lines}").expect("Couldn't write to temp log file...")
                });
                file.sync_all().expect("Failed to sync file");

                let tailer = Tailer::new(
                    vec![dir
                        .path()
                        .try_into()
                        .unwrap_or_else(|_| panic!("{:?} is not a directory!", dir.path()))],
                    rules,
                    Lookback::None,
                    None,
                    None,
                    async_channel::unbounded().0,
                    None,
                );

                let stream = process(tailer).expect("failed to read events");
                pin_mut!(stream);

                let events = take_events!(stream);
                assert_eq!(events.len(), 0, "{events:#?}");

                tokio::time::sleep(tokio::time::Duration::from_millis(250)).await;
                writeln!(file, "{log_lines}").expect("Couldn't write to temp log file...");
                file.sync_all().expect("Failed to sync file");

                let events = take_events!(stream);
                assert_eq!(events.len(), 1, "{events:#?}");
            });
        });
    }
    #[test]
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    fn smallfiles_lookback() {
        run_test(|| {
            tokio_test::block_on(async {
                let mut rules = Rules::new();
                rules.add_inclusion(RuleDef::glob_rule(r"**").unwrap());

                let log_lines1 = "This is a test log line";
                tracing::debug!("{}", log_lines1.len());
                let dir = tempdir().expect("Couldn't create temp dir...");

                let file_path = dir.path().join("test.log");

                let mut file = File::create(&file_path).expect("Couldn't create temp log file...");

                let line_write_count = (8192 / (log_lines1.len() + 1)) + 2;
                (0..line_write_count).for_each(|_| {
                    writeln!(file, "{log_lines1}").expect("Couldn't write to temp log file...")
                });
                file.sync_all().expect("Failed to sync file");

                let tailer = Tailer::new(
                    vec![dir
                        .path()
                        .try_into()
                        .unwrap_or_else(|_| panic!("{:?} is not a directory!", dir.path()))],
                    rules,
                    Lookback::SmallFiles,
                    None,
                    None,
                    async_channel::unbounded().0,
                    None,
                );

                let stream = process(tailer).expect("failed to read events");
                pin_mut!(stream);

                let events = take_events!(stream);
                assert_eq!(events.len(), 0);

                tokio::time::sleep(tokio::time::Duration::from_millis(250)).await;

                let log_lines2 = "This is a test log line2";
                writeln!(file, "{log_lines2}").expect("Couldn't write to temp log file...");
                file.sync_all().expect("Failed to sync file");

                let events = take_events!(stream);
                assert_eq!(events.len(), 1, "{events:#?}");
            });
        });
    }

    #[test]
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    fn start_lookback() {
        run_test(|| {
            tokio_test::block_on(async {
                let mut rules = Rules::new();
                rules.add_inclusion(RuleDef::glob_rule(r"**").unwrap());

                let log_lines = "This is a test log line";
                let dir = tempdir().expect("Couldn't create temp dir...");

                let file_path = dir.path().join("test.log");

                let mut file = File::create(&file_path).expect("Couldn't create temp log file...");
                let line_write_count = (8_388_608 / (log_lines.len() + 1)) + 1;
                (0..line_write_count).for_each(|i| {
                    writeln!(file, "{log_lines}, {i}").expect("Couldn't write to temp log file...")
                });
                file.sync_all().expect("Failed to sync file");

                let tailer = Tailer::new(
                    vec![dir
                        .path()
                        .try_into()
                        .unwrap_or_else(|_| panic!("{:?} is not a directory!", dir.path()))],
                    rules,
                    Lookback::Start,
                    None,
                    None,
                    async_channel::unbounded().0,
                    None,
                );

                let stream = process(tailer).expect("failed to read events");
                pin_mut!(stream);

                let events = take_events!(stream);
                assert!(events.len() >= line_write_count);

                (0..5).for_each(|_| {
                    writeln!(file, "{log_lines}").expect("Couldn't write to temp log file...");
                    file.sync_all().expect("Failed to sync file");
                });

                let events = take_events!(stream);
                assert_eq!(events.len(), 5);
            })
        })
    }

    #[tokio::test]
    async fn restart_tailer_with_empty_stream() {
        let mut rules = Rules::new();
        rules.add_inclusion(RuleDef::glob_rule(r"**").unwrap());

        let dir = tempdir().expect("Couldn't create temp dir...");
        let watched_dirs: Vec<DirPathBuf> = vec![dir
            .path()
            .try_into()
            .unwrap_or_else(|_| panic!("{:?} is not a directory!", dir.path()))];

        let initial_offsets: Option<HashMap<FileId, u64>> = None;

        let rt = RestartingTailer::new(
            (watched_dirs, rules, Lookback::None, initial_offsets),
            |_: &String| false,
            |&_| async { futures::stream::empty() },
            Duration::from_secs(3600),
        )
        .await;

        let result = rt.collect::<Vec<String>>().await;
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn restart_tailer_exhausting_stream() {
        let mut rules = Rules::new();
        rules.add_inclusion(RuleDef::glob_rule(r"**").unwrap());

        let dir = tempdir().expect("Couldn't create temp dir...");
        let watched_dirs: Vec<DirPathBuf> = vec![dir
            .path()
            .try_into()
            .unwrap_or_else(|_| panic!("{:?} is not a directory!", dir.path()))];

        let initial_offsets: Option<HashMap<FileId, u64>> = None;

        let rt = RestartingTailer::new(
            (watched_dirs, rules, Lookback::None, initial_offsets),
            |_: &usize| false,
            |&_| async { futures::stream::iter(vec![1, 2, 3]) },
            Duration::from_secs(3600),
        )
        .await;

        let result = rt.collect::<Vec<usize>>().await;
        assert_eq!(result, vec![1, 2, 3]);
    }

    #[tokio::test]
    async fn restart_tailer_multiple_recoveries() {
        let mut rules = Rules::new();
        rules.add_inclusion(RuleDef::glob_rule(r"**").unwrap());

        let dir = tempdir().expect("Couldn't create temp dir...");
        let watched_dirs: Vec<DirPathBuf> = vec![dir
            .path()
            .try_into()
            .unwrap_or_else(|_| panic!("{:?} is not a directory!", dir.path()))];

        let initial_offsets: Option<HashMap<FileId, u64>> = None;

        let global_stream_count: Rc<Cell<usize>> = Rc::new(Cell::new(1));
        let rt = RestartingTailer::new(
            (watched_dirs, rules, Lookback::None, initial_offsets),
            // For this test, any number divisible by 4 should trigger the recovery behavior.
            // Those values will be dropped from the collected output.
            |n: &usize| *n % 4 == 0,
            // Builds a closure to produce a stream of incrementing integers using a global
            // state such that a number is only produced once. A number, once produced, will
            // never be produced again across all instances of streams obtained from the closure.
            |&_| async {
                let state = global_stream_count.clone();
                Box::pin(futures::stream::unfold(state, |state| async {
                    let cur = state.get();
                    state.set(cur + 1);
                    Some((cur, state))
                }))
            },
            Duration::from_secs(3600),
        )
        .await;

        // // Limit the recovery stream to producing 9 elements. The value at 4 and 8 will be skipped
        // // which verifies that the stream was able to correctly recover more than once.
        let result = rt.take(8).collect::<Vec<usize>>().await;
        assert_eq!(result, vec![1, 2, 3, 5, 6, 7, 9, 10]);
    }

    #[tokio::test]
    async fn restart_tailer_successive_recoveries() {
        let mut rules = Rules::new();
        rules.add_inclusion(RuleDef::glob_rule(r"**").unwrap());

        let dir = tempdir().expect("Couldn't create temp dir...");
        let watched_dirs: Vec<DirPathBuf> = vec![dir
            .path()
            .try_into()
            .unwrap_or_else(|_| panic!("{:?} is not a directory!", dir.path()))];

        let initial_offsets: Option<HashMap<FileId, u64>> = None;

        let global_stream_count: Rc<Cell<usize>> = Rc::new(Cell::new(1));
        let rt = RestartingTailer::new(
            (watched_dirs, rules, Lookback::None, initial_offsets),
            // For this test, the recovery is triggered on both 2 and 3. This will cause two recovery
            // attempts in succession.
            |n: &usize| *n == 2 || *n == 3,
            // Builds a closure to produce a stream of incrementing integers up to 6, exclusive,
            // using a global state such that a number is only produced once. A number, once produced,
            // will never be produced again across all instances of streams obtained from the closure.
            |&_| async {
                let state = global_stream_count.clone();
                Box::pin(futures::stream::unfold(state, |state| async {
                    let cur = state.get();
                    if cur < 6 {
                        state.set(cur + 1);
                        Some((cur, state))
                    } else {
                        None
                    }
                }))
            },
            Duration::from_secs(3600),
        )
        .await;

        let result = rt.collect::<Vec<usize>>().await;
        assert_eq!(result, vec![1, 4, 5]);
    }

    #[test_log::test(tokio::test)]
    async fn tailer_retries() {
        use types::sources::RetryableLine;
        let mut rules = Rules::new();
        rules.add_inclusion(RuleDef::glob_rule(r"**").unwrap());

        let dir = tempdir().expect("Couldn't create temp dir...");

        let file_path = dir.path().join("test.log");

        let mut file = File::create(&file_path).expect("Couldn't create temp log file...");

        let tailer = Tailer::new(
            vec![dir
                .path()
                .try_into()
                .unwrap_or_else(|_| panic!("{:?} is not a directory!", dir.path()))],
            rules,
            Lookback::Start,
            None,
            None,
            async_channel::unbounded().0,
            None,
        );

        let messages = &["0", "1", "2", "3", "4"];
        messages.iter().for_each(|message| {
            writeln!(file, "{message}").expect("Couldn't write to temp log file...");
            file.sync_all().expect("Failed to sync file");
        });

        let stream = process(tailer).expect("failed to read events");
        pin_mut!(stream);
        let events = take_events!(stream);
        assert_eq!(events.len(), 5);
        for event in events.iter() {
            event
                .as_ref()
                .unwrap()
                .retry(Some(Duration::from_secs(1)))
                .unwrap();
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
        let mut events = take_events!(stream);
        while events.is_empty() {
            events = take_events!(stream);
        }
        assert_eq!(events.len(), 5);
        for event in events.iter() {
            event
                .as_ref()
                .unwrap()
                .retry(Some(Duration::from_secs(1)))
                .unwrap();
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
        let mut events = take_events!(stream);
        while events.is_empty() {
            events = take_events!(stream);
        }
        assert_eq!(events.len(), 5);
        for event in events.iter() {
            event
                .as_ref()
                .unwrap()
                .retry(Some(Duration::from_secs(1)))
                .unwrap();
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
        let mut events = take_events!(stream);
        while events.is_empty() {
            events = take_events!(stream);
        }
        assert_eq!(events.len(), 5);
        for event in events.iter() {
            event
                .as_ref()
                .unwrap()
                .retry(Some(Duration::from_secs(1)))
                .unwrap();
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
        let mut events = take_events!(stream);
        while events.is_empty() {
            events = take_events!(stream);
        }
        assert_eq!(events.len(), 5);
        for event in events.iter() {
            event
                .as_ref()
                .unwrap()
                .retry(Some(Duration::from_secs(1)))
                .unwrap();
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
        let mut events = take_events!(stream);
        while events.is_empty() {
            events = take_events!(stream);
        }
        assert_eq!(events.len(), 5);
        for event in events.iter() {
            event.as_ref().unwrap().commit().unwrap();
        }
    }
}

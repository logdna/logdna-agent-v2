use crate::cache::entry::Entry;
use crate::cache::event::Event;
use crate::cache::tailed_file::LazyLineSerializer;
pub use crate::cache::DirPathBuf;
use crate::cache::{EntryKey, Error as CacheError, FileSystem, EVENT_STREAM_BUFFER_COUNT};
use crate::lookback::Lookback;
use crate::rule::Rules;

use metrics::Metrics;
use state::{FileId, SpanVec};
use std::collections::HashMap;
use std::ops::{Deref, DerefMut};

use std::sync::Arc;
use tokio::sync::Mutex;

use futures::{ready, Future, Stream, StreamExt};

use std::time::Duration;

use pin_project_lite::pin_project;
use std::pin::Pin;
use std::task::{Context, Poll};

type SyncHashMap<K, V> = Arc<Mutex<HashMap<K, V>>>;

/// Tails files on a filesystem by inheriting events from a Watcher
pub struct Tailer {
    fs_cache: Arc<Mutex<FileSystem>>,
    event_times: SyncHashMap<EntryKey, (usize, time::OffsetDateTime)>,
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
            let path = entry.path().to_path_buf();
            match entry {
                Entry::File { data, .. } => {
                    let path_display = entry.path().display();
                    // If the file's passes the rules tail it
                    info!("initialize event for file {}", path_display);
                    if fs.is_initial_dir_target(&path) {
                        return data.borrow_mut().tail(vec![path]).await;
                    }
                }
                Entry::Symlink { link, .. } => {
                    let sym_path = path;
                    let final_target = get_file_for_path(&fs, link)?;

                    let entries = &fs.entries.borrow();
                    let path = entries.get(final_target)?.path();

                    info!(
                        "initialize event for symlink {}, final target {}",
                        sym_path.display(),
                        path.display()
                    );

                    let entry_key = get_file_for_path(fs, &path)?;
                    if let Entry::File { data, .. } = &entries.get(entry_key)? {
                        info!(
                            "initialized symlink {:?} as {:?}",
                            sym_path.display(),
                            final_target
                        );
                        let mut data = data.borrow_mut();
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
        .enumerate()
        .then({
            let fs = state.fs_cache.clone();
            let event_times = state.event_times;

            move |(event_idx, (event_result, event_time))| {
                let fs = fs.clone();
                let event_times = event_times.clone();

                async move {
                    match event_result {
                        Err(err) => Some(futures::stream::iter(vec![Err(err)]).left_stream()),
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

                            let line = handle_event(event, fs.lock().await.deref()).await;

                            let line = line.map(|option_val| option_val.map(Ok).right_stream());

                            if let Some((key, _)) = key_and_previous_event_time {
                                let mut event_times = event_times.lock().await;
                                let new_event_time = time::OffsetDateTime::now_utc();
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

impl Tailer {
    /// Creates new instance of Tailer
    pub fn new(
        watched_dirs: Vec<DirPathBuf>,
        rules: Rules,
        lookback_config: Lookback,
        initial_offsets: Option<HashMap<FileId, SpanVec>>,
        event_delay: Duration,
    ) -> Self {
        Self {
            fs_cache: Arc::new(Mutex::new(FileSystem::new(
                watched_dirs,
                lookback_config,
                initial_offsets.unwrap_or_default(),
                rules,
                event_delay,
            ))),
            event_times: Arc::new(Mutex::new(HashMap::new())),
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
        pending: Option<Fut>
    }
}

impl<P, C, F, S: Stream, T, Fut> RestartingTailer<P, C, F, S, Fut>
where
    C: Fn(&T) -> bool,
    F: FnMut(&P) -> Fut,
    S: Stream<Item = T>,
    Fut: Future<Output = S>,
{
    pub async fn new(params: P, restart: C, mut f: F) -> Self {
        let stream = f(&params).await;
        Self {
            params,
            restart,
            f,
            stream,
            pending: None,
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
    use crate::rule::{RuleDef, Rules};
    use crate::test::LOGGER;
    use http::types::body::LineBufferMut;
    use pin_utils::pin_mut;
    use std::cell::Cell;
    use std::convert::TryInto;
    use std::fs::File;
    use std::io::Write;
    use std::panic;
    use std::rc::Rc;
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
                rules.add_inclusion(RuleDef::glob_rule(r"**").unwrap());

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

                let tailer = Tailer::new(
                    vec![dir
                        .path()
                        .try_into()
                        .unwrap_or_else(|_| panic!("{:?} is not a directory!", dir.path()))],
                    rules,
                    Lookback::None,
                    None,
                    DELAY,
                );

                let stream = process(tailer)
                    .expect("failed to read events")
                    .timeout(std::time::Duration::from_millis(500));
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
                rules.add_inclusion(RuleDef::glob_rule(r"**").unwrap());

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

                let tailer = Tailer::new(
                    vec![dir
                        .path()
                        .try_into()
                        .unwrap_or_else(|_| panic!("{:?} is not a directory!", dir.path()))],
                    rules,
                    Lookback::SmallFiles,
                    None,
                    DELAY,
                );

                let stream = process(tailer)
                    .expect("failed to read events")
                    .timeout(std::time::Duration::from_millis(500));
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
                rules.add_inclusion(RuleDef::glob_rule(r"**").unwrap());

                let log_lines = "This is a test log line";
                let dir = tempdir().expect("Couldn't create temp dir...");

                let file_path = dir.path().join("test.log");

                let mut file = File::create(&file_path).expect("Couldn't create temp log file...");
                let line_write_count = (8_388_608 / (log_lines.as_bytes().len() + 1)) + 1;
                (0..line_write_count).for_each(|_| {
                    writeln!(file, "{}", log_lines).expect("Couldn't write to temp log file...")
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
                    DELAY,
                );

                let stream = process(tailer)
                    .expect("failed to read events")
                    .timeout(std::time::Duration::from_millis(500));
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
        )
        .await;

        let result = rt.collect::<Vec<usize>>().await;
        assert_eq!(result, vec![1, 4, 5]);
    }
}

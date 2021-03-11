use crate::error::JournalError;
use futures::{channel::oneshot, stream::Stream as FutureStream};
use http::types::body::LineBuilder;
use log::{info, warn};
use metrics::Metrics;
use std::{
    mem::drop,
    path::PathBuf,
    pin::Pin,
    sync::{
        mpsc::{sync_channel, Receiver, TryRecvError},
        Arc, Mutex,
    },
    task::{Context, Poll, Waker},
    thread::{self, JoinHandle},
    time::{Duration, SystemTime},
};
use systemd::journal::{Journal, JournalFiles, JournalRecord, JournalSeek};

const KEY_MESSAGE: &str = "MESSAGE";
const KEY_SYSTEMD_UNIT: &str = "_SYSTEMD_UNIT";
const KEY_SYSLOG_IDENTIFIER: &str = "SYSLOG_IDENTIFIER";
const KEY_CONTAINER_NAME: &str = "CONTAINER_NAME";
const DEFAULT_APP: &str = "UNKNOWN_SYSTEMD_APP";

#[derive(Clone)]
pub enum Path {
    Directory(PathBuf),
    Files(Vec<PathBuf>),
}

struct SharedState {
    waker: Option<Waker>,
}

pub struct Stream {
    thread: Option<JoinHandle<()>>,
    receiver: Option<Receiver<LineBuilder>>,
    shared_state: Arc<Mutex<SharedState>>,
    path: Path,
    thread_stop_chan: Option<oneshot::Sender<()>>,
}

impl Stream {
    pub fn new(path: Path) -> Self {
        let mut stream = Self {
            thread: None,
            receiver: None,
            shared_state: Arc::new(Mutex::new(SharedState { waker: None })),
            path,
            thread_stop_chan: None,
        };

        stream.spawn_thread();
        stream
    }

    fn spawn_thread(&mut self) {
        self.drop_thread();

        let (stop_sender, mut stop_receiver) = oneshot::channel();
        self.thread_stop_chan = Some(stop_sender);

        let (sender, receiver) = sync_channel(100);
        let thread_shared_state = self.shared_state.clone();
        let path = self.path.clone();
        let thread = thread::spawn(move || {
            let mut journal = Reader::new(path);

            let call_waker = || {
                let mut shared_state = match thread_shared_state.lock() {
                    Ok(shared_state) => shared_state,
                    Err(e) => {
                        // we can't wake up the stream so it will hang indefinitely; need
                        // to panic here
                        panic!(
                            "journald's worker thread unable to access shared state: {:?}",
                            e
                        );
                    }
                };
                if let Some(waker) = shared_state.waker.take() {
                    waker.wake();
                }
            };

            while let Ok(None) = stop_receiver.try_recv() {
                match journal.process_next_record() {
                    Ok(Some(line)) => {
                        if let Err(e) = sender.send(line) {
                            warn!(
                                "journald's worker thread unable to communicate with main thread: {}",
                                e
                            );
                            break;
                        }

                        call_waker();
                    }
                    Ok(None) => {}
                    Err(JournalError::RecordMissingField(e)) => {
                        warn!("dropping journald record: {:?}", e);
                    }
                    Err(JournalError::BadRead(e)) => {
                        warn!("unable to read from journald: {:?}", e);
                        break;
                    }
                }

                if let Err(e) = journal.reader.wait(Some(Duration::from_millis(100))) {
                    warn!(
                        "journald's worker thread unable to poll journald for next record: {}",
                        e
                    );
                    break;
                }
            }

            // some sort of error has occurred. Explicitly drop the sender before waking up the
            // stream to prevent a race condition
            drop(sender);
            call_waker();
        });

        self.thread = Some(thread);
        self.receiver = Some(receiver);
    }

    fn drop_thread(&mut self) {
        // if this fails it means the thread probably panicked and the receiver is offline
        // the thread should print some logs about this so don't need to overload with more logs
        if let Some(stop_chan) = self.thread_stop_chan.take() {
            let _ = stop_chan.send(());
        }

        if let Some(thread) = self.thread.take() {
            if let Err(e) = thread.join() {
                warn!("unable to join journald's worker thread: {:?}", e)
            }
        }
    }
}

impl FutureStream for Stream {
    type Item = LineBuilder;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Option<Self::Item>> {
        let mut self_ = self.as_mut();

        if let Some(ref receiver) = self_.receiver {
            match receiver.try_recv() {
                // TODO: Find a way to reuse vectors or just generally make this more efficient
                Ok(line) => {
                    return Poll::Ready(Some(line));
                }
                Err(TryRecvError::Disconnected) => {
                    warn!("journald's main thread unable to read from worker thread, restarting worker thread...");
                    self_.drop_thread();
                    self_.spawn_thread();
                }
                _ => {}
            }
        } else {
            warn!(
                "journald's main thread missing connection to worker thread, shutting down stream"
            );
            return Poll::Ready(None);
        }

        let mut shared_state = self_.shared_state.lock().unwrap();
        shared_state.waker = Some(cx.waker().clone());
        Poll::Pending
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        self.drop_thread();
    }
}

struct Reader {
    reader: Journal,
}

impl Reader {
    fn new(path: Path) -> Self {
        let mut reader = match path {
            Path::Directory(path) => Journal::open_directory(&path, JournalFiles::All, false)
                .expect("Could not open journald reader for directory"),
            Path::Files(paths) => {
                let paths: Vec<&std::path::Path> = paths.iter().map(PathBuf::as_path).collect();
                Journal::open_files(&paths).expect("Could not open journald reader for paths")
            }
        };
        reader
            .seek(JournalSeek::Tail)
            .expect("Could not seek to tail of journald logs");

        Self { reader }
    }

    fn process_next_record(&mut self) -> Result<Option<LineBuilder>, JournalError> {
        let record = match self.reader.next_entry() {
            Ok(Some(record)) => record,
            Ok(None) => return Ok(None),
            Err(e) => return Err(JournalError::BadRead(e)),
        };

        match self
            .reader
            .timestamp()
            .ok()
            .map(|timestamp| SystemTime::now().duration_since(timestamp).ok())
            .flatten()
        {
            Some(duration) => {
                // Reject any records with a timestamp older than 30 seconds
                if duration >= Duration::from_secs(30) {
                    info!("Received a stale journald record, reseeking pointer");
                    if let Err(e) = self.reader.seek(JournalSeek::Tail) {
                        return Err(JournalError::BadRead(e));
                    }
                }
            }
            None => {
                warn!("Unable to read timestamp associated with journald record");
            }
        } //TODO: Actually bake the timestamp into the outgoing line

        self.process_default_record(&record)
    }

    fn process_default_record(
        &self,
        record: &JournalRecord,
    ) -> Result<Option<LineBuilder>, JournalError> {
        let message = match record.get(KEY_MESSAGE) {
            Some(message) => message,
            None => {
                warn!("unable to get message of journald record");
                return Err(JournalError::RecordMissingField(KEY_MESSAGE.into()));
            }
        };

        let default_app = String::from(DEFAULT_APP);
        let app = record
            .get(KEY_CONTAINER_NAME)
            .or_else(|| record.get(KEY_SYSTEMD_UNIT))
            .or_else(|| record.get(KEY_SYSLOG_IDENTIFIER))
            .unwrap_or(&default_app);

        Metrics::journald().increment_lines();
        Metrics::journald().add_bytes(message.len() as u64);
        Ok(Some(LineBuilder::new().line(message).file(app)))
    }
}

#[cfg(all(feature = "journald_tests", test))]
#[cfg_attr(not(target_os = "linux"), ignore)]
mod tests {
    use super::*;
    use futures::stream::StreamExt;
    use serial_test::serial;
    use std::{thread::sleep, time::Duration};
    use systemd::journal;
    use tokio::time::timeout;

    const JOURNALD_LOG_PATH: &str = "/var/log/journal";

    //TODO: Tests are incredibly finicky. we can restructure them to be more robust and work in a
    //busy journald environment
    #[tokio::test]
    #[serial]
    async fn reader_gets_new_logs() {
        journal::print(1, "Reader got the correct line!");
        sleep(Duration::from_millis(50));
        let mut reader = Reader::new(Path::Directory(JOURNALD_LOG_PATH.into()));

        let record_status = reader.process_next_record();
        if let Ok(Some(line)) = record_status {
            assert!(line.line.is_some());
            if let Some(line_str) = line.line {
                assert_eq!(line_str, "Reader got the correct line!");
            }
        }

        assert!(matches!(reader.process_next_record(), Ok(None)));
    }

    #[tokio::test]
    #[serial]
    async fn stream_gets_new_logs() {
        journal::print(1, "Reader got the correct line 1!");
        sleep(Duration::from_millis(50));
        let mut stream = Stream::new(Path::Directory(JOURNALD_LOG_PATH.into()));
        sleep(Duration::from_millis(50));
        journal::print(1, "Reader got the correct line 2!");

        let first_line = match timeout(Duration::from_millis(500), stream.next()).await {
            Err(e) => {
                panic!("unable to grab first batch of lines from stream: {:?}", e);
            }
            Ok(None) => {
                panic!("expected to get a line from journald stream");
            }
            Ok(Some(batch)) => batch,
        };

        assert!(first_line.line.is_some());
        if let Some(line_str) = &first_line.line {
            assert_eq!(line_str, "Reader got the correct line 1!");
        }

        let second_line = match timeout(Duration::from_millis(500), stream.next()).await {
            Err(e) => {
                panic!("unable to grab second batch of lines from stream: {:?}", e);
            }
            Ok(None) => {
                panic!("expected to get a line from journald stream");
            }
            Ok(Some(batch)) => batch,
        };

        assert!(second_line.line.is_some());
        if let Some(line_str) = &second_line.line {
            assert_eq!(line_str, "Reader got the correct line 2!");
        }

        match timeout(Duration::from_millis(50), stream.next()).await {
            Err(_) => {}
            _ => panic!("did not expect any more events from journald stream"),
        }
    }
}

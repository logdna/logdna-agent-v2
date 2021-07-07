extern crate notify;

use futures::{stream, Stream};
use notify::{DebouncedEvent, Error as NotifyError, Watcher as NotifyWatcher};
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;
use time::OffsetDateTime;

type PathId = std::path::PathBuf;

#[cfg(target_os = "linux")]
type OsWatcher = notify::INotifyWatcher;
// #[cfg(target_os = "windows")]
// type OsWatcher = notify::ReadDirectoryChangesWatcher;
// #[cfg(not(any(target_os = "linux", target_os = "windows")))]
#[cfg(not(any(target_os = "linux")))]
type OsWatcher = notify::PollWatcher;

#[derive(Debug, PartialEq)]
/// Event wrapper to that hides platform and implementation details.
///
/// Gives us the ability to hide/map events from the used library and minimize code changes in
/// case the notify library adds breaking changes.
pub enum Event {
    /// `NoticeRemove` is emitted immediately after a remove or rename event for the path.
    ///
    /// The file will continue to exist until its last file handle is closed.
    ///
    /// `Write` events might follow as part of the normal flow.
    Remove(PathId),

    /// `Create` is emitted when a file or directory has been created and no events were detected
    /// for the path within the specified time frame.
    ///
    /// `Create` events have a higher priority than `Write`, `Write` will not be
    /// emitted if they are detected before the `Create` event has been emitted.
    Create(PathId),

    /// `Write` is emitted when a file has been written to and no events were detected for the path
    /// within the specified time frame.
    ///
    /// Upon receiving a `Create` event for a directory, it is necessary to scan the newly created
    /// directory for contents. The directory can contain files or directories if those contents
    /// were created before the directory could be watched, or if the directory was moved into the
    /// watched directory.
    Write(PathId),

    /// `Rename` is emitted when a file or directory has been moved within a watched directory and
    /// no events were detected for the new path within the specified time frame.
    ///
    /// The first path contains the source, the second path the destination.
    Rename(PathId, PathId),

    /// `Rescan` is emitted immediately after a problem has been detected that makes it necessary
    /// to re-scan the watched directories.
    Rescan,

    /// `Error` is emitted immediately after a error has been detected.
    ///
    ///  This event may contain a path for which the error was detected.
    Error(Error, Option<PathId>),
}

#[derive(Debug, PartialEq)]
pub enum Error {
    /// Generic error
    ///
    /// May be used in cases where a platform specific error is mapped to this type
    Generic(String),

    /// I/O errors
    Io(String),

    /// The provided path does not exist
    PathNotFound,

    /// Attempted to remove a watch that does not exist
    WatchNotFound,
}

pub enum RecursiveMode {
    Recursive,
    NonRecursive,
}

pub struct Watcher {
    watcher: OsWatcher,
    rx: Rc<async_channel::Receiver<DebouncedEvent>>,
}

impl Watcher {
    pub fn new(delay: Duration) -> Self {
        let (watcher_tx, blocking_rx) = std::sync::mpsc::channel();

        let watcher = OsWatcher::new(watcher_tx, delay).unwrap();
        let (async_tx, rx) = async_channel::unbounded();
        tokio::task::spawn_blocking(move || {
            while let Ok(event) = blocking_rx.recv() {
                // Safely ignore closed error as it's caused by the runtime being dropped
                // It can't result in a `TrySendError::Full` as it's an unbounded channel
                let _ = async_tx.try_send(event);
            }
        });

        Self {
            watcher,
            rx: Rc::new(rx),
        }
    }

    /// Adds a new directory or file to watch
    pub fn watch<P: AsRef<Path>>(&mut self, path: P, mode: RecursiveMode) -> Result<(), Error> {
        self.watcher.watch(path, mode.into()).map_err(|e| e.into())
    }

    /// Removes a file or directory
    pub fn unwatch<P: AsRef<Path>>(&mut self, path: P) -> Result<(), Error> {
        self.watcher.unwatch(path).map_err(|e| e.into())
    }

    /// Removes a file or directory, ignoring watch not found errors.
    ///
    /// Returns Ok(true) when watch was found and removed.
    pub fn unwatch_if_exists<P: AsRef<Path>>(&mut self, path: P) -> Result<bool, Error> {
        match self.watcher.unwatch(path).map_err(|e| e.into()) {
            Ok(_) => Ok(true),
            Err(e) => match e {
                // Ignore watch not found
                Error::WatchNotFound => Ok(false),
                _ => Err(e),
            },
        }
    }

    /// Starts receiving the watcher events
    pub fn receive(&self) -> impl Stream<Item = (Event, OffsetDateTime)> {
        let rx = Rc::clone(&self.rx);
        stream::unfold(rx, |rx| async move {
            loop {
                let received = rx.recv().await.expect("channel can not be closed");
                if let Some(mapped_event) = match received {
                    DebouncedEvent::NoticeRemove(p) => Some(Event::Remove(p)),
                    DebouncedEvent::Create(p) => Some(Event::Create(p)),
                    DebouncedEvent::Write(p) => Some(Event::Write(p)),
                    DebouncedEvent::Rename(source, dest) => Some(Event::Rename(source, dest)),
                    // TODO: Define what to do with Rescan
                    DebouncedEvent::Rescan => Some(Event::Rescan),
                    DebouncedEvent::Error(e, p) => Some(Event::Error(e.into(), p)),
                    // NoticeWrite can be useful but we don't use it
                    DebouncedEvent::NoticeWrite(_) => None,
                    // Ignore `Remove`: we use `NoticeRemove` that comes before in the flow
                    DebouncedEvent::Remove(_) => None,
                    // Ignore attribute changes
                    DebouncedEvent::Chmod(_) => None,
                } {
                    return Some(((mapped_event, OffsetDateTime::now_utc()), rx));
                }
            }
        })
    }
}

impl From<notify::Error> for Error {
    fn from(e: notify::Error) -> Error {
        match e {
            NotifyError::Generic(s) => Error::Generic(s),
            NotifyError::Io(err) => Error::Io(format!("{}", err)),
            NotifyError::PathNotFound => Error::PathNotFound,
            NotifyError::WatchNotFound => Error::WatchNotFound,
        }
    }
}

impl From<RecursiveMode> for notify::RecursiveMode {
    fn from(e: RecursiveMode) -> Self {
        match e {
            RecursiveMode::Recursive => notify::RecursiveMode::Recursive,
            RecursiveMode::NonRecursive => notify::RecursiveMode::NonRecursive,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use futures::StreamExt;
    use pin_utils::pin_mut;
    use predicates::prelude::*;
    use std::fs::{self, File};
    use std::io::{self, Write};
    use tempfile::tempdir;

    static DELAY: Duration = Duration::from_millis(200);

    macro_rules! is_match {
        ($p: expr, $e: ident, $expected_path: expr) => {
            match $p {
                Event::$e(path) => {
                    assert_eq!(path.file_name(), $expected_path.file_name());
                    assert_eq!(
                        path.parent().unwrap().file_name(),
                        $expected_path.parent().unwrap().file_name()
                    );
                }
                _ => panic!("event didn't match Event::{}", stringify!($e)),
            }
        };
    }

    macro_rules! take {
        ($stream: ident, $result: ident) => {
            tokio::time::sleep(DELAY).await;
            tokio::time::sleep(Duration::from_millis(20)).await;
            loop {
                tokio::select! {
                    item = $stream.next() => {
                        $result.push(item.unwrap().0);
                    }
                    _ = tokio::time::sleep(Duration::from_millis(200)) => {
                        break;
                    }
                }
            }
        };
    }

    macro_rules! append {
        ($file: ident) => {
            for i in 0..20 {
                writeln!($file, "SAMPLE {}", i)?;
            }
        };
    }

    macro_rules! wait_and_append {
        ($file: ident) => {
            tokio::time::sleep(DELAY * 3).await;
            append!($file);
        };
    }

    #[tokio::test]
    async fn test_unwatch_if_exists() {
        let dir = tempdir().unwrap();
        let dir_untracked = tempdir().unwrap();
        let mut w = Watcher::new(DELAY);
        w.watch(dir.path(), RecursiveMode::Recursive).unwrap();
        assert!(matches!(
            w.unwatch_if_exists(dir_untracked.path()),
            Ok(false)
        ));
        assert!(matches!(w.unwatch_if_exists(dir.path()), Ok(true)));
    }

    #[tokio::test]
    async fn test_initial_write_get_debounced_into_create() -> io::Result<()> {
        let dir = tempdir().unwrap().into_path();
        let dir_path = &dir;

        let mut w = Watcher::new(DELAY);
        w.watch(dir_path, RecursiveMode::Recursive).unwrap();

        let file1_path = dir_path.join("file1.log");
        let mut file1 = File::create(&file1_path)?;
        append!(file1);

        let stream = w.receive();
        pin_mut!(stream);

        tokio::time::sleep(Duration::from_millis(500)).await;
        let mut items = Vec::new();
        take!(stream, items);
        // Depending on timers, it will get debounced or not :(
        assert!(!items.is_empty());
        is_match!(&items[0], Create, file1_path);
        Ok(())
    }

    #[tokio::test]
    async fn test_create_write_delete() -> io::Result<()> {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        let mut w = Watcher::new(DELAY);
        w.watch(dir_path, RecursiveMode::Recursive).unwrap();

        let file_path = dir_path.join("file1.log");
        let mut file = File::create(&file_path)?;
        append!(file);

        let stream = w.receive();
        pin_mut!(stream);

        tokio::time::sleep(Duration::from_millis(500)).await;
        let mut items = Vec::new();
        take!(stream, items);
        // Depending on timers, it can get debounced into a single create
        assert!(!items.is_empty());
        is_match!(&items[0], Create, file_path);

        wait_and_append!(file);
        fs::remove_file(&file_path)?;
        take!(stream, items);

        let is_equal = |p: &PathId| p.as_os_str() == file_path.as_os_str();
        let items: Vec<_> = items
            .iter()
            .filter(|e| match e {
                Event::Write(p) => is_equal(p),
                Event::Remove(p) => is_equal(p),
                Event::Create(p) => is_equal(p),
                _ => false,
            })
            .collect();

        is_match!(items.last().unwrap(), Remove, file_path);

        Ok(())
    }

    #[tokio::test]
    async fn test_watch_file_write_after_create() -> io::Result<()> {
        let dir = tempdir().unwrap().into_path();

        let mut w = Watcher::new(DELAY);
        w.watch(&dir, RecursiveMode::Recursive).unwrap();

        let file1_path = &dir.join("file1.log");
        let mut file1 = File::create(&file1_path)?;

        let stream = w.receive();
        pin_mut!(stream);

        let mut items = Vec::new();
        take!(stream, items);

        assert!(!items.is_empty());
        is_match!(&items[0], Create, file1_path);

        wait_and_append!(file1);
        take!(stream, items);

        is_match!(&items[1], Write, file1_path);
        Ok(())
    }

    /// macOS will follow symlink files
    #[tokio::test]
    #[cfg(target_os = "macos")]
    async fn test_watch_symlink_write_after_create_macos() -> io::Result<()> {
        let dir = tempdir()?.into_path();
        let excluded_dir = tempdir()?.into_path();

        let mut w = Watcher::new(DELAY);
        w.watch(&dir, RecursiveMode::Recursive).unwrap();

        let file_path = &excluded_dir.join("file1.log");
        let symlink_path = dir.join("symlink.log");
        let mut file = File::create(&file_path)?;
        std::os::unix::fs::symlink(&file_path, &symlink_path)?;

        let stream = w.receive();
        pin_mut!(stream);

        let mut items = Vec::new();
        take!(stream, items);

        assert!(!items.is_empty());
        is_match!(&items[0], Create, symlink_path);

        wait_and_append!(file);

        tokio::time::sleep(Duration::from_millis(1000)).await;

        let stream = w.receive();
        pin_mut!(stream);

        take!(stream, items);

        wait_and_append!(file);
        take!(stream, items);

        // Changes are yielded directly to the symlink itself
        let predicate_fn = predicate::in_iter(items);
        assert!(predicate_fn.eval(&Event::Create(symlink_path.clone())));
        assert!(predicate_fn.eval(&Event::Write(symlink_path.clone())));

        Ok(())
    }

    /// Linux will NOT follow symlink files
    #[tokio::test]
    #[cfg(target_os = "linux")]
    async fn test_watch_symlink_write_after_create_linux() -> io::Result<()> {
        let dir = tempdir()?.into_path();
        let excluded_dir = tempdir()?.into_path();

        let mut w = Watcher::new(DELAY);
        w.watch(&dir, RecursiveMode::Recursive).unwrap();

        let file_path = &excluded_dir.join("file1.log");
        let symlink_path = dir.join("symlink.log");
        let mut file = File::create(&file_path)?;
        std::os::unix::fs::symlink(&file_path, &symlink_path)?;

        let stream = w.receive();
        pin_mut!(stream);

        let mut items = Vec::new();
        take!(stream, items);

        assert!(!items.is_empty());
        is_match!(&items[0], Create, symlink_path);

        wait_and_append!(file);

        let stream = w.receive();
        pin_mut!(stream);

        take!(stream, items);

        // Append doesn't produce a Write event
        // because the symlink target is not watched on linux
        wait_and_append!(file);
        take!(stream, items);

        assert_eq!(items.len(), 1);
        is_match!(&items[0], Create, symlink_path);

        Ok(())
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn test_watch_symlink_and_target_write_after_create() -> io::Result<()> {
        let dir = tempdir()?.into_path();
        let excluded_dir = tempdir()?.into_path();

        let mut w = Watcher::new(DELAY);
        w.watch(&dir, RecursiveMode::Recursive).unwrap();

        let file_path = &excluded_dir.join("file1.log");
        let symlink_path = dir.join("symlink.log");
        let mut file = File::create(&file_path)?;
        std::os::unix::fs::symlink(&file_path, &symlink_path)?;

        let stream = w.receive();
        pin_mut!(stream);

        let mut items = Vec::new();
        take!(stream, items);

        assert!(!items.is_empty());
        is_match!(&items[0], Create, symlink_path);

        // Add a watch manually
        let link_target = fs::read_link(&symlink_path)?;
        w.watch(&link_target, RecursiveMode::NonRecursive).unwrap();

        wait_and_append!(file);

        let stream = w.receive();
        pin_mut!(stream);

        take!(stream, items);

        wait_and_append!(file);
        take!(stream, items);

        fs::remove_file(&symlink_path)?;
        take!(stream, items);

        let predicate_fn = predicate::in_iter(items);
        // Changes are yielded directly to the target file
        assert!(predicate_fn.eval(&Event::Write(link_target.clone())));
        // The symlink was removed
        assert!(predicate_fn.eval(&Event::Remove(symlink_path.clone())));

        Ok(())
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn test_watch_symlink_and_target_changed() -> io::Result<()> {
        let dir = tempdir()?.into_path();
        let excluded_dir = tempdir()?.into_path();

        let mut w = Watcher::new(DELAY);
        w.watch(&dir, RecursiveMode::Recursive).unwrap();

        let file_path = &excluded_dir.join("file1.log");
        let symlink_path = dir.join("symlink.log");
        let mut file = File::create(&file_path)?;
        std::os::unix::fs::symlink(&file_path, &symlink_path)?;
        let mut items = Vec::new();

        // Add a watch manually
        let link_target = fs::read_link(&symlink_path)?;
        w.watch(&link_target, RecursiveMode::NonRecursive).unwrap();

        wait_and_append!(file);

        let stream = w.receive();
        pin_mut!(stream);
        take!(stream, items);

        tokio::time::sleep(DELAY).await;
        tokio::time::sleep(DELAY).await;

        let file_new_path = &excluded_dir.join("file_new.log");
        File::create(&file_new_path)?;
        fs::remove_file(&symlink_path)?;
        std::os::unix::fs::symlink(&file_new_path, &symlink_path)?;

        let mut items = Vec::new();
        take!(stream, items);

        let predicate_fn = predicate::in_iter(items);
        // The symlink was edited: yielded as a write in the symlink path
        assert!(predicate_fn.eval(&Event::Write(symlink_path.clone())));

        Ok(())
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn test_watch_symlink_directory() -> io::Result<()> {
        let dir = tempdir()?.into_path();
        let excluded_dir = tempdir()?.into_path();

        let file1_path = &excluded_dir.join("file1.log");
        let symlink_path = &dir.join("symlink-dir");
        let mut file1 = File::create(&file1_path)?;
        std::os::unix::fs::symlink(&excluded_dir, &symlink_path)?;

        let mut w = Watcher::new(DELAY);
        w.watch(&dir, RecursiveMode::Recursive).unwrap();

        let stream = w.receive();
        pin_mut!(stream);

        let mut items = Vec::new();
        take!(stream, items);
        let sub_dir_path = &excluded_dir.join("subdir");
        fs::create_dir(sub_dir_path)?;

        wait_and_append!(file1);
        take!(stream, items);

        let file2_path = &excluded_dir.join("file2.log");
        let mut file2 = File::create(&file2_path)?;
        let file_in_subdir_path = &sub_dir_path.join("file_in_subdir.log");
        let mut file_in_subdir = File::create(&file_in_subdir_path)?;
        wait_and_append!(file2);
        wait_and_append!(file_in_subdir);
        take!(stream, items);

        let predicate_fn = predicate::in_iter(items);

        // The file names are yielded as a child of the symlink dir
        assert!(predicate_fn.eval(&Event::Create(
            symlink_path.join(file2_path.file_name().unwrap())
        )));
        assert!(predicate_fn.eval(&Event::Write(
            symlink_path.join(file1_path.file_name().unwrap())
        )));
        assert!(predicate_fn.eval(&Event::Create(
            symlink_path.join(sub_dir_path.file_name().unwrap())
        )));

        let sub_dir_in_symlink = symlink_path.join(sub_dir_path.file_name().unwrap());
        assert!(predicate_fn.eval(&Event::Create(sub_dir_in_symlink.clone())));
        // Expected to be "symlink-dir/subdir/file_in_subdir.log"
        assert!(predicate_fn.eval(&Event::Create(
            sub_dir_in_symlink.join(file_in_subdir_path.file_name().unwrap())
        )));

        Ok(())
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn test_watch_symlink_directory_move_in_move_out() -> io::Result<()> {
        let dir = tempdir()?.into_path();
        let excluded_dir = tempdir()?.into_path();

        let file1_path = &excluded_dir.join("file1.log");
        let symlink_path = &dir.join("symlink-dir");
        let mut file1 = File::create(&file1_path)?;
        let sub_dir_path = &excluded_dir.join("subdir");
        fs::create_dir(sub_dir_path)?;
        let file_in_subdir_path = &sub_dir_path.join("file_in_subdir.log");
        let mut file_in_subdir = File::create(&file_in_subdir_path)?;

        let mut w = Watcher::new(DELAY);
        w.watch(&dir, RecursiveMode::Recursive).unwrap();

        let stream = w.receive();
        pin_mut!(stream);
        let mut items = Vec::new();

        tokio::time::sleep(DELAY * 2).await;
        std::os::unix::fs::symlink(&excluded_dir, &symlink_path)?;

        take!(stream, items);
        let predicate_fn = predicate::in_iter(items);
        // Only the symlink create is yielded on linux
        assert!(predicate_fn.eval(&Event::Create(symlink_path.clone())));

        // Manually add watch to children files
        let symlink_child_file1 = symlink_path.join(file1_path.file_name().unwrap());
        let symlink_child_file2 = symlink_path
            .join(sub_dir_path.file_name().unwrap())
            .join(file_in_subdir_path.file_name().unwrap());
        w.watch(&symlink_child_file1, RecursiveMode::Recursive)
            .unwrap();
        w.watch(&symlink_child_file2, RecursiveMode::Recursive)
            .unwrap();

        let mut items = Vec::new();
        wait_and_append!(file1);
        wait_and_append!(file_in_subdir);
        take!(stream, items);

        let predicate_fn = predicate::in_iter(items);
        // Write events are received as child of symlink
        assert!(predicate_fn.eval(&Event::Write(symlink_child_file1.clone())));
        assert!(predicate_fn.eval(&Event::Write(symlink_child_file2.clone())));

        let mut items = Vec::new();
        fs::remove_file(&symlink_path)?;
        take!(stream, items);
        let predicate_fn = predicate::in_iter(items);
        assert!(predicate_fn.eval(&Event::Remove(symlink_path.clone())));

        w.unwatch_if_exists(&symlink_child_file1).unwrap();
        w.unwatch_if_exists(&symlink_child_file2).unwrap();

        // Discard all previous events (it might include Error(Io))
        let mut items = Vec::new();
        take!(stream, items);

        // Verify no errors are produced even if target file changes
        tokio::time::sleep(DELAY * 2).await;
        let mut items = Vec::new();
        wait_and_append!(file1);
        take!(stream, items);
        assert_eq!(items, Vec::new());
        Ok(())
    }

    #[tokio::test]
    #[cfg(target_os = "macos")]
    async fn test_watch_hardlink_file_macos() -> io::Result<()> {
        let dir = tempdir()?.into_path();
        let excluded_dir = tempdir()?.into_path();

        let file_path = &excluded_dir.join("file1.log");
        let link_path = &dir.join("symlink.log");
        let mut file = File::create(&file_path)?;
        fs::hard_link(&file_path, &link_path)?;

        let mut w = Watcher::new(DELAY);
        w.watch(&dir, RecursiveMode::Recursive).unwrap();

        let stream = w.receive();
        pin_mut!(stream);

        let mut items = Vec::new();
        take!(stream, items);

        wait_and_append!(file);
        take!(stream, items);

        // macOS will follow hardlinks
        assert_eq!(items.len(), 1);
        is_match!(&items[0], Write, link_path);
        Ok(())
    }

    #[tokio::test]
    #[cfg(target_os = "linux")]
    async fn test_watch_hardlink_file_linux() -> io::Result<()> {
        let dir = tempdir()?.into_path();
        let excluded_dir = tempdir()?.into_path();

        let file_path = &excluded_dir.join("file1.log");
        let link_path = &dir.join("symlink.log");
        let mut file = File::create(&file_path)?;
        fs::hard_link(&file_path, &link_path)?;

        let mut w = Watcher::new(DELAY);
        w.watch(&dir, RecursiveMode::Recursive).unwrap();

        let stream = w.receive();
        pin_mut!(stream);
        let mut items = Vec::new();
        take!(stream, items);
        wait_and_append!(file);
        take!(stream, items);

        // linux will NOT follow hardlinks
        assert_eq!(items.len(), 0);
        Ok(())
    }
}

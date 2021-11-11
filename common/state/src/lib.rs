use rocksdb::{ColumnFamilyDescriptor, IteratorMode, Options, WriteBatch, DB};

use derivative::Derivative;
use futures::channel::oneshot;
use futures::future::{Future, FutureExt};
use futures::stream::StreamExt;
use serde::{Deserialize, Serialize};
use slotmap::{DefaultKey, SlotMap};

use log::{error, info, warn};

use std::collections::HashMap;
use std::convert::{AsRef, Into, TryInto};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use thiserror::Error;

mod offsets;
mod span;

pub use offsets::{Offset, OffsetMap};
pub use span::{Span, SpanError, SpanVec};

const OFFSET_NAME: &str = "file_offsets";

#[derive(Debug, Error)]
pub enum StateError {
    #[error("{0}")]
    RocksDb(#[from] rocksdb::Error),
    #[error("{0}")]
    IoError(#[from] std::io::Error),
    #[error("{0:?}")]
    PermissionDenied(PathBuf),
    #[error("{0:?}")]
    InvalidPath(PathBuf),
}

#[derive(Derivative)]
#[derivative(Debug)]
pub struct AgentState {
    #[derivative(Debug = "ignore")]
    db: Arc<DB>,
    #[derivative(Debug = "ignore")]
    offset_cf_opt: Options,
}

// This was the old new fn implementation. It could probably be broken up
// into a few smaller, private functions.
fn _construct_agent_state(path: &Path) -> Result<AgentState, StateError> {
    if path.metadata()?.permissions().readonly() {
        return Err(StateError::PermissionDenied(path.into()));
    }

    let path = path.join("agent_state.db");

    let mut db_opts = Options::default();
    db_opts.create_missing_column_families(true);
    db_opts.create_if_missing(true);

    let offset_cf_opt = Options::default();
    let offset_cf = ColumnFamilyDescriptor::new(OFFSET_NAME, offset_cf_opt.clone());
    let cfs = vec![offset_cf];

    info!("Opening state db at {:?}", path);

    let db = match DB::open_cf_descriptors(&db_opts, &path, cfs) {
        Ok(db) => db,
        // Attempt to repair a badly closed DB
        Err(e) => {
            warn!("error opening state db, attempted to repair: {}", e);
            DB::repair(&db_opts, &path).map_or_else(
                |_| {
                    DB::destroy(&db_opts, &path)?;
                    DB::open_cf_descriptors(
                        &db_opts,
                        &path,
                        vec![ColumnFamilyDescriptor::new(
                            OFFSET_NAME,
                            offset_cf_opt.clone(),
                        )],
                    )
                },
                |_| {
                    DB::open_cf_descriptors(
                        &db_opts,
                        &path,
                        vec![ColumnFamilyDescriptor::new(
                            OFFSET_NAME,
                            offset_cf_opt.clone(),
                        )],
                    )
                },
            )?
        }
    };
    Ok(AgentState {
        db: Arc::new(db),
        offset_cf_opt,
    })
}

impl AgentState {
    pub fn new(path: impl AsRef<Path>) -> Result<Self, StateError> {
        let path = path.as_ref();

        if !path.exists() {
            std::fs::create_dir_all(path).map_err(StateError::IoError)?;
        } else if !path.is_dir() {
            error!("{} is not a directory", path.to_string_lossy());
            return Err(StateError::InvalidPath(path.to_path_buf()));
        }

        _construct_agent_state(path)
    }
}

impl AgentState {
    pub fn get_offset_state(&self) -> FileOffsetState {
        FileOffsetState::new(self.db.clone(), self.offset_cf_opt.clone())
    }
}

#[derive(Clone, Debug, Deserialize, Copy, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct FileId(u64);

impl FileId {
    pub fn ffi(&self) -> u64 {
        self.0
    }
}

impl From<&u64> for FileId {
    fn from(i: &u64) -> FileId {
        FileId(*i)
    }
}

impl From<u64> for FileId {
    fn from(i: u64) -> FileId {
        FileId(i)
    }
}

#[derive(Debug, Error)]
pub enum FileOffsetStateError {
    #[error("{0}")]
    UpdateError(#[from] async_channel::SendError<FileOffsetEvent>),
    #[error("{0}")]
    DbError(String),
    #[error("{0}")]
    RocksDb(#[from] rocksdb::Error),
    #[error("FileOffsetState already running")]
    AlreadyRunning,
    #[error("FileOffsetState shutdown handle already taken")]
    ShutdownHandleTaken,
    #[error("{0}")]
    SpanError(#[from] SpanError),
    #[error("Failed to stage update")]
    StageUpdateFail,
}

#[derive(Debug)]
pub struct FileOffset {
    pub key: FileId,
    pub offset: Span,
}

#[derive(Debug)]
pub struct FileOffsets {
    pub key: FileId,
    pub offsets: SpanVec,
}

impl<'a, T> From<&'a (T, Span)> for FileOffset
where
    FileId: std::convert::From<&'a T>,
{
    fn from(pair: &'a (T, Span)) -> FileOffset {
        let key = (&pair.0).into();
        let offset = pair.1;
        FileOffset { key, offset }
    }
}

pub enum FileOffsetUpdate {
    Update(OffsetMap, oneshot::Sender<DefaultKey>),
    Delete(FileId),
}

pub enum FileOffsetEvent {
    Update(FileOffsetUpdate),
    Clear,
    Flush(Option<DefaultKey>),
}

#[derive(Clone)]
pub struct FileOffsetWriteHandle {
    tx: async_channel::Sender<FileOffsetEvent>,
}

impl FileOffsetWriteHandle {
    pub async fn update<'a>(&self, offsets: OffsetMap) -> Result<DefaultKey, FileOffsetStateError> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(FileOffsetEvent::Update(FileOffsetUpdate::Update(
                offsets, tx,
            )))
            .await?;

        let key = rx
            .await
            .map_err(|_| FileOffsetStateError::StageUpdateFail)?;

        Ok(key)
    }

    pub async fn delete(&self, file_name: impl Into<FileId>) -> Result<(), FileOffsetStateError> {
        Ok(self
            .tx
            .send(FileOffsetEvent::Update(FileOffsetUpdate::Delete(
                file_name.into(),
            )))
            .await?)
    }
}

pub struct FileOffsetFlushHandle {
    tx: async_channel::Sender<FileOffsetEvent>,
}

impl FileOffsetFlushHandle {
    pub async fn flush(&self, key: Option<DefaultKey>) -> Result<(), FileOffsetStateError> {
        Ok(self.tx.send(FileOffsetEvent::Flush(key)).await?)
    }

    pub async fn clear(&self) -> Result<(), FileOffsetStateError> {
        Ok(self.tx.send(FileOffsetEvent::Clear).await?)
    }
}

pub struct FileOffsetShutdownHandle {
    tx: async_channel::Sender<FileOffsetEvent>,
}

impl FileOffsetShutdownHandle {
    pub fn shutdown(&self) -> bool {
        self.tx.close()
    }
}

#[derive(Clone)]
pub struct FileOffsetState {
    db: Arc<DB>,
    cf_opts: Options,
    rx: std::cell::RefCell<Option<async_channel::Receiver<FileOffsetEvent>>>,
    shutdown: std::cell::RefCell<Option<async_channel::Sender<FileOffsetEvent>>>,
    tx: async_channel::Sender<FileOffsetEvent>,
}

type OffsetStreamState = (
    Option<WriteBatch>,
    (
        HashMap<FileId, SpanVec>,
        SlotMap<DefaultKey, OffsetMap>,
        HashMap<FileId, SpanVec>,
        Vec<u8>,
    ),
);

impl FileOffsetState {
    fn new(db: Arc<DB>, cf_opts: Options) -> Self {
        let (tx, rx) = async_channel::unbounded();

        FileOffsetState {
            db,
            cf_opts,
            rx: std::cell::RefCell::new(Some(rx)),
            shutdown: std::cell::RefCell::new(Some(tx.clone())),
            tx,
        }
    }

    pub fn offsets(&self) -> Result<Vec<FileOffsets>, FileOffsetStateError> {
        let cf_handle = self.db.cf_handle(OFFSET_NAME).ok_or_else(|| {
            FileOffsetStateError::DbError("Failed to get ColumnFamily handle".into())
        })?;
        self.db
            .iterator_cf(cf_handle, IteratorMode::Start)
            .map(|(k, v)| {
                let (k_bytes, _) = k.split_at(std::mem::size_of::<u64>());
                let key = FileId(u64::from_be_bytes(k_bytes.try_into().unwrap_or([0; 8])));
                let mut offsets = SpanVec::with_capacity(std::cmp::max(1, (v.len() - 1) / 2));

                if v.len() == std::mem::size_of::<u64>() {
                    let (v_bytes_min, _) = v.split_at(std::mem::size_of::<u64>());
                    offsets.insert(
                        Span::new(
                            0,
                            u64::from_be_bytes(v_bytes_min.try_into().unwrap_or([0; 8])),
                        )
                        .unwrap(),
                    )
                } else {
                    let (_v_bytes_min, mut v_bytes_rem) = v.split_at(std::mem::size_of::<u64>());
                    while v_bytes_rem.len() > std::mem::size_of::<u64>() {
                        let (v_bytes_start, _v_bytes_rem) =
                            v_bytes_rem.split_at(std::mem::size_of::<u64>());
                        let (v_bytes_end, _) = _v_bytes_rem.split_at(std::mem::size_of::<u64>());
                        v_bytes_rem = _v_bytes_rem;
                        offsets.insert(Span::new(
                            u64::from_be_bytes(v_bytes_start.try_into().unwrap_or([0; 8])),
                            u64::from_be_bytes(v_bytes_end.try_into().unwrap_or([0; 8])),
                        )?);
                    }
                }
                Ok(FileOffsets { key, offsets })
            })
            .collect::<Result<Vec<_>, FileOffsetStateError>>()
    }

    pub fn write_handle(&self) -> FileOffsetWriteHandle {
        FileOffsetWriteHandle {
            tx: self.tx.clone(),
        }
    }

    pub fn flush_handle(&self) -> FileOffsetFlushHandle {
        FileOffsetFlushHandle {
            tx: self.tx.clone(),
        }
    }

    pub fn shutdown_handle(&self) -> Result<FileOffsetShutdownHandle, FileOffsetStateError> {
        Ok(FileOffsetShutdownHandle {
            tx: self
                .shutdown
                .borrow_mut()
                .take()
                .ok_or(FileOffsetStateError::ShutdownHandleTaken)?,
        })
    }

    pub fn run(&self) -> Result<impl Future<Output = ()>, FileOffsetStateError> {
        let rx = self
            .rx
            .borrow_mut()
            .take()
            .ok_or(FileOffsetStateError::AlreadyRunning)?;
        let db = self.db.clone();
        Ok(rx
            .fold(
                (
                    Some(WriteBatch::default()),
                    (
                        HashMap::<FileId, SpanVec>::new(),
                        SlotMap::<DefaultKey, OffsetMap>::new(),
                        HashMap::<FileId, SpanVec>::new(),
                        Vec::new(), /* scratch space */
                    ),
                ),
                move |stream_state: OffsetStreamState, event: FileOffsetEvent| {
                    let db = db.clone();
                    async move {
                        match db.cf_handle(OFFSET_NAME).ok_or_else(|| {
                            FileOffsetStateError::DbError(
                                "Failed to get ColumnFamily handle".into(),
                            )
                        }) {
                            Ok(cf_handle) => {
                                match handle_file_offset_event(event, cf_handle, stream_state) {
                                    Ok(EventAction::Write((Some(wb), rest_of_state))) => {
                                        match db.write(wb).map(|_| ()) {
                                            Ok(_) => Ok((None, rest_of_state)),
                                            Err(e) => Err((e.into(), (None, rest_of_state))),
                                        }
                                    }
                                    Ok(EventAction::Write((None, rest_of_state))) => {
                                        warn!("Got a write with an empty write buffer");
                                        Ok((None, rest_of_state))
                                    }
                                    Ok(EventAction::Nop(stream_state)) => Ok(stream_state),
                                    Err((e, state)) => Err((e, state)),
                                }
                            }
                            Err(e) => Err((e, stream_state)),
                        }
                        .or_else(
                            move |(e, stream_state)| -> Result<_, FileOffsetStateError> {
                                error!("{:?}", e);
                                Ok(stream_state)
                            },
                        )
                        // Safe to unwrap as the or_else impl returns a valid (Option<_>, Vec<u8>)
                        .unwrap()
                    }
                },
            )
            .map(|_| ()))
    }
}

enum EventAction {
    Write(OffsetStreamState),
    Nop(OffsetStreamState),
}

fn handle_file_offset_flush(
    key: Option<DefaultKey>,
    cf_handle: &rocksdb::ColumnFamily,
    state: OffsetStreamState,
) -> OffsetStreamState {
    let (wb, (mut state, mut pending, mut span_buf, mut bytes_buf)) = state;
    let mut wb = wb.unwrap_or_default();
    if let Some(key) = key {
        if let Some(batch) = pending.get_mut(key) {
            // Should be clear anyway
            debug_assert!(span_buf.is_empty());
            span_buf.clear();

            for (file_id, offsets) in batch.iter() {
                // Get the working span_v for this file
                let mut span_v = {
                    if let Some(span_v) = span_buf.remove(file_id) {
                        span_v
                    } else {
                        state.remove(file_id).unwrap_or_default()
                    }
                };
                for offset in offsets.iter() {
                    span_v.insert(*offset);
                }
                span_buf.insert(*file_id, span_v);
            }
            for (file_id, span_v) in span_buf.drain() {
                if let Some(first) = span_v.first().map(|o| o.end) {
                    bytes_buf.clear();
                    bytes_buf.extend_from_slice(&u64::to_be_bytes(first));
                    // Minimum completed end
                    for offset in span_v.iter() {
                        // Start
                        bytes_buf.extend_from_slice(&u64::to_be_bytes(offset.start));
                        // End
                        bytes_buf.extend_from_slice(&u64::to_be_bytes(offset.end));
                    }
                    wb.put_cf(cf_handle, u64::to_be_bytes(file_id.0), &bytes_buf);

                    // Put the updated span back into the state
                    state.insert(file_id, span_v);
                }
            }
        };
    };
    (Some(wb), (state, pending, span_buf, bytes_buf))
}

fn handle_file_offset_event(
    event: FileOffsetEvent,
    cf_handle: &rocksdb::ColumnFamily,
    stream_state: OffsetStreamState,
) -> Result<EventAction, (FileOffsetStateError, OffsetStreamState)> {
    let (wb, (mut state, mut pending, span_buf, bytes_buf)) = stream_state;
    match (wb, event) {
        (wb, FileOffsetEvent::Flush(key)) => Ok(EventAction::Write(handle_file_offset_flush(
            key,
            cf_handle,
            (wb, (state, pending, span_buf, bytes_buf)),
        ))),
        (wb, FileOffsetEvent::Update(e)) => match e {
            FileOffsetUpdate::Update(offsets, tx) => {
                let key = pending.insert(offsets);
                match tx.send(key) {
                    Ok(_) => Ok(EventAction::Nop((
                        wb,
                        (state, pending, span_buf, bytes_buf),
                    ))),
                    Err(_) => Err((
                        FileOffsetStateError::StageUpdateFail,
                        (wb, (state, pending, span_buf, bytes_buf)),
                    )),
                }
            }
            FileOffsetUpdate::Delete(key) => {
                let mut wb = wb.unwrap_or_default();
                state.remove(&key);
                wb.delete_cf(cf_handle, u64::to_be_bytes(key.0));
                Ok(EventAction::Nop((
                    Some(wb),
                    (state, pending, span_buf, bytes_buf),
                )))
            }
        },
        (_, FileOffsetEvent::Clear) => {
            pending.clear();
            Ok(EventAction::Nop((
                None,
                (state, pending, span_buf, bytes_buf),
            )))
        }
    }
}

pub trait GetOffset {
    fn get_key(&self) -> Option<u64>;
    fn get_offset(&self) -> Option<(u64, u64)>;
}

#[cfg(test)]
mod test {

    use super::*;
    use std::fs::File;
    use tempfile::tempdir;

    #[test]
    fn it_works() {
        let _ = env_logger::Builder::from_default_env().try_init();
        let data_dir = tempdir().expect("Could not create temp dir").into_path();

        // create a db, write to it, mutate it, delete entries.
        // The times/delays are significant
        fn _test(db_path: &std::path::Path, initial_count: usize) {
            let agent_state = AgentState::new(db_path).unwrap();
            let offset_state = agent_state.get_offset_state();

            let wh = offset_state.write_handle();
            let fh = offset_state.flush_handle();
            let sh = offset_state.shutdown_handle().unwrap();
            assert_eq!(initial_count, offset_state.offsets().unwrap().len());

            let paths = [1, 2, 3, 4];

            tokio_test::block_on(async move {
                let _ = tokio::join!(
                    async {
                        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

                        assert_eq!(4, offset_state.offsets().unwrap().len());
                        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
                        assert_eq!(4, offset_state.offsets().unwrap().len());
                        assert_eq!(
                            13 * 2 + 14 * 2,
                            offset_state
                                .offsets()
                                .unwrap()
                                .iter()
                                .fold(0, |a, fo| a + fo.offsets.last().unwrap().end),
                            "{:?}",
                            offset_state.offsets()
                        );
                        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
                        assert_eq!(
                            2,
                            offset_state.offsets().unwrap().len(),
                            "{:?}",
                            offset_state.offsets()
                        );
                        sh.shutdown();
                    },
                    async move {
                        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

                        let mut updates = OffsetMap::default();
                        for path in paths.iter() {
                            updates.insert(*path, (0, 2)).unwrap();
                        }
                        for path in paths.iter() {
                            updates.insert(*path, (2, 13)).unwrap();
                        }
                        let key1 = wh.update(updates).await.unwrap();

                        let mut updates = OffsetMap::default();
                        for path in paths[..2].iter() {
                            updates.insert(*path, (0, 14)).unwrap();
                        }
                        let key2 = wh.update(updates).await.unwrap();
                        fh.flush(Some(key1)).await.unwrap();
                        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

                        fh.flush(Some(key2)).await.unwrap();
                        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

                        for path in paths[..2].iter() {
                            wh.delete(path).await.unwrap();
                        }
                        fh.flush(None).await.unwrap();
                    },
                    offset_state.run().unwrap()
                );
            });
        }
        _test(&data_dir, 0);
        _test(&data_dir, 2);
    }

    #[test]
    fn load_agent_state_dir_missing() {
        // build a path with multiple levels of missing directories to ensure they're all created
        let missing_state_dir = tempdir()
            .unwrap()
            .into_path()
            .join("a")
            .join("ghostly")
            .join("path");
        assert!(
            !missing_state_dir.exists(),
            "test prereq failed: {:?} reported as already existing",
            missing_state_dir
        );

        let result = AgentState::new(&missing_state_dir);
        assert!(result.is_ok());
        assert!(
            missing_state_dir.exists(),
            "state directory was not created"
        );
    }

    #[test]
    #[cfg(unix)]
    fn new_agent_state_dir_create_error() {
        use std::fs::DirBuilder;
        use std::os::unix::fs::DirBuilderExt;

        let noperm_dir = tempdir().unwrap().into_path().join("denied");
        assert!(
            !noperm_dir.exists(),
            "test prereq failed: {:?} reported as already existing",
            noperm_dir
        );
        DirBuilder::new()
            .mode(0o000) // is only supported with the unix extension
            .create(&noperm_dir)
            .unwrap();

        assert!(
            noperm_dir.exists(),
            "test preqreq failed: failed to create {:?}",
            noperm_dir
        );
        assert!(
            noperm_dir.metadata().unwrap().permissions().readonly(),
            "test prereq failed: {:?} is not read only",
            noperm_dir
        );

        let result = AgentState::new(&noperm_dir).unwrap_err();
        assert!(matches!(result, StateError::PermissionDenied(p) if p == noperm_dir));
    }

    #[test]
    fn new_agent_state_not_dir() {
        let blocking_file = tempdir().unwrap().into_path().join("block");
        File::create(&blocking_file).unwrap();

        let result = AgentState::new(&blocking_file).unwrap_err();
        assert!(matches!(result, StateError::InvalidPath(p) if p == blocking_file));
    }

    #[test]
    fn new_agent_state_dir_exists() {
        let state_dir = tempdir().unwrap().into_path();
        let result = AgentState::new(&state_dir);
        assert!(result.is_ok(), "failed to create valid AgentState struct");
    }
}

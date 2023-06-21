mod offsets;
mod span;

#[cfg(feature = "state")]
mod state;

pub use offsets::{Offset, OffsetMap};
pub use span::{Span, SpanError, SpanVec};

#[cfg(feature = "state")]
pub use state::{AgentState, StateError};

use async_channel::SendError;
use futures::channel::oneshot;
use slotmap::DefaultKey;

#[derive(
    Clone, Debug, serde::Deserialize, Copy, Eq, Hash, Ord, PartialEq, PartialOrd, serde::Serialize,
)]
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

pub trait GetOffset {
    fn get_key(&self) -> Option<u64>;
    fn get_offset(&self) -> Option<(u64, u64)>;
}

#[derive(Debug, thiserror::Error)]
pub enum FileOffsetStateError {
    #[error("{0}")]
    UpdateError(#[from] async_channel::SendError<FileOffsetEvent>),
    #[error("{0}")]
    DbError(String),
    #[cfg(feature = "state")]
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

pub enum FileOffsetUpdate {
    Update(OffsetMap, oneshot::Sender<DefaultKey>),
    Delete(FileId),
}

pub enum FileOffsetEvent {
    Update(FileOffsetUpdate),
    Clear,
    Flush(Option<DefaultKey>),
    GarbageCollect { retained_files: Vec<FileId> },
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

#[derive(Clone)]
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

    pub fn do_gc_blocking(
        &self,
        retained_files: Vec<FileId>,
    ) -> Result<(), SendError<FileOffsetEvent>> {
        self.tx
            .send_blocking(FileOffsetEvent::GarbageCollect { retained_files })
    }
}

#[derive(Clone)]
pub struct FileOffsetShutdownHandle {
    tx: async_channel::Sender<FileOffsetEvent>,
}

impl FileOffsetShutdownHandle {
    pub fn shutdown(&self) -> bool {
        self.tx.close()
    }
}

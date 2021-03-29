use rocksdb::{ColumnFamilyDescriptor, IteratorMode, Options, WriteBatch, DB};

use derivative::Derivative;
use futures::future::{Future, FutureExt};
use futures::stream::StreamExt;

use log::{error, info, warn};

use std::convert::{AsRef, Into, TryInto};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use thiserror::Error;

const OFFSET_NAME: &str = "file_offsets";

#[derive(Debug, Error)]
pub enum StateError {
    #[error("{0}")]
    RocksDb(#[from] rocksdb::Error),
    #[error("{0}")]
    IoError(#[from] std::io::Error),
    #[error("{0:?}")]
    PermissionDenied(PathBuf),
}

#[derive(Derivative)]
#[derivative(Debug)]
pub struct AgentState {
    #[derivative(Debug = "ignore")]
    db: Arc<DB>,
    #[derivative(Debug = "ignore")]
    offset_cf_opt: Options,
}

impl AgentState {
    pub fn new(path: impl AsRef<Path>) -> Result<Self, StateError> {
        let path = path.as_ref();

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
        Ok(Self {
            db: Arc::new(db),
            offset_cf_opt,
        })
    }
}

impl AgentState {
    pub fn get_offset_state(&self) -> FileOffsetState {
        FileOffsetState::new(self.db.clone(), self.offset_cf_opt.clone())
    }
}

#[derive(Debug, Hash, Clone, PartialEq, Eq)]
pub struct FileName(bytes::Bytes);

impl FileName {
    pub fn bytes(&self) -> &[u8] {
        &self.0
    }
}

impl<T> From<T> for FileName
where
    T: AsRef<[u8]>,
{
    fn from(b: T) -> FileName {
        FileName(bytes::Bytes::copy_from_slice(b.as_ref()))
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
}

pub struct FileOffset {
    pub key: FileName,
    pub offset: u64,
}

pub enum FileOffsetUpdate {
    Update(FileOffset),
    Delete(FileName),
}

pub enum FileOffsetEvent {
    Update(FileOffsetUpdate),
    Clear,
    Flush,
}

#[derive(Clone)]
pub struct FileOffsetWriteHandle {
    tx: async_channel::Sender<FileOffsetEvent>,
}

impl FileOffsetWriteHandle {
    pub async fn update(
        &self,
        file_name: impl Into<FileName>,
        offset: u64,
    ) -> Result<(), FileOffsetStateError> {
        Ok(self
            .tx
            .send(FileOffsetEvent::Update(FileOffsetUpdate::Update(
                FileOffset {
                    key: file_name.into(),
                    offset,
                },
            )))
            .await?)
    }

    pub async fn delete(&self, file_name: impl Into<FileName>) -> Result<(), FileOffsetStateError> {
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
    pub async fn flush(&self) -> Result<(), FileOffsetStateError> {
        Ok(self.tx.send(FileOffsetEvent::Flush).await?)
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

    pub fn offsets(&self) -> Result<Vec<FileOffset>, FileOffsetStateError> {
        let cf_handle = self.db.cf_handle(OFFSET_NAME).ok_or_else(|| {
            FileOffsetStateError::DbError("Failed to get ColumnFamily handle".into())
        })?;
        Ok(self
            .db
            .iterator_cf(cf_handle, IteratorMode::Start)
            .map(|(k, v)| {
                let (int_bytes, _) = v.split_at(std::mem::size_of::<u64>());
                FileOffset {
                    key: FileName(bytes::Bytes::copy_from_slice(k.as_ref())),
                    offset: u64::from_be_bytes(int_bytes.try_into().unwrap_or([0; 8])),
                }
            })
            .collect::<Vec<_>>())
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
            .fold(Some(WriteBatch::default()), move |acc, event| {
                let db = db.clone();
                async move {
                    match db.cf_handle(OFFSET_NAME).ok_or_else(|| {
                        FileOffsetStateError::DbError("Failed to get ColumnFamily handle".into())
                    }) {
                        Ok(cf_handle) => match (acc, event) {
                            (Some(wb), FileOffsetEvent::Flush) => {
                                let ret = db.write(wb).map(|_| ());
                                ret.map(|_| None).map_err(|e| e.into())
                            }
                            (None, FileOffsetEvent::Flush) => Ok(None),
                            (wb, FileOffsetEvent::Update(e)) => {
                                let mut wb = wb.unwrap_or_default();
                                match e {
                                    FileOffsetUpdate::Update(FileOffset { key, offset }) => {
                                        wb.put_cf(cf_handle, key.0, u64::to_be_bytes(offset))
                                    }
                                    FileOffsetUpdate::Delete(key) => wb.delete_cf(cf_handle, key.0),
                                };
                                Ok(Some(wb))
                            }
                            (_, FileOffsetEvent::Clear) => Ok(None),
                        },
                        Err(e) => Err(e),
                    }
                    .map_err(|e| error!("{:?}", e))
                    .ok()
                    .flatten()
                }
            })
            .map(|_| ()))
    }
}

pub trait GetOffset {
    fn get_key(&self) -> Option<&[u8]>;
    fn get_offset(&self) -> Option<u64>;
}

#[cfg(test)]
mod test {

    use super::*;
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

            let paths = ["path1", "path2", "path3", "path04"];

            tokio_test::block_on(async {
                let _ = tokio::join!(
                    async {
                        tokio::time::delay_for(tokio::time::Duration::from_millis(200)).await;

                        assert_eq!(4, offset_state.offsets().unwrap().len());
                        tokio::time::delay_for(tokio::time::Duration::from_millis(200)).await;
                        assert_eq!(4, offset_state.offsets().unwrap().len());
                        assert_eq!(
                            13 * 2 + 14 * 2,
                            offset_state
                                .offsets()
                                .unwrap()
                                .iter()
                                .fold(0, |a, fo| a + fo.offset)
                        );
                        tokio::time::delay_for(tokio::time::Duration::from_millis(200)).await;
                        assert_eq!(2, offset_state.offsets().unwrap().len());
                        sh.shutdown();
                    },
                    async move {
                        tokio::time::delay_for(tokio::time::Duration::from_millis(100)).await;
                        for path in paths.iter() {
                            wh.update(path.as_bytes(), 13).await.unwrap();
                        }
                        fh.flush().await.unwrap();
                        tokio::time::delay_for(tokio::time::Duration::from_millis(200)).await;

                        for path in paths[..2].iter() {
                            wh.update(path.as_bytes(), 14).await.unwrap();
                        }
                        fh.flush().await.unwrap();
                        tokio::time::delay_for(tokio::time::Duration::from_millis(200)).await;

                        for path in paths[..2].iter() {
                            wh.delete(path.as_bytes()).await.unwrap();
                        }
                        fh.flush().await.unwrap();
                    },
                    offset_state.run().unwrap()
                );
            });
        }
        _test(&data_dir, 0);
        _test(&data_dir, 2);
    }
}

use rocksdb::{ColumnFamilyDescriptor, IteratorMode, Options, WriteBatch, DB};

use derivative::Derivative;
use futures::future::FutureExt;
use futures::stream::StreamExt;
use std::convert::{AsRef, TryInto};
use std::path::Path;
use std::sync::Arc;
use thiserror::Error;

const OFFSET_NAME: &str = "file_offsets";

#[derive(Debug, Error)]
pub enum StateError {
    #[error("{0}")]
    RocksDb(#[from] rocksdb::Error),
}

/**Parent State object **/
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

        let mut db_opts = Options::default();
        db_opts.create_missing_column_families(true);
        db_opts.create_if_missing(true);

        let offset_cf_opt = Options::default();
        let offset_cf = ColumnFamilyDescriptor::new(OFFSET_NAME, offset_cf_opt.clone());
        let cfs = vec![offset_cf];

        let db = if let Ok(db) = DB::open_cf_descriptors(&db_opts, path, cfs) {
            db
        } else {
            DB::repair(&db_opts, path).map_or_else(
                |_| {
                    DB::destroy(&db_opts, path).expect("Couldn't destroy state file");
                    DB::open_cf_descriptors(
                        &db_opts,
                        path,
                        vec![ColumnFamilyDescriptor::new(
                            OFFSET_NAME,
                            offset_cf_opt.clone(),
                        )],
                    )
                },
                |_| {
                    DB::open_cf_descriptors(
                        &db_opts,
                        path,
                        vec![ColumnFamilyDescriptor::new(
                            OFFSET_NAME,
                            offset_cf_opt.clone(),
                        )],
                    )
                },
            )?
        };
        // Attempt to repair a badly closed DB
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

pub struct FileName(bytes::Bytes);

#[derive(Debug, Error)]
pub enum FileOffsetStateError {
    #[error("{0}")]
    UpdateError(#[from] async_channel::SendError<FileOffsetEvent>),
    #[error("FileOffsetState already running")]
    AlreadyRunning,
}

pub struct FileOffset {
    key: FileName,
    offset: u64,
}

pub enum FileOffsetUpdate {
    Update(FileOffset),
    Delete(FileName),
}

pub enum FileOffsetEvent {
    Update(FileOffsetUpdate),
    Flush,
}

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
                    offset: offset,
                },
            )))
            .await?)
    }

    pub async fn delete(&self, file_name: FileName) -> Result<(), FileOffsetStateError> {
        Ok(self
            .tx
            .send(FileOffsetEvent::Update(FileOffsetUpdate::Delete(file_name)))
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
}

#[derive(Clone)]
pub struct FileOffsetState {
    db: Arc<DB>,
    cf_opts: Options,
    rx: Option<async_channel::Receiver<FileOffsetEvent>>,
    tx: async_channel::Sender<FileOffsetEvent>,
}

impl FileOffsetState {
    fn new(db: Arc<DB>, cf_opts: Options) -> Self {
        let (tx, rx) = async_channel::unbounded();

        FileOffsetState {
            db,
            cf_opts,
            rx: Some(rx),
            tx,
        }
    }
    pub fn iter(&self) -> impl Iterator<Item = FileOffset> + '_ {
        let cf_handle = self.db.cf_handle(OFFSET_NAME).unwrap();
        self.db
            .iterator_cf(cf_handle, IteratorMode::Start)
            .map(|(k, v)| {
                let (int_bytes, _) = v.split_at(std::mem::size_of::<u64>());
                FileOffset {
                    key: FileName(bytes::Bytes::copy_from_slice(k.as_ref())),
                    offset: u64::from_be_bytes(int_bytes.try_into().unwrap_or([0; 8])),
                }
            })
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

    pub fn run(&mut self) -> Result<impl std::future::Future<Output = ()>, FileOffsetStateError> {
        let rx = self.rx.take().ok_or(FileOffsetStateError::AlreadyRunning)?;
        let self_ref = self.db.clone();
        Ok(rx
            .fold(Some(WriteBatch::default()), {
                let self_ref = self_ref.clone();
                move |a, e| {
                    let self_ref = self_ref.clone();
                    async move {
                        let cf_handle = self_ref.cf_handle(OFFSET_NAME).unwrap();
                        match (a, e) {
                            (Some(wb), FileOffsetEvent::Flush) => {
                                self_ref.write(wb).expect("Couldn't flush state"); // TODO
                                None
                            }
                            (None, FileOffsetEvent::Flush) => None,
                            (wb, FileOffsetEvent::Update(e)) => {
                                let mut wb = wb.unwrap_or(WriteBatch::default());
                                match e {
                                    FileOffsetUpdate::Update(FileOffset { key, offset }) => {
                                        wb.put_cf(cf_handle, key.0, u64::to_be_bytes(offset));
                                    }
                                    FileOffsetUpdate::Delete(key) => {
                                        wb.delete_cf(cf_handle, key.0);
                                    }
                                }
                                Some(wb)
                            }
                        }
                    }
                }
            })
            .map(|_| ()))
    }
}

// TODO:
// Give tailer a writer
// Give Retry a writer
// Wrap all the Line/LazyLines to carry their offset
// impl trait to allow things carrying their offset to write them
// Give http client a flusher
// Teach it to flush
// Update fs to populate offsets from offsetstate

// test

// cry
#[test]
fn it_works() {
    println!("Hello, world!");
}

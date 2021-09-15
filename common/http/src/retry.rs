use std::convert::TryInto;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use async_compat::CompatExt;

use chrono::prelude::Utc;
use crossbeam::queue::SegQueue;

use futures::stream::{self, Stream};
use futures_timer::Delay;

use metrics::Metrics;

use serde::Deserialize;
use thiserror::Error;

use tokio::fs::{read_dir, remove_file, rename, File, OpenOptions};
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader, BufWriter};

use uuid::Uuid;

use crate::offsets::Offset;
use crate::types::body::{IngestBody, IngestBodyBuffer, IntoIngestBodyBuffer};

#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Serde(#[from] serde_json::Error),
    #[error(transparent)]
    Recv(#[from] crossbeam::channel::RecvError),
    #[error(transparent)]
    Send(#[from] crossbeam::channel::SendError<Box<IngestBodyBuffer>>),
    #[error("{0:?} is not valid utf8")]
    NonUtf8(std::path::PathBuf),
    #[error("{0} is not a valid file name")]
    InvalidFileName(std::string::String),
}

#[derive(Default)]
pub struct Retry {
    directory: PathBuf,
    waiting: SegQueue<PathBuf>,
    retry_base_delay_secs: i64,
}

#[derive(Deserialize)]
struct DiskRead {
    offsets: Option<Vec<Offset>>,
    body: IngestBody,
}

impl Retry {
    pub fn new(directory: PathBuf, retry_base_delay: Duration) -> Retry {
        std::fs::create_dir_all(&directory)
            .unwrap_or_else(|_| panic!("can't create {:#?}", &directory));
        Retry {
            directory,
            waiting: SegQueue::new(),
            retry_base_delay_secs: retry_base_delay.as_secs() as i64,
        }
    }

    async fn fill_waiting(&self) -> Result<(), Error> {
        let mut files = read_dir(&self.directory).await?;
        while let Some(file) = files.next_entry().await? {
            let path = file.path();
            if path.is_dir() {
                continue;
            }

            if path.extension() == Some(std::ffi::OsStr::new("retry")) {
                let file_name = path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .map(|s| s.to_string())
                    .ok_or_else(|| Error::NonUtf8(path.clone()))?;

                let timestamp: i64 = file_name
                    .split('_')
                    .map(|s| s.to_string())
                    .collect::<Vec<String>>()
                    .get(0)
                    .and_then(|s| FromStr::from_str(s).ok())
                    .ok_or_else(|| Error::InvalidFileName(file_name.clone()))?;

                if Utc::now().timestamp() - timestamp < self.retry_base_delay_secs {
                    continue;
                }
                self.waiting.push(path);
            }
        }

        Ok(())
    }

    async fn read_from_disk(path: &Path) -> Result<(Option<Vec<Offset>>, IngestBody), Error> {
        let mut file = BufReader::new(File::open(path).await?);
        let mut data = String::new();
        file.read_to_string(&mut data).await?;
        remove_file(&path).await?;
        let DiskRead { offsets, body } = serde_json::from_str(&data)?;
        Ok((offsets, body))
    }

    pub fn into_stream(
        self,
    ) -> impl Stream<Item = Result<(IngestBodyBuffer, Option<Vec<Offset>>), Error>> {
        stream::unfold(self, |state| async move {
            loop {
                // Try to populate retry queue
                if state.waiting.is_empty() {
                    if let Err(e) = state.fill_waiting().await {
                        return Some((Err(e), state));
                    };
                    // If there are still no objects then sleep for a while
                    if state.waiting.is_empty() {
                        Delay::new(Duration::from_secs(
                            state.retry_base_delay_secs.try_into().unwrap_or(2),
                        ))
                        .await;
                    }
                }

                if let Some(path) = state.waiting.pop() {
                    match Retry::read_from_disk(&path).await {
                        Ok((offsets, ingest_body)) => {
                            match IntoIngestBodyBuffer::into(ingest_body).await {
                                Ok(body_buffer) => {
                                    return Some((Ok((body_buffer, offsets)), state))
                                }
                                Err(e) => return Some((Err(e.into()), state)),
                            }
                        }
                        Err(e) => return Some((Err(e), state)),
                    }
                }
            }
        })
    }
}

pub struct RetrySender {
    directory: PathBuf,
}

impl RetrySender {
    pub fn new(directory: PathBuf) -> Self {
        Self { directory }
    }

    pub async fn retry(
        &self,
        offsets: Option<&[Offset]>,
        body: &IngestBodyBuffer,
    ) -> Result<(), Error> {
        Metrics::http().increment_retries();

        let fn_ts = Utc::now().timestamp();
        let fn_uuid = Uuid::new_v4().to_string();

        // Write to a partial file to avoid concurrently reading from a file that's not been written
        let mut file_name = self.directory.clone();
        file_name.push(format!("{}_{}.retry.partial", fn_ts, fn_uuid));

        let mut file = BufWriter::new(
            OpenOptions::new()
                .create(true)
                .write(true)
                .open(&file_name)
                .await?,
        );

        // Manually serialize the body and offsets
        file.write_all(b"{").await?;
        let mut file = if let Some(offsets) = offsets {
            file.write_all(b"\"offsets\":").await?;

            // Serde can't write to async_write, so offload this to a threadpool
            let std_file = tokio::task::spawn_blocking(
                {
                    // Get the std::fs::File out of the tokio BufWriter
                    file.flush().await?;
                    let mut std_file = std::io::BufWriter::new(file.into_inner().into_std().await);
                    let offsets = offsets.to_vec();
                    move || -> Result<std::fs::File, std::io::Error> {
                        // Serialise the offsets to the std::io::BufWriter
                        serde_json::to_writer(&mut std_file, &offsets)?;
                        std_file.flush()?;
                        Ok(std_file.into_inner()?)
                    }
                }
            ).await.unwrap(/*FIXME handle properly*/)?;
            let mut file = BufWriter::new(File::from_std(std_file));
            file.write_all(b",").await?;
            file
        } else {
            file
        };
        file.write_all(b"\"body\":").await?;
        let mut reader = body.reader();
        let _bytes_written = futures::io::copy(&mut reader, &mut file.compat_mut()).await?;
        file.write_all(b"}").await?;
        file.flush().await?;

        Ok(rename(
            file_name,
            format!("/tmp/logdna/{}_{}.retry", fn_ts, fn_uuid),
        )
        .await?)
    }
}

pub fn retry(dir: PathBuf, retry_base_delay: Duration) -> (RetrySender, Retry) {
    (
        RetrySender::new(dir.clone()),
        Retry::new(dir, retry_base_delay),
    )
}

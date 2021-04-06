use std::fs::{create_dir_all, read_dir, remove_file, File, OpenOptions};
use std::io::{Read, Write};
use std::str::FromStr;

use chrono::prelude::Utc;
use crossbeam::queue::SegQueue;
use uuid::Uuid;

use crate::types::body::{IngestBody, IngestBodyBuffer, IntoIngestBodyBuffer};
use crate::Offset;
use metrics::Metrics;
use serde::Deserialize;
use std::path::{Path, PathBuf};

use thiserror::Error;

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
    waiting: SegQueue<PathBuf>,
}

#[derive(Deserialize)]
struct DiskRead {
    offsets: Option<Vec<Offset>>,
    body: IngestBody,
}

impl Retry {
    pub fn new() -> Retry {
        create_dir_all("/tmp/logdna/").expect("can't create /tmp/logdna");
        Retry {
            waiting: SegQueue::new(),
        }
    }

    pub fn retry(
        &self,
        offsets: Option<&Vec<Offset>>,
        body: &IngestBodyBuffer,
    ) -> Result<(), Error> {
        Metrics::http().increment_retries();
        let mut file = OpenOptions::new().create(true).write(true).open(format!(
            "/tmp/logdna/{}_{}.retry",
            Utc::now().timestamp(),
            Uuid::new_v4().to_string()
        ))?;
        file.write_all(b"{")?;
        if let Some(offsets) = offsets {
            file.write_all(b"\"offsets\":")?;
            serde_json::to_writer(&mut file, &offsets)?;
            file.write_all(b",")?;
        };
        file.write_all(b"\"body\":")?;
        let mut reader = body.reader();
        let _bytes_written = std::io::copy(&mut reader, &mut file)?;
        file.write_all(b"}")?;
        Ok(())
    }

    pub async fn poll(&self) -> Result<(Option<Vec<Offset>>, Option<IngestBodyBuffer>), Error> {
        if self.waiting.is_empty() {
            self.fill_waiting()?
        }

        if let Some(path) = self.waiting.pop() {
            let (offsets, ingest_body) = Self::read_from_disk(&path)?;
            return Ok((
                offsets,
                Some(IntoIngestBodyBuffer::into(ingest_body).await?),
            ));
        }

        Ok((None, None))
    }

    fn fill_waiting(&self) -> Result<(), Error> {
        let files = read_dir("/tmp/logdna/")?;
        for file in files {
            let path = file?.path();
            if path.is_dir() {
                continue;
            }

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

            if Utc::now().timestamp() - timestamp < 15 {
                continue;
            }

            self.waiting.push(path);
        }

        Ok(())
    }

    fn read_from_disk(path: &Path) -> Result<(Option<Vec<Offset>>, IngestBody), Error> {
        let mut file = File::open(path)?;
        let mut data = String::new();
        file.read_to_string(&mut data)?;
        remove_file(&path)?;
        let DiskRead { offsets, body } = serde_json::from_str(&data)?;
        Ok((offsets, body))
    }
}

use std::convert::TryInto;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use async_compat::CompatExt;

use crossbeam::queue::SegQueue;
use time::OffsetDateTime;

use futures::stream::{self, Stream};
use futures_timer::Delay;

use metrics::Metrics;

use serde::Deserialize;
use thiserror::Error;

use tokio::fs::{metadata, read_dir, remove_file, rename, File, OpenOptions};
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader, BufWriter};

use uuid::Uuid;

use crate::types::body::{IngestBody, IngestBodyBuffer, IntoIngestBodyBuffer};
use state::OffsetMap;

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
    retry_step_delay: Duration,
}

#[derive(Deserialize)]
struct DiskRead {
    offsets: Option<OffsetMap>,
    body: IngestBody,
}

pub struct RetryItem {
    pub body_buffer: IngestBodyBuffer,
    pub offsets: Option<OffsetMap>,
    pub path: PathBuf,
}

impl RetryItem {
    fn new(body_buffer: IngestBodyBuffer, offsets: Option<OffsetMap>, path: PathBuf) -> Self {
        Self {
            body_buffer,
            offsets,
            path,
        }
    }
}

impl Retry {
    pub fn new(
        directory: PathBuf,
        retry_base_delay: Duration,
        retry_step_delay: Duration,
    ) -> Retry {
        std::fs::create_dir_all(&directory)
            .unwrap_or_else(|_| panic!("can't create {:#?}", &directory));
        Retry {
            directory,
            waiting: SegQueue::new(),
            retry_base_delay_secs: retry_base_delay.as_secs() as i64,
            retry_step_delay,
        }
    }

    async fn fill_waiting(&self) -> Result<(), Error> {
        let mut files = read_dir(&self.directory).await?;
        let mut retry_disk_used = 0u64;
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

                let file_size = match metadata(path.clone()).await {
                    Ok(md) => md.len(),
                    Err(e) => {
                        warn!(
                            "retry file size unknown, metrics may be skewed; reason={}",
                            e
                        );
                        0
                    }
                };

                retry_disk_used += file_size;

                if OffsetDateTime::now_utc().unix_timestamp() - timestamp
                    < self.retry_base_delay_secs
                {
                    continue;
                }
                Metrics::retry().inc_pending();
                self.waiting.push(path);
            }
        }

        Metrics::retry().report_storage_used(retry_disk_used);
        Ok(())
    }

    async fn read_from_disk(path: &Path) -> Result<(Option<OffsetMap>, IngestBody), Error> {
        let mut file = BufReader::new(File::open(path).await?);
        let mut data = String::new();
        file.read_to_string(&mut data).await?;
        remove_file(&path).await?;
        let DiskRead { offsets, body } = serde_json::from_str(&data)?;
        Ok((offsets, body))
    }

    pub fn into_stream(self) -> impl Stream<Item = Result<RetryItem, Error>> {
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
                        continue;
                    }
                }

                if let Some(path) = state.waiting.pop() {
                    Metrics::retry().dec_pending();
                    // Step delay
                    Delay::new(state.retry_step_delay).await;
                    match Retry::read_from_disk(&path).await {
                        Ok((offsets, ingest_body)) => {
                            match IntoIngestBodyBuffer::into(ingest_body).await {
                                Ok(body_buffer) => {
                                    return Some((
                                        Ok(RetryItem::new(body_buffer, offsets, path)),
                                        state,
                                    ))
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
        offsets: Option<OffsetMap>,
        body: &IngestBodyBuffer,
    ) -> Result<(), Error> {
        Metrics::http().increment_retries();

        let fn_ts = OffsetDateTime::now_utc().unix_timestamp();
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

        let mut new_file_name = self.directory.clone();
        new_file_name.push(format!("{}_{}.retry", fn_ts, fn_uuid));

        Ok(rename(file_name, new_file_name).await?)
    }
}

pub fn retry(
    dir: PathBuf,
    retry_base_delay: Duration,
    retry_step_delay: Duration,
) -> (RetrySender, Retry) {
    (
        RetrySender::new(dir.clone()),
        Retry::new(dir, retry_base_delay, retry_step_delay),
    )
}

#[cfg(test)]
mod tests {

    use super::*;

    use std::collections::{HashMap, HashSet};
    use std::io::Read;
    use std::time::Duration;

    use futures::stream::{self, StreamExt};

    use proptest::prelude::*;

    use tempfile::tempdir;

    use crate::batch::TimedRequestBatcherStreamExt;
    use crate::types::body::Line;

    use test_types::strategies::{line_st, offset_st};

    proptest! {
        #![proptest_config(ProptestConfig {
          cases: 10, .. ProptestConfig::default()
        })]


        #[test]
        fn roundtrip(
            inp in (0..1024usize)
                .prop_flat_map(|size|(Just(size),
                                      proptest::collection::vec(line_st(offset_st(1024)), size)
                ))) {

            let dir = tempdir().expect("Couldn't create temp dir...");
            let dir_path = format!("{}/", dir.path().to_str().unwrap());

            let (size, lines) = inp;
            let (retrier, retry_stream) = retry(dir_path.clone().into(), Duration::from_millis(1000), Duration::from_millis(0));
            let (results, retry_results): (_, Vec<_>) =
                tokio_test::block_on({
                    let dir_path = dir_path.clone();

                    let batch_stream = stream::iter(lines.iter()).timed_request_batches(5_000, Duration::new(1, 0));
                    async move {
                        let results = batch_stream.collect::<Vec<_>>().await;

                        // Check there are no retry files
                        assert_eq!(std::fs::read_dir(&dir_path).unwrap().count(), 0);
                        // Retry all the results and assert they come off the stream
                        for (idx, body_offsets) in results.iter().enumerate() {
                            let (body, offsets) = body_offsets.as_ref().unwrap();
                            retrier.retry(Some(offsets.clone()), body).await.unwrap();
                            // Check there are the right number of retry files
                            assert_eq!(std::fs::read_dir(&dir_path).unwrap().count(), idx + 1);
                        }

                        let results_len = results.len();
                        let retry_results = retry_stream.into_stream()
                            .take(results_len)
                            .enumerate()
                            .map({
                                let dir_path = dir_path.clone();
                                move |(idx, res)| {
                                    assert_eq!(std::fs::read_dir(&dir_path).unwrap().count(), results_len - (idx + 1));
                                    res
                                }})
                            .collect::<Vec<_>>().await;
                        (results, retry_results)
                }});

            assert_eq!(std::fs::read_dir(&dir_path).unwrap().count(), 0);

            assert_eq!(results.len(), retry_results.len());

            let lines = lines.into_iter().map(|offsetline| {
                offsetline.line
            })
                .collect::<Vec<_>>();

            // Grab results and check we got them all
            let stream_results = results.into_iter().map(move |body_offsets|{
                let mut buf = String::new();
                let (body, _offsets) = body_offsets.unwrap();
                body.reader()
                .read_to_string(&mut buf)
                .unwrap();
                let mut body: HashMap<String, Vec<Line>> = serde_json::from_str(&buf).unwrap();
                body.remove("lines").unwrap_or_default()
            })
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();

            let lines_set: HashSet<String> = lines.iter().map(|l|l.line.clone()).collect::<HashSet<_>>();
            let l: HashSet<String> = stream_results.iter().map(|r|r.line.clone()).collect::<HashSet<_>>();

            assert_eq!(stream_results.len(), size);
            assert_eq!(lines_set, l);

            // Grab retries and check we got them all
            let retry_results = retry_results.into_iter().map(move |body_offsets|{
                let mut buf = String::new();
                let body = body_offsets.unwrap();
                body.body_buffer.reader()
                .read_to_string(&mut buf)
                .unwrap();
                let mut body: HashMap<String, Vec<Line>> = serde_json::from_str(&buf).unwrap();
                body.remove("lines").unwrap_or_default()
            })
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();
            let r: HashSet<String> = retry_results.iter().map(|r|r.line.clone()).collect::<HashSet<_>>();

            assert_eq!(retry_results.len(), size);
            assert_eq!(lines_set, r);
        }
    }
}

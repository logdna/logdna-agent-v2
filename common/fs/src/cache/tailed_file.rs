use crate::cache::{get_inode, RetryStreamMessage, WatchEvent};
use std::collections::HashMap;
use std::convert::{TryFrom, TryInto};
use std::ops::DerefMut;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;

use http::types::body::{KeyValueMap, LineBufferMut, LineBuilder, LineMeta, LineMetaMut};
use http::types::error::LineMetaError;
use http::types::serialize::{
    IngestLineSerialize, IngestLineSerializeError, SerializeI64, SerializeMap, SerializeStr,
    SerializeUtf8, SerializeValue,
};

use state::{FileId, GetOffset, SpanVec};

use metrics::Metrics;

use types::sources::{RetryableLine, SourceError};

use async_channel::Sender;
use async_trait::async_trait;

use bytes::Bytes;

use futures::io::AsyncBufReadExt;
use futures::lock::Mutex;
use futures::{stream, Stream, StreamExt};

use serde_json::Value;

use time::OffsetDateTime;
use tokio::io::{AsyncSeekExt, BufReader, SeekFrom};
use tokio_util::compat::{Compat, TokioAsyncReadCompatExt};
use tracing::{debug, error, info, warn};

#[derive(Debug)]
pub struct LazyLineSerializer {
    annotations: Option<KeyValueMap>,
    app: Option<String>,
    env: Option<String>,
    host: Option<String>,
    labels: Option<KeyValueMap>,
    level: Option<String>,
    meta: Option<Value>,
    path: Option<String>,
    line_buffer: Option<Bytes>,

    file_offset: (u64, u64, u64),

    reader: Arc<Mutex<TailedFileInner>>,
    retry_events_send: Option<async_channel::Sender<RetryStreamMessage>>,
}

#[async_trait]
impl IngestLineSerialize<String, bytes::Bytes, std::collections::HashMap<String, String>>
    for LazyLineSerializer
{
    type Ok = ();

    fn has_annotations(&self) -> bool {
        self.annotations.is_some()
    }
    async fn annotations<'b, S>(
        &mut self,
        ser: &mut S,
    ) -> Result<Self::Ok, IngestLineSerializeError>
    where
        S: SerializeMap<'b, HashMap<String, String>> + std::marker::Send,
    {
        if let Some(ref annotations) = self.annotations {
            ser.serialize_map(annotations).await?;
        }
        Ok(())
    }
    fn has_app(&self) -> bool {
        self.app.is_some()
    }
    async fn app<S>(&mut self, writer: &mut S) -> Result<Self::Ok, IngestLineSerializeError>
    where
        S: SerializeStr<String> + std::marker::Send,
    {
        if let Some(app) = self.app.as_ref() {
            writer.serialize_str(app).await?;
        };
        Ok(())
    }
    fn has_env(&self) -> bool {
        self.env.is_some()
    }
    async fn env<S>(&mut self, writer: &mut S) -> Result<Self::Ok, IngestLineSerializeError>
    where
        S: SerializeStr<String> + std::marker::Send,
    {
        if let Some(env) = self.env.as_ref() {
            writer.serialize_str(env).await?;
        };
        Ok(())
    }
    fn has_file(&self) -> bool {
        true
    }
    async fn file<S>(&mut self, writer: &mut S) -> Result<Self::Ok, IngestLineSerializeError>
    where
        S: SerializeStr<String> + std::marker::Send,
    {
        if let Some(path) = self.path.as_ref() {
            writer.serialize_str(path).await?;
        };
        Ok(())
    }
    fn has_host(&self) -> bool {
        self.host.is_some()
    }
    async fn host<S>(&mut self, writer: &mut S) -> Result<Self::Ok, IngestLineSerializeError>
    where
        S: SerializeStr<String> + std::marker::Send,
    {
        if let Some(host) = self.host.as_ref() {
            writer.serialize_str(host).await?;
        };
        Ok(())
    }
    fn has_labels(&self) -> bool {
        self.labels.is_some()
    }
    async fn labels<'b, S>(&mut self, ser: &mut S) -> Result<Self::Ok, IngestLineSerializeError>
    where
        S: SerializeMap<'b, HashMap<String, String>> + std::marker::Send,
    {
        if let Some(ref labels) = self.labels {
            ser.serialize_map(labels).await?;
        }
        Ok(())
    }
    fn has_level(&self) -> bool {
        self.level.is_some()
    }
    async fn level<S>(&mut self, writer: &mut S) -> Result<Self::Ok, IngestLineSerializeError>
    where
        S: SerializeStr<String> + std::marker::Send,
    {
        if let Some(level) = self.level.as_ref() {
            writer.serialize_str(level).await?;
        };
        Ok(())
    }
    fn has_meta(&self) -> bool {
        self.meta.is_some()
    }
    async fn meta<S>(&mut self, writer: &mut S) -> Result<Self::Ok, IngestLineSerializeError>
    where
        S: SerializeValue + std::marker::Send,
    {
        if let Some(meta) = self.meta.as_ref() {
            writer.serialize(meta).await?;
        };
        Ok(())
    }
    async fn line<S>(&mut self, writer: &mut S) -> Result<Self::Ok, IngestLineSerializeError>
    where
        S: SerializeUtf8<bytes::Bytes> + std::marker::Send,
    {
        // Try to use the cached value first
        let bytes = if let Some(buf) = &self.line_buffer {
            buf.clone()
        } else {
            let borrowed_reader = self.reader.lock().await;
            line_bytes(&borrowed_reader.buf)
        };
        writer.serialize_utf8(bytes).await?;

        Ok(())
    }
    async fn timestamp<S>(&mut self, writer: &mut S) -> Result<Self::Ok, IngestLineSerializeError>
    where
        S: SerializeI64 + std::marker::Send,
    {
        writer
            .serialize_i64(&OffsetDateTime::now_utc().unix_timestamp())
            .await?;

        Ok(())
    }
    fn field_count(&self) -> usize {
        3 + usize::from(!Option::is_none(&self.annotations))
            + usize::from(!Option::is_none(&self.app))
            + usize::from(!Option::is_none(&self.env))
            + usize::from(!Option::is_none(&self.host))
            + usize::from(!Option::is_none(&self.labels))
            + usize::from(!Option::is_none(&self.level))
            + usize::from(!Option::is_none(&self.meta))
    }
}

impl LazyLineSerializer {
    pub fn new(
        reader: Arc<Mutex<TailedFileInner>>,
        path: String,
        offset: (u64, u64, u64),
        retry_events_send: Option<async_channel::Sender<RetryStreamMessage>>,
    ) -> Self {
        Self {
            reader,
            path: Some(path),
            annotations: None,
            app: None,
            env: None,
            host: None,
            labels: None,
            level: None,
            meta: None,
            line_buffer: None,
            file_offset: offset,
            retry_events_send,
        }
    }
}

impl LineMeta for LazyLineSerializer {
    fn get_annotations(&self) -> Option<&KeyValueMap> {
        self.annotations.as_ref()
    }
    fn get_app(&self) -> Option<&str> {
        self.app.as_deref()
    }
    fn get_env(&self) -> Option<&str> {
        self.env.as_deref()
    }
    fn get_file(&self) -> Option<&str> {
        self.path.as_deref()
    }
    fn get_host(&self) -> Option<&str> {
        self.host.as_deref()
    }
    fn get_labels(&self) -> Option<&KeyValueMap> {
        self.labels.as_ref()
    }
    fn get_level(&self) -> Option<&str> {
        self.level.as_deref()
    }
    fn get_meta(&self) -> Option<&Value> {
        self.meta.as_ref()
    }
}

impl LineMetaMut for LazyLineSerializer {
    fn get_annotations_mut(&mut self) -> &mut Option<KeyValueMap> {
        &mut self.annotations
    }
    fn get_app_mut(&mut self) -> &mut Option<String> {
        &mut self.app
    }
    fn get_env_mut(&mut self) -> &mut Option<String> {
        &mut self.env
    }
    fn get_file_mut(&mut self) -> &mut Option<String> {
        &mut self.path
    }
    fn get_host_mut(&mut self) -> &mut Option<String> {
        &mut self.host
    }
    fn get_labels_mut(&mut self) -> &mut Option<KeyValueMap> {
        &mut self.labels
    }
    fn get_level_mut(&mut self) -> &mut Option<String> {
        &mut self.level
    }
    fn get_meta_mut(&mut self) -> &mut Option<Value> {
        &mut self.meta
    }
    fn set_annotations(&mut self, annotations: KeyValueMap) -> Result<(), LineMetaError> {
        self.annotations = Some(annotations);
        Ok(())
    }
    fn set_app(&mut self, app: String) -> Result<(), LineMetaError> {
        self.app = Some(app);
        Ok(())
    }
    fn set_env(&mut self, env: String) -> Result<(), LineMetaError> {
        self.env = Some(env);
        Ok(())
    }
    fn set_file(&mut self, file: String) -> Result<(), LineMetaError> {
        self.path = Some(file);
        Ok(())
    }
    fn set_host(&mut self, host: String) -> Result<(), LineMetaError> {
        self.host = Some(host);
        Ok(())
    }
    fn set_labels(&mut self, labels: KeyValueMap) -> Result<(), LineMetaError> {
        self.labels = Some(labels);
        Ok(())
    }
    fn set_level(&mut self, level: String) -> Result<(), LineMetaError> {
        self.level = Some(level);
        Ok(())
    }
    fn set_meta(&mut self, meta: Value) -> Result<(), LineMetaError> {
        self.meta = Some(meta);
        Ok(())
    }
}

impl LineBufferMut for LazyLineSerializer {
    fn get_line_buffer(&mut self) -> Option<&[u8]> {
        if self.line_buffer.as_ref().is_some() {
            // Get the value without locking
            return self.line_buffer.as_deref();
        }

        match self.reader.try_lock() {
            Some(file_inner) => {
                // Cache the value to avoid further cloning
                self.line_buffer = Some(line_bytes(&file_inner.buf));
                self.line_buffer.as_deref()
            }
            None => None,
        }
    }

    fn set_line_buffer(&mut self, line: Vec<u8>) -> Result<(), LineMetaError> {
        self.line_buffer = Some(line.into());
        Ok(())
    }
}

impl GetOffset for LazyLineSerializer {
    fn get_offset(&self) -> Option<(u64, u64)> {
        Some((self.file_offset.1, self.file_offset.2))
    }
    fn get_key(&self) -> Option<u64> {
        Some(self.file_offset.0)
    }
}

#[derive(Debug)]
pub struct TailedFileInner {
    reader: Compat<tokio::io::BufReader<tokio::fs::File>>,
    buf: Vec<u8>,
    initial_offsets: SpanVec,
    offset: u64,
    inode: u64,
    path: PathBuf,
    retries: u32,
    retrying_until: Option<OffsetDateTime>,
}

impl TailedFileInner {
    pub fn get_inode(&self) -> FileId {
        self.inode.into()
    }
}

#[derive(Debug, Clone)]
pub struct TailedFile<T> {
    inner: Arc<Mutex<TailedFileInner>>,
    resume_events_sender: Option<Sender<(u64, OffsetDateTime)>>,
    retry_events_sender: Option<Sender<RetryStreamMessage>>,
    _phantom: std::marker::PhantomData<T>,
}

impl<T> TailedFile<T> {
    pub(crate) fn new(
        path: &Path,
        initial_offsets: SpanVec,
        resume_events_sender: Option<Sender<(u64, OffsetDateTime)>>,
        retry_events_sender: Option<Sender<RetryStreamMessage>>,
    ) -> Result<Self, std::io::Error> {
        let file = path_abs::FileRead::open(path)?;
        let inode = get_inode(path, Some(file.as_ref()))?;
        Ok(Self {
            inner: Arc::new(Mutex::new(TailedFileInner {
                reader: BufReader::new(tokio::fs::File::from_std(file.into())).compat(),
                buf: Vec::new(),
                offset: 0,
                path: path.to_path_buf(),
                initial_offsets,
                inode,
                retries: 0,
                retrying_until: None,
            })),
            resume_events_sender,
            retry_events_sender,
            _phantom: std::marker::PhantomData::<T>,
        })
    }

    pub fn get_inner(&self) -> &Arc<Mutex<TailedFileInner>> {
        &self.inner
    }
}

impl TailedFile<LineBuilder> {
    // tail a file for new line(s)
    pub async fn tail(
        &mut self,
        paths: &[PathBuf],
        _seek_to: Option<u64>,
    ) -> Option<impl Stream<Item = LineBuilder>> {
        // get the file len
        {
            let mut inner = self.inner.lock().await;
            let len = match inner
                .reader
                .get_ref()
                .get_ref()
                .metadata()
                .await
                .map(|m| m.len())
            {
                Ok(v) => v,
                Err(e) => {
                    error!("unable to stat {:?}: {:?}", &paths[0], e);
                    return None;
                }
            };

            if let Some(initial_offset) = inner.initial_offsets.pop_first().map(|offset| offset.end)
            {
                inner.offset = inner
                    .reader
                    .get_mut()
                    .get_mut()
                    .seek(SeekFrom::Start(initial_offset))
                    .await
                    .map_err(|e| error!("{:?}", e))
                    .unwrap_or(0);
                info!("initial_offset {} for {}", inner.offset, inner.inode);
            };

            // if we are at the end of the file there's no work to do
            if inner.offset == len {
                return None;
            }

            // if the offset is greater than the file's len
            // it's very likely a truncation occurred
            if inner.offset > len {
                info!(
                    "{:?} was truncated from {} to {}",
                    &paths[0], inner.offset, len
                );
                // Reset offset back to the start... ish?
                // TODO: Work out the purpose of the 8192 something to do with lookback? That seems wrong.
                inner.offset = if len < 8192 { 0 } else { len };
                // seek to the offset, this creates the "tailing" effect
                let offset = inner.offset;
                if let Err(e) = inner
                    .reader
                    .get_mut()
                    .get_mut()
                    .seek(SeekFrom::Start(offset))
                    .await
                {
                    error!("error seeking {:?}", e);
                    return None;
                }
            }
        }

        Some(
            stream::unfold(self.inner.clone(), move |rc_reader| async move {
                let reader = rc_reader.clone();
                let mut borrow = reader.try_lock().unwrap();
                let TailedFileInner {
                    ref mut reader,
                    ref mut buf,
                    ref mut offset,
                    ..
                } = borrow.deref_mut();

                let res = {
                    let mut pinned_reader = Pin::new(reader);
                    if let Some(c) = buf.last() {
                        if *c == b'\n' {
                            buf.clear();
                        }
                    }
                    pinned_reader.read_until(b'\n', buf).await
                };
                let c = match res {
                    Ok(0) => return None,
                    Ok(n) => n,
                    Err(e) => {
                        warn!("error encountered while tailing file: {}", e);
                        return Some((Err(e), rc_reader));
                    }
                };
                let n: u64 = c.try_into().unwrap();
                *offset += n;
                let mut s = String::from_utf8_lossy(&buf[..c]).to_string();
                if s.ends_with('\n') {
                    s.pop();
                    if s.ends_with('\r') {
                        s.pop();
                    }
                    Metrics::fs().add_bytes(n);
                    Some((Ok(s), rc_reader))
                } else {
                    None
                }
            })
            .filter_map({
                let paths: Vec<_> = paths.to_vec();
                move |line_res| {
                    let paths = paths.clone();
                    async move {
                        let paths = paths.clone();
                        line_res.ok().map({
                            move |line| {
                                debug!("tailer sendings lines for {:?}", paths);
                                stream::iter(paths.into_iter().map({
                                    move |path| {
                                        Metrics::fs().increment_lines();
                                        LineBuilder::new()
                                            .line(line.clone())
                                            .file(path.to_str().unwrap_or("").to_string())
                                    }
                                }))
                            }
                        })
                    }
                }
            })
            .flatten(),
        )
    }
}

impl RetryableLine for LazyLineSerializer {
    fn retries_remaining(&self) -> u32 {
        let inner = self.reader.try_lock().unwrap();
        5 - inner.retries
    }

    fn retry_at(&self) -> time::OffsetDateTime {
        OffsetDateTime::now_utc() + self.retry_after()
    }

    fn retry_after(&self) -> std::time::Duration {
        let inner = self.reader.try_lock().unwrap();
        inner.retries * std::time::Duration::from_secs(5)
    }

    fn retry(&self, delay: Option<std::time::Duration>) -> Result<(), SourceError> {
        let retries_remaining = self.retries_remaining();
        let retry_at = self.retry_at();
        let mut inner = self.reader.try_lock().unwrap();

        if retries_remaining > 0 {
            let delay = delay
                .map(|delay| OffsetDateTime::now_utc() + delay * inner.retries)
                .unwrap_or(retry_at);

            let path = inner.path.clone();
            let event = WatchEvent::Write(path);
            if inner.retrying_until.is_none() {
                inner.retries += 1;
                inner.retrying_until = Some(delay);
                if let Some(retry_events_send) = self.retry_events_send.as_ref() {
                    retry_events_send
                        .send_blocking((event, delay, inner.retries, Some(self.file_offset.1)))
                        .unwrap();
                }
            }
        }
        Ok(())
    }

    fn commit(&self) -> Result<(), SourceError> {
        // TODO
        Ok(())
    }
}

pub struct LazyLines {
    reader: Arc<Mutex<TailedFileInner>>,
    total_read: usize,
    target_read: Option<usize>,
    paths: Rc<[String]>,
    resume_channel_send: Option<async_channel::Sender<(u64, OffsetDateTime)>>,
    retry_channel_send: Option<Sender<RetryStreamMessage>>,
}

impl LazyLines {
    pub async fn new(
        reader: Arc<Mutex<TailedFileInner>>,
        paths: Rc<[String]>,
        target_read: Option<u64>,
        resume_channel_send: Option<Sender<(u64, OffsetDateTime)>>,
        retry_channel_send: Option<Sender<RetryStreamMessage>>,
    ) -> Self {
        let (initial_end, initial_offset): (Option<u64>, Option<u64>) = {
            let inner = &mut reader.lock().await.reader;

            let inner = inner.get_mut().get_mut();
            let initial_offset = match inner.seek(SeekFrom::Current(0)).await {
                Ok(v) => Some(v),
                Err(e) => {
                    error!("unable to get current file offset {:?}: {:?}", &paths[0], e);
                    None
                }
            };

            let initial_end = if target_read.is_some() {
                target_read
            } else {
                match inner.metadata().await.map(|m| m.len()) {
                    Ok(v) => Some(v),
                    Err(e) => {
                        error!("unable to stat {:?}: {:?}", &paths[0], e);
                        None
                    }
                }
            };

            (initial_end, initial_offset)
        };
        let target_read: Option<usize> = match (initial_end, initial_offset) {
            (Some(e), Some(o)) => (e - o).try_into().ok(),
            _ => None,
        };
        Self {
            reader,
            total_read: 0,
            target_read,
            paths,
            resume_channel_send,
            retry_channel_send,
        }
    }
}

impl TailedFile<LazyLineSerializer> {
    // tail a file for new line(s)
    pub(crate) async fn tail(
        &mut self,
        paths: &[PathBuf],
        seek_to: Option<u64>,
    ) -> Option<impl Stream<Item = LazyLineSerializer>> {
        let target_read = {
            let mut inner = self.inner.lock().await;

            let len = match inner
                .reader
                .get_ref()
                .get_ref()
                .metadata()
                .await
                .map(|m| m.len())
            {
                Ok(v) => v,
                Err(e) => {
                    error!("unable to stat {:?}: {:?}", &paths[0], e);
                    return None;
                }
            };

            // line retry handling
            let target_read = if let Some(seek_to) = seek_to {
                inner.offset = inner
                    .reader
                    .get_mut()
                    .get_mut()
                    .seek(SeekFrom::Start(seek_to))
                    .await
                    .map_err(|e| error!("{:?}", e))
                    .unwrap_or(0);
                None
            } else {
                // else we're on a normal read

                if let Some(retrying_until) = inner.retrying_until {
                    if retrying_until
                        >= OffsetDateTime::now_utc() - std::time::Duration::from_millis(100)
                    {
                        debug!("waiting to retry, skipping read on {:#?}", paths);
                        if inner.retries >= 5 {
                            debug!(
                                "queuing a retry for {:?} after {:#?}",
                                paths,
                                retrying_until - OffsetDateTime::now_utc()
                                    + std::time::Duration::from_millis(1000)
                            );
                            if let Some(retry_events_send) = self.retry_events_sender.as_ref() {
                                let event = WatchEvent::Write(paths[0].clone());
                                retry_events_send
                                    .send_blocking((
                                        event,
                                        retrying_until + std::time::Duration::from_millis(1000),
                                        0,
                                        None,
                                    ))
                                    .unwrap();
                            }
                        }
                        return None;
                    } else {
                        // clear retry
                        debug!("clearing retries on {:#?}", paths);
                        inner.retrying_until = None;
                        if let Some(sender) = self.resume_events_sender.as_ref() {
                            if let Err(e) =
                                sender.try_send((inner.inode, OffsetDateTime::now_utc()))
                            {
                                warn!("Couldn't send tailer continuation event: {}", e);
                            } else {
                                debug!(
                                    "sending resume event for {:#?} at offset {:#?}",
                                    paths, inner.offset
                                );
                            };
                        }
                    }
                }

                if let Some(initial_offset) =
                    inner.initial_offsets.pop_first().map(|offset| offset.end)
                {
                    inner.offset = inner
                        .reader
                        .get_mut()
                        .get_mut()
                        .seek(SeekFrom::Start(initial_offset))
                        .await
                        .map_err(|e| error!("{:?}", e))
                        .unwrap_or(0)
                };

                let target_read = inner.initial_offsets.first().map(|offsets| offsets.start);

                target_read
            };
            // if we are at the end of the file there's no work to do
            if inner.offset == len {
                debug!("at the end of the file, no more work to do");
                return None;
            }

            // if the offset is greater than the file's len
            // it's very likely a truncation occurred
            if inner.offset > len {
                info!(
                    "{:?} was truncated from {} to {}",
                    &paths[0], inner.offset, len
                );
                // Reset offset back to the start... ish?
                // TODO: Work out the purpose of the 8192 something to do with lookback? That seems wrong.
                inner.offset = if len < 8192 { 0 } else { len };
                // seek to the offset, this creates the "tailing" effect
                let offset = inner.offset;
                if let Err(e) = inner
                    .reader
                    .get_mut()
                    .get_mut()
                    .seek(SeekFrom::Start(offset))
                    .await
                {
                    error!("error seeking {:?}", e);
                    return None;
                }
            }

            target_read
        };

        Some(
            stream::unfold(
                LazyLines::new(
                    self.inner.clone(),
                    Rc::from(
                        paths
                            .iter()
                            .map(|path| path.to_string_lossy().into())
                            .collect::<Vec<_>>(),
                    ),
                    target_read,
                    self.resume_events_sender.clone(),
                    self.retry_events_sender.clone(),
                )
                .await,
                |mut lazy_lines| async move {
                    let LazyLines {
                        ref reader,
                        ref mut total_read,
                        ref target_read,
                        ref paths,
                        ref resume_channel_send,
                        ref retry_channel_send,
                        ..
                    } = lazy_lines;

                    let rc_reader = reader.clone();
                    let retry_channel_send = retry_channel_send.clone();
                    // Get the next line
                    let mut borrow = rc_reader.try_lock().unwrap();
                    let TailedFileInner {
                        ref mut reader,
                        ref mut buf,
                        ref mut offset,
                        ref mut inode,
                        ref path,
                        ..
                    } = borrow.deref_mut();

                    let mut initial_offset = *offset;
                    if let Some(c) = buf.last() {
                        if *c == b'\n' {
                            buf.clear();
                        } else {
                            initial_offset -= u64::try_from(buf.len())
                                .expect("Couldn't convert buffer length to u64")
                        }
                    }

                    let mut pinned_reader = Pin::new(reader);
                    // If we've read more than a 16 KB from this one event and reached the end
                    // of the file as it was when we started break to prevent starvation
                    if *total_read > (1024 * 16) {
                        if let Some(target_read) = target_read {
                            if *total_read > *target_read {
                                debug!("read 16KB from a single event, returning");

                                // put event on watch stream to ensure processing completes
                                if let Some(sender) = resume_channel_send {
                                    if let Err(e) =
                                        sender.try_send((*inode, OffsetDateTime::now_utc()))
                                    {
                                        warn!("Couldn't send tailer continuation event: {}", e);
                                    };
                                }
                                return None;
                            }
                        }
                    }

                    // Read a line into the internal buffer
                    let result = pinned_reader.read_until(b'\n', buf).await;
                    match result {
                        Ok(count) if count > 0 => {
                            if let Some(c) = buf.last() {
                                if *c == b'\n' {
                                    *total_read += count;
                                    debug!("tailer sendings lines for {:?}", &paths);
                                    let count = TryInto::<u64>::try_into(count).unwrap();
                                    Metrics::fs().increment_lines();
                                    Metrics::fs().add_bytes(count);
                                    *offset += count;
                                    let ret = (0..paths.len()).map({
                                        let paths = paths.clone();
                                        let rc_reader = rc_reader.clone();
                                        let retry_channel_send = retry_channel_send.clone();
                                        let current_offset = (*inode, initial_offset, *offset);
                                        move |path_idx| {
                                            LazyLineSerializer::new(
                                                rc_reader.clone(),
                                                paths[path_idx].clone(),
                                                current_offset,
                                                retry_channel_send.clone(),
                                            )
                                        }
                                    });
                                    Some((Ok(stream::iter(ret)), lazy_lines))
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        }
                        Ok(_) => None,
                        // We got an io error, should we propagate this up somehow? calls to TailedFile::tail
                        // will implicitly retry
                        Err(e) => {
                            warn!("{}", e);
                            if e.kind() == std::io::ErrorKind::NotFound {
                                warn!("Attempting to reopen {:?}", path);
                                match path_abs::FileRead::open(path).map_err(Into::into).and_then(
                                    |file| {
                                        get_inode(path, Some(file.as_ref()))
                                            .map(|inode| (file.into(), inode))
                                    },
                                ) {
                                    Ok((file, new_inode)) => {
                                        let reader_bufreader = pinned_reader.get_mut().get_mut();
                                        *reader_bufreader = tokio::io::BufReader::new(
                                            tokio::fs::File::from_std(file),
                                        );
                                        *inode = new_inode;
                                        *offset = 0;
                                        if !buf.is_empty() {
                                            warn!("Dropping trailing data from unreachable file");
                                        }
                                        buf.clear();
                                    }
                                    e => {
                                        warn!("Error attempting to reopen {:?}", e);
                                    }
                                }
                            };
                            Some((
                                Err(std::io::Error::new(
                                    std::io::ErrorKind::Other,
                                    format!(
                                        "unable to read tailed file for path \"{:?}\": {:?}",
                                        path, e
                                    ),
                                )),
                                lazy_lines,
                            ))
                        }
                    }
                },
            )
            .filter_map(move |line_res| async move {
                // Discard errors
                line_res.ok()
            })
            .flatten(),
        )
    }
}

/// Returns a Bytes using a copy of the line without the last char.
fn line_bytes(buf: &[u8]) -> Bytes {
    // This method can be removed once we re-implement a line reader
    Bytes::copy_from_slice(&buf[..buf.len().saturating_sub(1)])
}

#[cfg(test)]
mod tests {
    use super::*;
    use middleware::line_rules::LineRules;
    use middleware::{Middleware, Status};
    use tempfile::tempdir;

    use std::fs::OpenOptions;

    #[test]
    fn lazy_lines_should_get_set_line_with_rules() {
        let mut l = get_line();
        let buf = b"hello trace world".to_vec();
        l.reader.try_lock().unwrap().deref_mut().buf = buf;

        {
            let exclusion = ["DEBUG".to_owned(), "(?i:TRACE)".to_owned()];
            let p = LineRules::new(&exclusion, &[], &[]).unwrap();
            assert!(matches!(p.process(&mut l), Status::Skip));
        }
        {
            let inclusion = ["DEBUG".to_owned(), "(?i:TRACE)".to_owned()];
            let p = LineRules::new(&[], &inclusion, &[]).unwrap();

            assert!(matches!(p.process(&mut l), Status::Ok(_)));
        }
    }

    #[test]
    fn lazy_lines_should_support_redaction() {
        let redact = ["NAME".to_owned(), r"\d+".to_owned()];
        let mut l = get_line();

        {
            let buf = b"my name is NAME and I was born in the year 1914".to_vec();
            l.reader.try_lock().unwrap().deref_mut().buf = buf;
            let p = LineRules::new(&[], &[], &redact).unwrap();
            match p.process(&mut l) {
                Status::Ok(_) => assert_eq!(
                    std::str::from_utf8(l.get_line_buffer().unwrap()).unwrap(),
                    "my name is [REDACTED] and I was born in the year [REDACTED]"
                ),
                _ => panic!("it should have been OK"),
            }
        }
    }

    fn get_line() -> LazyLineSerializer {
        let file_path = tempdir().unwrap().into_path().join("test.log");
        let file_inner = Arc::new(Mutex::new(TailedFileInner {
            reader: BufReader::new(tokio::fs::File::from_std(
                OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create(true)
                    .truncate(true)
                    .open(&file_path)
                    .unwrap(),
            ))
            .compat(),
            buf: Vec::new(),
            offset: 0,
            initial_offsets: SpanVec::new(),
            inode: 0,
            path: file_path,
            retries: 0,
            retrying_until: None,
        }));
        LazyLineSerializer::new(file_inner, "file/path.log".to_owned(), (0, 0, 0), None)
    }
}

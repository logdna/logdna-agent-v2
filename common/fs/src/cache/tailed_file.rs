#![allow(clippy::await_holding_refcell_ref)]
use http::types::body::{KeyValueMap, LineBuilder, LineMeta, LineMetaMut};
use http::types::error::LineMetaError;
use http::types::serialize::{
    IngestLineSerialize, IngestLineSerializeError, SerializeI64, SerializeMap, SerializeStr,
    SerializeUtf8, SerializeValue,
};

use chrono::Utc;
use metrics::Metrics;

use futures::io::AsyncBufRead;
use futures::lock::Mutex;
use futures::task::{Context, Poll};
use futures::{ready, Stream, StreamExt};

use async_trait::async_trait;
use pin_project_lite::pin_project;

use serde_json::Value;

use std::collections::HashMap;
use std::convert::TryInto;
use std::fs::OpenOptions;
use std::io;
use std::mem;
use std::ops::DerefMut;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use tokio::io::{BufReader, SeekFrom};
use tokio_util::compat::{Compat, Tokio02AsyncReadCompatExt};

fn read_until_internal<R: AsyncBufRead + ?Sized>(
    mut reader: Pin<&mut R>,
    cx: &mut Context<'_>,
    byte: u8,
    buf: &mut Vec<u8>,
    read: &mut usize,
) -> Poll<io::Result<usize>> {
    loop {
        let (done, used) = {
            let available = ready!(reader.as_mut().poll_fill_buf(cx))?;
            if let Some(i) = memchr::memchr(byte, available) {
                buf.extend_from_slice(&available[..=i]);
                (true, i + 1)
            } else {
                buf.extend_from_slice(available);
                (false, available.len())
            }
        };
        reader.as_mut().consume(used);
        *read += used;
        if done || used == 0 {
            if used == 0 {
                Metrics::fs().increment_partial_reads();
            }
            return Poll::Ready(Ok(mem::replace(read, 0)));
        }
    }
}

fn read_line_lossy<R: AsyncBufRead + ?Sized>(
    reader: Pin<&mut R>,
    cx: &mut Context<'_>,
    bytes: &mut Vec<u8>,
    read: &mut usize,
) -> Poll<io::Result<(String, usize)>> {
    match ready!(read_until_internal(reader, cx, b'\n', bytes, read)) {
        Ok(count) => {
            debug_assert_eq!(*read, 0);
            let ret = String::from_utf8_lossy(bytes).to_string();
            bytes.clear();
            Poll::Ready(Ok((ret, count)))
        }
        Err(e) => Poll::Ready(Err(e)),
    }
}

pin_project! {
    #[derive(Debug)]
    #[must_use = "streams do nothing unless polled"]
    pub struct LineBuilderLines {
        #[pin]
        reader: Arc<Mutex<TailedFileInner>>,
        read: usize,
    }
}

impl LineBuilderLines {
    pub fn new(reader: Arc<Mutex<TailedFileInner>>) -> Self {
        Self { reader, read: 0 }
    }
}

impl Stream for LineBuilderLines {
    type Item = io::Result<String>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.project();
        let mut borrow = this.reader.try_lock().unwrap();
        let TailedFileInner {
            ref mut reader,
            ref mut buf,
            ref mut offset,
        } = borrow.deref_mut();

        let pinned_reader = Pin::new(reader);
        let (mut s, n) = ready!(read_line_lossy(pinned_reader, cx, buf, this.read))?;
        if n == 0 && s.is_empty() {
            return Poll::Ready(None);
        }

        let n: u64 = n.try_into().unwrap();
        if s.ends_with('\n') {
            s.pop();
            if s.ends_with('\r') {
                s.pop();
            }
        }

        *offset += n;

        Metrics::fs().add_bytes(n);
        Poll::Ready(Some(Ok(s)))
    }
}

pub struct LazyLines {
    reader: Arc<Mutex<TailedFileInner>>,
    current: Option<Arc<Mutex<TailedFileInner>>>,
    read: usize,
    path: usize,
    paths: Vec<String>,
}

impl LazyLines {
    pub fn new(reader: Arc<Mutex<TailedFileInner>>, paths: Vec<String>) -> Self {
        Self {
            reader,
            current: None,
            read: 0,
            path: 0,
            paths,
        }
    }
}

#[derive(Debug)]
pub struct LazyLineSerializer {
    annotations: Option<KeyValueMap>,
    app: Option<String>,
    env: Option<String>,
    host: Option<String>,
    labels: Option<KeyValueMap>,
    level: Option<String>,
    meta: Option<Value>,

    path: String,

    reader: Arc<Mutex<TailedFileInner>>,
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
            ser.serialize_map(&annotations).await?;
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
        writer.serialize_str(&self.path).await?;
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
            ser.serialize_map(&labels).await?;
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
        let bytes = {
            let borrowed_reader = self.reader.lock().await;
            bytes::Bytes::copy_from_slice(&borrowed_reader.buf)
        };
        writer.serialize_utf8(bytes).await?;

        Ok(())
    }
    async fn timestamp<S>(&mut self, writer: &mut S) -> Result<Self::Ok, IngestLineSerializeError>
    where
        S: SerializeI64 + std::marker::Send,
    {
        writer.serialize_i64(&Utc::now().timestamp()).await?;

        Ok(())
    }
    fn field_count(&self) -> usize {
        3 + if Option::is_none(&self.annotations) {
            0
        } else {
            1
        } + if Option::is_none(&self.app) { 0 } else { 1 }
            + if Option::is_none(&self.env) { 0 } else { 1 }
            + if Option::is_none(&self.host) { 0 } else { 1 }
            + if Option::is_none(&self.labels) { 0 } else { 1 }
            + if Option::is_none(&self.level) { 0 } else { 1 }
            + if Option::is_none(&self.meta) { 0 } else { 1 }
    }
}

impl LazyLineSerializer {
    pub fn new(reader: Arc<Mutex<TailedFileInner>>, path: String) -> Self {
        // New line, make sure the buffer is cleared
        reader.try_lock().unwrap().deref_mut().buf.clear();
        Self {
            reader,
            path,
            annotations: None,
            app: None,
            env: None,
            host: None,
            labels: None,
            level: None,
            meta: None,
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
        Some(self.path.as_str())
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
        self.path = file;
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

impl Stream for LazyLines {
    type Item = LazyLineSerializer;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // TODO: Work out how to signal file done, None line and Some(0) read?

        let LazyLines {
            reader,
            ref mut read,
            ref mut current,
            ref mut path,
            paths,
            ..
        } = self.get_mut();
        let rc_reader = reader;
        loop {
            if current.is_some() && *path < paths.len() {
                let ret = LazyLineSerializer::new(rc_reader.clone(), paths[*path].clone());
                *path += 1;
                break Poll::Ready(Some(ret));
            }
            // Get the next line
            let mut borrow = rc_reader.try_lock().unwrap();
            let TailedFileInner {
                ref mut reader,
                ref mut buf,
                ref mut offset,
            } = borrow.deref_mut();

            if *path >= paths.len() {
                buf.clear();
            }

            let pinned_reader = Pin::new(reader);
            if let Ok(read) = ready!(read_until_internal(pinned_reader, cx, b'\n', buf, read)) {
                if read == 0 {
                    break Poll::Ready(None);
                } else {
                    debug!("tailer sendings lines for {:?}", &paths);
                    *path = 0;
                    *offset += TryInto::<u64>::try_into(read).unwrap();
                    *current = Some(rc_reader.clone())
                }
            }
        }
    }
}

#[derive(Debug)]
pub struct TailedFileInner {
    reader: Compat<tokio::io::BufReader<tokio::fs::File>>,
    buf: Vec<u8>,
    offset: u64,
}

#[derive(Debug, Clone)]
pub struct TailedFile<T> {
    inner: Arc<Mutex<TailedFileInner>>,
    _phantom: std::marker::PhantomData<T>,
}

impl<T> TailedFile<T> {
    pub(crate) fn new(path: &Path) -> Result<Self, std::io::Error> {
        Ok(Self {
            inner: Arc::new(Mutex::new(TailedFileInner {
                reader: BufReader::new(tokio::fs::File::from_std(
                    OpenOptions::new().read(true).open(path)?,
                ))
                .compat(),
                buf: Vec::new(),
                offset: 0,
            })),
            _phantom: std::marker::PhantomData::<T>,
        })
    }
    pub(crate) async fn seek(&mut self, offset: u64) -> Result<(), std::io::Error> {
        let mut inner = self.inner.lock().await;
        inner.offset = offset;
        inner
            .reader
            .get_mut()
            .get_mut()
            .seek(SeekFrom::Start(offset))
            .await?;
        Ok(())
    }
}

impl TailedFile<LineBuilder> {
    // tail a file for new line(s)
    pub async fn tail(&mut self, paths: Vec<PathBuf>) -> Option<impl Stream<Item = LineBuilder>> {
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
            LineBuilderLines::new(self.inner.clone())
                .filter_map({
                    let paths = paths.clone();
                    move |line_res| {
                        let paths = paths.clone();
                        async move {
                            let paths = paths.clone();
                            line_res.ok().map({
                                move |line| {
                                    debug!("tailer sendings lines for {:?}", paths);
                                    futures::stream::iter(paths.into_iter().map({
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

impl TailedFile<LazyLineSerializer> {
    // tail a file for new line(s)
    pub(crate) async fn tail(
        &mut self,
        paths: Vec<PathBuf>,
    ) -> Option<impl Stream<Item = LazyLineSerializer>> {
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

        debug!("tailer sendings lines for {:?}", &paths);
        Some(LazyLines::new(
            self.inner.clone(),
            paths
                .into_iter()
                .map(|path| path.to_string_lossy().into())
                .collect(),
        ))
    }
}

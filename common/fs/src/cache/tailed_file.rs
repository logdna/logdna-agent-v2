use http::types::body::{KeyValueMap, LineBuilder, LineMeta, LineMetaMut, LineBufferMut};
use http::types::error::LineMetaError;
use http::types::serialize::{
    IngestLineSerialize, IngestLineSerializeError, SerializeI64, SerializeMap, SerializeStr,
    SerializeUtf8, SerializeValue,
};

use state::GetOffset;

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
use std::num::NonZeroUsize;
use std::ops::DerefMut;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use tokio::io::{AsyncSeekExt, BufReader, SeekFrom};
use tokio_util::compat::{Compat, TokioAsyncReadCompatExt};

fn read_until_internal<R: AsyncBufRead + ?Sized>(
    mut reader: Pin<&mut R>,
    cx: &mut Context<'_>,
    byte: u8,
    buf: &mut Vec<u8>,
    read: &mut usize,
) -> Poll<io::Result<Option<NonZeroUsize>>> {
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
        if done {
            debug_assert!(*read > 0);
            return Poll::Ready(Ok(Some(
                NonZeroUsize::new(mem::replace(read, 0))
                    .expect("No such thing as a line 0 bytes long"),
            )));
        }
        if used == 0 {
            // We've hit the end of the file and not finished a line
            Metrics::fs().increment_partial_reads();
            return Poll::Ready(Ok(None));
        }
    }
}

fn read_line_lossy<R: AsyncBufRead + ?Sized>(
    reader: Pin<&mut R>,
    cx: &mut Context<'_>,
    bytes: &mut Vec<u8>,
    read: &mut usize,
) -> Poll<io::Result<Option<(String, NonZeroUsize)>>> {
    match ready!(read_until_internal(reader, cx, b'\n', bytes, read))? {
        Some(count) => {
            let ret = String::from_utf8_lossy(bytes).to_string();
            bytes.clear();
            Poll::Ready(Ok(Some((ret, count))))
        }
        None => Poll::Ready(Ok(None)),
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
            ..
        } = borrow.deref_mut();

        let pinned_reader = Pin::new(reader);
        let (mut s, n) = match ready!(read_line_lossy(pinned_reader, cx, buf, this.read)) {
            Ok(Some((s, n))) => (s, n),
            Err(e) => return Poll::Ready(Some(Err(e))),
            Ok(None) => return Poll::Ready(None),
        };
        let n: u64 = n.get().try_into().unwrap();
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
    current_offset: Option<(u64, u64)>,
    read: usize,
    path: usize,
    paths: Vec<String>,
}

impl LazyLines {
    pub fn new(reader: Arc<Mutex<TailedFileInner>>, paths: Vec<String>) -> Self {
        Self {
            reader,
            current_offset: None,
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
    line_buffer: Option<Vec<u8>>,

    file_offset: (u64, u64),

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
        // Try to use the cached value first
        let replaced_buf = self.line_buffer.take();
        let bytes = if let Some(buf) = replaced_buf {
            bytes::Bytes::from(buf)
        } else {
            let borrowed_reader = self.reader.lock().await;
            bytes::Bytes::copy_from_slice(&borrowed_reader.buf[..borrowed_reader.buf.len() - 1])
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
    pub fn new(reader: Arc<Mutex<TailedFileInner>>, path: String, offset: (u64, u64)) -> Self {
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
            line_buffer: None,
            file_offset: offset,
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

impl LineBufferMut for LazyLineSerializer {
    fn get_line_buffer(&mut self) -> Option<&[u8]> {
        if self.line_buffer.as_ref().is_some() {
            // Get the value without locking
            return self.line_buffer.as_deref();
        }

        match self.reader.try_lock() {
            Some(file_inner) => {
                // Cache the value to avoid excessive cloning
                self.line_buffer = Some(file_inner.buf.clone());
                self.line_buffer.as_deref()
            }
            None => None
        }
    }

    fn set_line_buffer(&mut self, line: Vec<u8>) -> Result<(), LineMetaError> {
        self.line_buffer = Some(line);
        Ok(())
    }
}

impl GetOffset for LazyLineSerializer {
    fn get_offset(&self) -> Option<u64> {
        Some(self.file_offset.1)
    }
    fn get_key(&self) -> Option<u64> {
        Some(self.file_offset.0)
    }
}

impl Stream for LazyLines {
    type Item = LazyLineSerializer;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // TODO: Work out how to signal file done, None line and Some(0) read?

        let LazyLines {
            reader,
            ref mut read,
            ref mut current_offset,
            ref mut path,
            paths,
            ..
        } = self.get_mut();
        let rc_reader = reader;
        loop {
            if current_offset.is_some() && *path < paths.len() {
                let ret = LazyLineSerializer::new(
                    rc_reader.clone(),
                    paths[*path].clone(),
                    current_offset.clone().unwrap(),
                );
                *path += 1;
                Metrics::fs().increment_lines();
                break Poll::Ready(Some(ret));
            }
            // Get the next line
            let mut borrow = rc_reader.try_lock().unwrap();
            let TailedFileInner {
                ref mut reader,
                ref mut buf,
                ref mut offset,
                ref inode,
                ..
            } = borrow.deref_mut();

            if *path >= paths.len() {
                debug_assert_eq!(*read, 0);
                *current_offset = None;
                *path = 0;
                buf.clear();
            }

            let pinned_reader = Pin::new(reader);
            let result = ready!(read_until_internal(pinned_reader, cx, b'\n', buf, read));
            match result {
                Ok(Some(count)) => {
                    // Got a line
                    debug_assert_eq!(*read, 0);
                    debug!("tailer sendings lines for {:?}", &paths);
                    let count = TryInto::<u64>::try_into(count.get()).unwrap();
                    Metrics::fs().add_bytes(count);
                    *offset += count;
                    *current_offset = Some((*inode, *offset))
                }
                // We got an error, should we propagate this up somehow? calls to TailedFile::tail
                // will implicitly retry
                Err(e) => warn!("{}", e),
                // Reached the end of the file, but havn't hit a newline yet
                Ok(None) => break Poll::Ready(None),
            }
        }
    }
}

#[derive(Debug)]
pub struct TailedFileInner {
    reader: Compat<tokio::io::BufReader<tokio::fs::File>>,
    buf: Vec<u8>,
    offset: u64,
    file_path: PathBuf,
    inode: u64,
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
                file_path: path.into(),
                inode: path.metadata()?.ino(),
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
    pub(crate) async fn get_inode(&self) -> u64 {
        let inner = self.inner.lock().await;
        inner.inode
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

        Some(LazyLines::new(
            self.inner.clone(),
            paths
                .into_iter()
                .map(|path| path.to_string_lossy().into())
                .collect(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use middleware::line_rules::LineRules;
    use middleware::{Middleware, Status};
    use tempfile::tempdir;

    #[test]
    fn lazy_lines_should_get_set_line_with_rules() {
        let mut l = get_line();
        let buf = b"hello trace world".to_vec();
        l.reader.try_lock().unwrap().deref_mut().buf = buf;

        {
            let exclusion = &vec!["DEBUG".to_owned(), "(?i:TRACE)".to_owned()];
            let p = LineRules::new(exclusion, &[], &[]).unwrap();
            assert!(matches!(p.process(&mut l), Status::Skip));
        }
        {
            let inclusion = &vec!["DEBUG".to_owned(), "(?i:TRACE)".to_owned()];
            let p = LineRules::new(&[], inclusion, &[]).unwrap();
            assert!(matches!(p.process(&mut l), Status::Ok(_)));
        }
    }

    #[test]
    fn lazy_lines_should_support_redaction() {
        let redact = &vec!["NAME".to_owned(), r"\d+".to_owned()];
        let mut l = get_line();

        {
            let buf = b"my name is NAME and I was born in the year 1914".to_vec();
            l.reader.try_lock().unwrap().deref_mut().buf = buf;
            let p = LineRules::new(&[], &[], redact).unwrap();
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
                    .open(&file_path)
                    .unwrap(),
            ))
            .compat(),
            buf: Vec::new(),
            offset: 0,
            file_path,
            inode: 0,
        }));
        LazyLineSerializer::new(file_inner, "file/path.log".to_owned(), (0, 0))
    }
}

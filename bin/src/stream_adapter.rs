use async_trait::async_trait;

use fs::cache::tailed_file::LazyLineSerializer;
use http::types::body::{KeyValueMap, Line, LineBufferMut, LineBuilder, LineMeta, LineMetaMut};
use http::types::error::{LineError, LineMetaError};
use http::types::serialize::{
    IngestLineSerialize, IngestLineSerializeError, SerializeI64, SerializeMap, SerializeStr,
    SerializeUtf8, SerializeValue,
};

use types::sources::{RetryableLine, RetryableSource, SourceError, SourceRetry};

use serde_json::Value;

use futures::Stream;
use state::GetOffset;
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

#[derive(Debug)]
pub struct RetryableLineBuilder<T, R> {
    pub retryer: Option<R>,
    pub line: T,
}

impl<R> RetryableLineBuilder<LineBuilder, R> {
    pub fn build(self) -> Result<Line, LineError> {
        self.line.build()
    }
}

pub struct RetryableLineStream<'a, T> {
    delay_stream: Option<std::sync::Arc<DelayStream<T>>>,
    stream: Pin<Box<dyn Stream<Item = T> + 'a>>,
}

impl<'a, T> RetryableLineStream<'a, T> {
    fn new(
        stream: Pin<Box<dyn Stream<Item = T> + 'a>>,
        delay: Option<std::time::Duration>,
    ) -> Self {
        let delay_stream = delay.and_then(|delay| {
            (delay > std::time::Duration::from_secs(0)).then_some(Arc::new(DelayStream::new(delay)))
        });
        Self {
            delay_stream,
            stream,
        }
    }
}

impl<'a> RetryableSource<LineBuilder> for RetryableLineStream<'a, LineBuilder> {
    type RetryableLine =
        RetryableLineBuilder<LineBuilder, std::sync::Weak<DelayStream<LineBuilder>>>;

    fn retryable(&self, line: LineBuilder) -> Self::RetryableLine {
        RetryableLineBuilder {
            retryer: self.delay_stream.as_ref().map(Arc::downgrade),
            line,
        }
    }
}

impl<'a> Stream for RetryableLineStream<'a, LineBuilder> {
    type Item = RetryableLineBuilder<LineBuilder, std::sync::Weak<DelayStream<LineBuilder>>>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // TODO poll DelayStream for any delayed events that are ready
        self.stream
            .as_mut()
            .poll_next(cx)
            .map(|opt| opt.map(|line| self.retryable(line)))
    }
}

pub fn retryable_line_stream<'a, T>(
    stream: impl Stream<Item = T> + 'a,
    delay: Option<std::time::Duration>,
) -> RetryableLineStream<'a, T>
where
    T: LineBufferMut + std::marker::Send + std::marker::Sync + 'a,
{
    RetryableLineStream::new(Box::pin(stream), delay)
}

/// A Stream extension trait allowing you to call `retryable_line_stream` on any `Stream`
/// of objects implementing IngestLineSerialize
pub trait RetryableLineStreamExt: Stream {
    fn retryable_lines<'a>(
        self,
        delay: Option<std::time::Duration>,
    ) -> RetryableLineStream<'a, Self::Item>
    where
        Self::Item: LineBufferMut + std::marker::Send + std::marker::Sync + 'a,
        Self: Sized + 'a,
    {
        retryable_line_stream(self, delay)
    }
}

// TODO: Replace with actual impl
pub struct DelayStream<T> {
    _sender: async_channel::Sender<T>,
    _receiver: async_channel::Receiver<T>,
}

impl<T> DelayStream<T> {
    fn new(_delay: std::time::Duration) -> Self {
        let (sender, receiver) = async_channel::unbounded();
        DelayStream {
            _sender: sender,
            _receiver: receiver,
        }
    }
    fn send_to_retry(&self, _line: &T, _delay: std::time::Duration) -> Result<(), SourceError> {
        // TODO: reintroduce delayed sending for non-file lines
        Ok(())
    }
}

impl<T> SourceRetry for DelayStream<T> {
    type RetryableLine = T;
    fn retry(
        &self,
        line: &Self::RetryableLine,
        delay: std::time::Duration,
    ) -> Result<(), SourceError> {
        self.send_to_retry(line, delay)
    }

    fn commit(&self, _line: &Self::RetryableLine) -> Result<(), SourceError> {
        // TODO
        Ok(())
    }
}

impl<T: ?Sized> RetryableLineStreamExt for T where T: Stream {}

pub(crate) enum StrictOrLazyLineBuilder<T, R> {
    Strict(RetryableLineBuilder<T, R>),
    Lazy(LazyLineSerializer),
}

#[allow(clippy::large_enum_variant)]
pub(crate) enum StrictOrLazyLines {
    Strict(Line),
    Lazy(LazyLineSerializer),
}

#[async_trait]
impl IngestLineSerialize<String, bytes::Bytes, std::collections::HashMap<String, String>>
    for StrictOrLazyLines
{
    type Ok = ();

    fn has_annotations(&self) -> bool {
        match self {
            StrictOrLazyLines::Strict(line) => {
                line.get_annotations().filter(|s| !s.is_empty()).is_some()
            }
            StrictOrLazyLines::Lazy(line) => {
                line.get_annotations().filter(|s| !s.is_empty()).is_some()
            }
        }
    }
    async fn annotations<'b, S>(
        &mut self,
        writer: &mut S,
    ) -> Result<Self::Ok, IngestLineSerializeError>
    where
        S: SerializeMap<'b, HashMap<String, String>> + std::marker::Send,
    {
        match self {
            StrictOrLazyLines::Strict(line) => (&*line).annotations(writer).await,
            StrictOrLazyLines::Lazy(line) => line.annotations(writer).await,
        }
    }
    fn has_app(&self) -> bool {
        match self {
            StrictOrLazyLines::Strict(line) => line.get_app().filter(|s| !s.is_empty()).is_some(),
            StrictOrLazyLines::Lazy(line) => line.get_app().filter(|s| !s.is_empty()).is_some(),
        }
    }
    async fn app<S>(&mut self, writer: &mut S) -> Result<Self::Ok, IngestLineSerializeError>
    where
        S: SerializeStr<String> + std::marker::Send,
    {
        match self {
            StrictOrLazyLines::Strict(line) => (&*line).app(writer).await,
            StrictOrLazyLines::Lazy(line) => line.app(writer).await,
        }
    }
    fn has_env(&self) -> bool {
        match self {
            StrictOrLazyLines::Strict(line) => line.get_env().filter(|s| !s.is_empty()).is_some(),
            StrictOrLazyLines::Lazy(line) => line.get_env().filter(|s| !s.is_empty()).is_some(),
        }
    }
    async fn env<S>(&mut self, writer: &mut S) -> Result<Self::Ok, IngestLineSerializeError>
    where
        S: SerializeStr<String> + std::marker::Send,
    {
        match self {
            StrictOrLazyLines::Strict(line) => (&*line).env(writer).await,
            StrictOrLazyLines::Lazy(line) => line.env(writer).await,
        }
    }
    fn has_file(&self) -> bool {
        match self {
            StrictOrLazyLines::Strict(line) => line.get_file().filter(|s| !s.is_empty()).is_some(),
            StrictOrLazyLines::Lazy(line) => line.get_file().filter(|s| !s.is_empty()).is_some(),
        }
    }
    async fn file<S>(&mut self, writer: &mut S) -> Result<Self::Ok, IngestLineSerializeError>
    where
        S: SerializeStr<String> + std::marker::Send,
    {
        match self {
            StrictOrLazyLines::Strict(line) => (&*line).file(writer).await,
            StrictOrLazyLines::Lazy(line) => line.file(writer).await,
        }
    }
    fn has_host(&self) -> bool {
        match self {
            StrictOrLazyLines::Strict(line) => line.get_host().filter(|s| !s.is_empty()).is_some(),
            StrictOrLazyLines::Lazy(line) => line.get_host().filter(|s| !s.is_empty()).is_some(),
        }
    }
    async fn host<S>(&mut self, writer: &mut S) -> Result<Self::Ok, IngestLineSerializeError>
    where
        S: SerializeStr<String> + std::marker::Send,
    {
        match self {
            StrictOrLazyLines::Strict(line) => (&*line).host(writer).await,
            StrictOrLazyLines::Lazy(line) => line.host(writer).await,
        }
    }
    fn has_labels(&self) -> bool {
        match self {
            StrictOrLazyLines::Strict(line) => {
                line.get_labels().filter(|s| !s.is_empty()).is_some()
            }
            StrictOrLazyLines::Lazy(line) => line.get_labels().filter(|s| !s.is_empty()).is_some(),
        }
    }
    async fn labels<'b, S>(&mut self, writer: &mut S) -> Result<Self::Ok, IngestLineSerializeError>
    where
        S: SerializeMap<'b, HashMap<String, String>> + std::marker::Send,
    {
        match self {
            StrictOrLazyLines::Strict(line) => (&*line).labels(writer).await,
            StrictOrLazyLines::Lazy(line) => line.labels(writer).await,
        }
    }
    fn has_level(&self) -> bool {
        match self {
            StrictOrLazyLines::Strict(line) => line.get_level().filter(|s| !s.is_empty()).is_some(),
            StrictOrLazyLines::Lazy(line) => line.get_level().filter(|s| !s.is_empty()).is_some(),
        }
    }
    async fn level<S>(&mut self, writer: &mut S) -> Result<Self::Ok, IngestLineSerializeError>
    where
        S: SerializeStr<String> + std::marker::Send,
    {
        match self {
            StrictOrLazyLines::Strict(line) => (&*line).level(writer).await,
            StrictOrLazyLines::Lazy(line) => line.level(writer).await,
        }
    }
    fn has_meta(&self) -> bool {
        match self {
            StrictOrLazyLines::Strict(line) => line.get_meta().is_some(),
            StrictOrLazyLines::Lazy(line) => line.get_meta().is_some(),
        }
    }
    async fn meta<S>(&mut self, writer: &mut S) -> Result<Self::Ok, IngestLineSerializeError>
    where
        S: SerializeValue + std::marker::Send,
    {
        match self {
            StrictOrLazyLines::Strict(line) => (&*line).meta(writer).await,
            StrictOrLazyLines::Lazy(line) => line.meta(writer).await,
        }
    }
    async fn line<S>(&mut self, writer: &mut S) -> Result<Self::Ok, IngestLineSerializeError>
    where
        S: SerializeUtf8<bytes::Bytes> + std::marker::Send,
    {
        match self {
            StrictOrLazyLines::Strict(line) => (&*line).line(writer).await,
            StrictOrLazyLines::Lazy(line) => line.line(writer).await,
        }
    }
    async fn timestamp<S>(&mut self, writer: &mut S) -> Result<Self::Ok, IngestLineSerializeError>
    where
        S: SerializeI64 + std::marker::Send,
    {
        match self {
            StrictOrLazyLines::Strict(line) => (&*line).timestamp(writer).await,
            StrictOrLazyLines::Lazy(line) => line.timestamp(writer).await,
        }
    }
    fn field_count(&self) -> usize {
        match self {
            StrictOrLazyLines::Strict(line) => line.field_count(),
            StrictOrLazyLines::Lazy(line) => line.field_count(),
        }
    }
}

impl GetOffset for StrictOrLazyLines {
    fn get_offset(&self) -> Option<state::Span> {
        match self {
            StrictOrLazyLines::Strict(_) => None,
            StrictOrLazyLines::Lazy(line) => line.get_offset(),
        }
    }

    fn get_key(&self) -> Option<u64> {
        match self {
            StrictOrLazyLines::Strict(_) => None,
            StrictOrLazyLines::Lazy(line) => line.get_key(),
        }
    }
}

impl<T: LineMeta, R> StrictOrLazyLineBuilder<T, R> {
    fn as_line_meta(&self) -> &dyn LineMeta {
        use StrictOrLazyLineBuilder::*;
        match self {
            Strict(line) => line,
            Lazy(line) => line,
        }
    }
}

impl<T: LineMeta + LineMetaMut, R> StrictOrLazyLineBuilder<T, R> {
    fn as_line_meta_mut(&mut self) -> &mut dyn LineMetaMut {
        use StrictOrLazyLineBuilder::*;
        match self {
            Strict(line) => line,
            Lazy(line) => line,
        }
    }
}

impl<T, R> LineMeta for StrictOrLazyLineBuilder<T, R>
where
    T: LineMeta,
{
    fn get_annotations(&self) -> Option<&KeyValueMap> {
        self.as_line_meta().get_annotations()
    }
    fn get_app(&self) -> Option<&str> {
        self.as_line_meta().get_app()
    }
    fn get_env(&self) -> Option<&str> {
        self.as_line_meta().get_env()
    }
    fn get_file(&self) -> Option<&str> {
        self.as_line_meta().get_file()
    }
    fn get_host(&self) -> Option<&str> {
        self.as_line_meta().get_host()
    }
    fn get_labels(&self) -> Option<&KeyValueMap> {
        self.as_line_meta().get_labels()
    }
    fn get_level(&self) -> Option<&str> {
        self.as_line_meta().get_level()
    }
    fn get_meta(&self) -> Option<&Value> {
        self.as_line_meta().get_meta()
    }
}

impl<T, R> LineMetaMut for StrictOrLazyLineBuilder<T, R>
where
    T: LineMeta + LineMetaMut,
{
    fn get_annotations_mut(&mut self) -> &mut Option<KeyValueMap> {
        self.as_line_meta_mut().get_annotations_mut()
    }
    fn get_app_mut(&mut self) -> &mut Option<String> {
        self.as_line_meta_mut().get_app_mut()
    }
    fn get_env_mut(&mut self) -> &mut Option<String> {
        self.as_line_meta_mut().get_env_mut()
    }
    fn get_file_mut(&mut self) -> &mut Option<String> {
        self.as_line_meta_mut().get_file_mut()
    }
    fn get_host_mut(&mut self) -> &mut Option<String> {
        self.as_line_meta_mut().get_host_mut()
    }
    fn get_labels_mut(&mut self) -> &mut Option<KeyValueMap> {
        self.as_line_meta_mut().get_labels_mut()
    }
    fn get_level_mut(&mut self) -> &mut Option<String> {
        self.as_line_meta_mut().get_level_mut()
    }
    fn get_meta_mut(&mut self) -> &mut Option<Value> {
        self.as_line_meta_mut().get_meta_mut()
    }
    fn set_annotations(&mut self, annotations: KeyValueMap) -> Result<(), LineMetaError> {
        self.as_line_meta_mut().set_annotations(annotations)
    }
    fn set_app(&mut self, app: String) -> Result<(), LineMetaError> {
        self.as_line_meta_mut().set_app(app)
    }
    fn set_env(&mut self, env: String) -> Result<(), LineMetaError> {
        self.as_line_meta_mut().set_env(env)
    }
    fn set_file(&mut self, file: String) -> Result<(), LineMetaError> {
        self.as_line_meta_mut().set_file(file)
    }
    fn set_host(&mut self, host: String) -> Result<(), LineMetaError> {
        self.as_line_meta_mut().set_host(host)
    }
    fn set_labels(&mut self, labels: KeyValueMap) -> Result<(), LineMetaError> {
        self.as_line_meta_mut().set_labels(labels)
    }
    fn set_level(&mut self, level: String) -> Result<(), LineMetaError> {
        self.as_line_meta_mut().set_level(level)
    }
    fn set_meta(&mut self, meta: Value) -> Result<(), LineMetaError> {
        self.as_line_meta_mut().set_meta(meta)
    }
}

impl<T, R> LineBufferMut for StrictOrLazyLineBuilder<T, R>
where
    T: LineMeta + LineMetaMut + LineBufferMut,
{
    fn get_line_buffer(&mut self) -> Option<&[u8]> {
        use StrictOrLazyLineBuilder::*;
        let line: &mut dyn LineBufferMut = match self {
            Strict(line) => line,
            Lazy(line) => line,
        };
        line.get_line_buffer()
    }

    fn set_line_buffer(&mut self, line: Vec<u8>) -> Result<(), LineMetaError> {
        use StrictOrLazyLineBuilder::*;
        let self_line: &mut dyn LineBufferMut = match self {
            Strict(line) => line,
            Lazy(line) => line,
        };

        self_line.set_line_buffer(line)
    }
}

impl<T, R> LineMeta for RetryableLineBuilder<T, R>
where
    T: LineMeta,
{
    fn get_annotations(&self) -> Option<&KeyValueMap> {
        self.line.get_annotations()
    }
    fn get_app(&self) -> Option<&str> {
        self.line.get_app()
    }
    fn get_env(&self) -> Option<&str> {
        self.line.get_env()
    }
    fn get_file(&self) -> Option<&str> {
        self.line.get_file()
    }
    fn get_host(&self) -> Option<&str> {
        self.line.get_host()
    }
    fn get_labels(&self) -> Option<&KeyValueMap> {
        self.line.get_labels()
    }
    fn get_level(&self) -> Option<&str> {
        self.line.get_level()
    }
    fn get_meta(&self) -> Option<&Value> {
        self.line.get_meta()
    }
}

impl<T, R> LineMetaMut for RetryableLineBuilder<T, R>
where
    T: LineMeta + LineMetaMut,
{
    fn get_annotations_mut(&mut self) -> &mut Option<KeyValueMap> {
        self.line.get_annotations_mut()
    }
    fn get_app_mut(&mut self) -> &mut Option<String> {
        self.line.get_app_mut()
    }
    fn get_env_mut(&mut self) -> &mut Option<String> {
        self.line.get_env_mut()
    }
    fn get_file_mut(&mut self) -> &mut Option<String> {
        self.line.get_file_mut()
    }
    fn get_host_mut(&mut self) -> &mut Option<String> {
        self.line.get_host_mut()
    }
    fn get_labels_mut(&mut self) -> &mut Option<KeyValueMap> {
        self.line.get_labels_mut()
    }
    fn get_level_mut(&mut self) -> &mut Option<String> {
        self.line.get_level_mut()
    }
    fn get_meta_mut(&mut self) -> &mut Option<Value> {
        self.line.get_meta_mut()
    }
    fn set_annotations(&mut self, annotations: KeyValueMap) -> Result<(), LineMetaError> {
        self.line.set_annotations(annotations)
    }
    fn set_app(&mut self, app: String) -> Result<(), LineMetaError> {
        self.line.set_app(app)
    }
    fn set_env(&mut self, env: String) -> Result<(), LineMetaError> {
        self.line.set_env(env)
    }
    fn set_file(&mut self, file: String) -> Result<(), LineMetaError> {
        self.line.set_file(file)
    }
    fn set_host(&mut self, host: String) -> Result<(), LineMetaError> {
        self.line.set_host(host)
    }
    fn set_labels(&mut self, labels: KeyValueMap) -> Result<(), LineMetaError> {
        self.line.set_labels(labels)
    }
    fn set_level(&mut self, level: String) -> Result<(), LineMetaError> {
        self.line.set_level(level)
    }
    fn set_meta(&mut self, meta: Value) -> Result<(), LineMetaError> {
        self.line.set_meta(meta)
    }
}

impl<T, R> LineBufferMut for RetryableLineBuilder<T, R>
where
    T: LineMeta + LineMetaMut + LineBufferMut,
{
    fn get_line_buffer(&mut self) -> Option<&[u8]> {
        self.line.get_line_buffer()
    }

    fn set_line_buffer(&mut self, line: Vec<u8>) -> Result<(), LineMetaError> {
        self.line.set_line_buffer(line)
    }
}

impl<T, R> RetryableLine for RetryableLineBuilder<T, R>
where
    R: SourceRetry<RetryableLine = T>,
{
    fn retry(&self, delay: Option<std::time::Duration>) -> Result<(), SourceError> {
        self.retryer
            .as_ref()
            .map(|retryer| {
                retryer.retry(
                    &self.line,
                    delay.unwrap_or(std::time::Duration::from_secs(0)),
                )
            })
            .unwrap_or(Ok(()))
    }

    fn commit(&self) -> Result<(), SourceError> {
        self.retryer
            .as_ref()
            .map(|retryer| retryer.commit(&self.line))
            .unwrap_or(Ok(()))
    }

    fn retry_at(&self) -> time::OffsetDateTime {
        // TODO: More complex logic for non-file lines?
        time::OffsetDateTime::now_utc()
    }
    fn retry_after(&self) -> std::time::Duration {
        // TODO: More complex logic for non-file lines?
        std::time::Duration::from_secs(0)
    }
    fn retries_remaining(&self) -> u32 {
        // TODO: More complex logic for non-file lines?
        0
    }
}

impl<T, R> RetryableLine for StrictOrLazyLineBuilder<T, R>
where
    R: SourceRetry<RetryableLine = T>,
{
    fn retry(&self, delay: Option<std::time::Duration>) -> Result<(), SourceError> {
        use StrictOrLazyLineBuilder::*;
        match self {
            Strict(line) => line.retry(delay),
            Lazy(line) => line.retry(delay),
        }
    }

    fn commit(&self) -> Result<(), SourceError> {
        use StrictOrLazyLineBuilder::*;
        match self {
            Strict(line) => line.commit(),
            Lazy(line) => line.commit(),
        }
    }

    fn retry_at(&self) -> time::OffsetDateTime {
        use StrictOrLazyLineBuilder::*;
        match self {
            Strict(line) => line.retry_at(),
            Lazy(line) => line.retry_at(),
        }
    }
    fn retry_after(&self) -> std::time::Duration {
        use StrictOrLazyLineBuilder::*;
        match self {
            Strict(line) => line.retry_after(),
            Lazy(line) => line.retry_after(),
        }
    }

    fn retries_remaining(&self) -> u32 {
        use StrictOrLazyLineBuilder::*;
        match self {
            Strict(line) => line.retries_remaining(),
            Lazy(line) => line.retries_remaining(),
        }
    }
}

use core::pin::Pin;
use futures::stream::{Fuse, FusedStream, Stream};
use futures::task::{Context, Poll};
use futures::Future;
use futures::StreamExt;

#[cfg(feature = "sink")]
use futures_sink::Sink;

use futures_timer::Delay;
use std::collections::HashMap;
use std::time::Duration;

use crate::types::body::IngestBodyBuffer;

use crate::types::serialize::{
    body_serializer_source, IngestBodySerializer, IngestLineSerialize, IngestLineSerializeError,
};

use pin_project_lite::pin_project;

use thiserror::Error;

/// A Stream extension trait allowing you to call `timed_request_batches` on any `Stream`
/// of objects implementing IngestLineSerialize
pub trait TimedRequestBatcherStreamExt: Stream {
    fn timed_request_batches<'a>(
        self,
        capacity: usize,
        duration: Duration,
    ) -> TimedRequestBatcher<'a, Self>
    where
        Self::Item: IngestLineSerialize<String, bytes::Bytes, HashMap<String, String>, Ok = ()>
            + std::marker::Send
            + std::marker::Sync,
        Self: Sized,
        <Self as futures::Stream>::Item: 'a
            + IngestLineSerialize<
                std::string::String,
                bytes::Bytes,
                HashMap<std::string::String, std::string::String>,
                Ok = (),
            >,
    {
        TimedRequestBatcher::<'a, Self>::new(self, capacity, duration)
    }
}

impl<T: ?Sized> TimedRequestBatcherStreamExt for T where T: Stream {}

pub type BufferSource = Pin<
    Box<
        dyn Stream<Item = Result<IngestBodySerializer, IngestLineSerializeError>>
            + std::marker::Send
            + std::marker::Sync,
    >,
>;

pub type DynWriteFut<'a> = dyn Future<Output = Result<IngestBodySerializer, IngestLineSerializeError>>
    + std::marker::Send
    + 'a;

pub type WriteFut<'a> = Pin<Box<DynWriteFut<'a>>>;

type PollResult = Poll<Option<Result<IngestBodyBuffer, TimedRequestBatcherError>>>;

#[derive(Debug, Error)]
pub enum TimedRequestBatcherError {
    #[error("{0}")]
    BufferError(#[from] IngestLineSerializeError),
    #[error("No buffers available from buffer source")]
    BufferStreamError,
}

pin_project! {
    /// A Stream of batchs.
    #[must_use = "streams do nothing unless polled"]
    pub struct TimedRequestBatcher<'a, St: Stream> {
        #[pin]
        stream: Fuse<St>,
        current: Option<IngestBodySerializer>,
        cap: usize,
        #[pin]
        timer: Option<Delay>,
        duration: Duration,
        buffer_source: BufferSource,
        write_fut: Option<WriteFut<'a>>,
    }
}

impl<'a, St: Stream> TimedRequestBatcher<'a, St>
where
    St::Item: IngestLineSerialize<String, bytes::Bytes, HashMap<String, String>, Ok = ()>
        + 'a
        + std::marker::Send
        + std::marker::Sync,
{
    pub fn new(stream: St, capacity: usize, duration: Duration) -> TimedRequestBatcher<'a, St> {
        assert!(capacity > 0);

        let buffer_source = Box::pin(body_serializer_source(
            16 * 1024, /* 16 KB segments */
            50,        /* 16KB * 50 = 256 KB initial capacity */
            None,      /* No max size */
            Some(100), /* max 512KB idle buffers */
        ));

        TimedRequestBatcher {
            stream: stream.fuse(),
            current: None,
            cap: capacity,
            timer: None,
            duration,
            buffer_source,
            write_fut: None,
        }
    }

    /// Acquires a reference to the underlying stream that this combinator is
    /// pulling from.
    pub fn get_ref(&self) -> &St {
        self.stream.get_ref()
    }

    /// Acquires a mutable reference to the underlying stream that this
    /// combinator is pulling from.
    ///
    /// Note that care must be taken to avoid tampering with the state of the
    /// stream which may otherwise confuse this combinator.
    pub fn get_mut(&mut self) -> &mut St {
        self.stream.get_mut()
    }

    /// Acquires a pinned mutable reference to the underlying stream that this
    /// combinator is pulling from.
    ///
    /// Note that care must be taken to avoid tampering with the state of the
    /// stream which may otherwise confuse this combinator.
    pub fn get_pin_mut(self: Pin<&mut Self>) -> Pin<&mut St> {
        let this = self.project();
        this.stream.get_pin_mut()
    }

    /// Consumes this combinator, returning the underlying stream.
    ///
    /// Note that this may discard intermediate state of this combinator, so
    /// care should be taken to avoid losing resources when this is called.
    pub fn into_inner(self) -> St {
        self.stream.into_inner()
    }

    fn poll_write_fut(
        write_fut: &mut Option<WriteFut>,
        current: &mut Option<IngestBodySerializer>,
        cx: &mut Context<'_>,
    ) -> Option<PollResult> {
        if write_fut.is_some() {
            let fut = write_fut.as_mut().unwrap().as_mut();
            match fut.poll(cx) {
                Poll::Ready(Ok(ser)) => {
                    // yay, done writing
                    *current = Some(ser);
                }
                Poll::Ready(Err(e)) => return Some(Poll::Ready(Some(Err(e.into())))),
                Poll::Pending => return Some(Poll::Pending),
            }
            *write_fut = None;
        }
        None
    }

    fn poll_populate_buffer(
        current: &mut Option<IngestBodySerializer>,
        buffer_source: &mut BufferSource,
        cx: &mut Context<'_>,
    ) -> Option<PollResult> {
        // Ensure we have a buffer to write to
        if current.is_none() {
            // Populate this.current
            match buffer_source.as_mut().poll_next(cx) {
                Poll::Ready(buf_res) => match buf_res {
                    Some(Ok(buf)) => {
                        *current = Some(buf);
                    }
                    Some(Err(e)) => return Some(Poll::Ready(Some(Err(e.into())))),
                    None => {
                        return Some(Poll::Ready(Some(Err(
                            TimedRequestBatcherError::BufferStreamError,
                        ))))
                    }
                },
                Poll::Pending => {
                    return Some(Poll::Pending);
                }
            };
        }
        None
    }

    fn poll_check_capacity(
        current_ptr: &mut Option<IngestBodySerializer>,
        timer: &mut Option<Delay>,
        cap: usize,
    ) -> Option<PollResult> {
        // Check if we've filled our buffer
        let current = current_ptr.as_mut().unwrap();
        if current.count() > 0 && current.bytes_len() >= cap {
            // Stop our timer and return our current buffer
            *timer = None;
            let ret = current_ptr.take().unwrap();

            match ret.end() {
                Ok(body) => {
                    return Some(Poll::Ready(Some(Ok(IngestBodyBuffer::from_buffer(body)))))
                }
                Err(e) => return Some(Poll::Ready(Some(Err(e.into())))),
            }
        }
        None
    }

    fn poll_process_stream(
        line_stream: Pin<&mut futures::stream::Fuse<St>>,
        current: &mut Option<IngestBodySerializer>,
        timer: &mut Option<Delay>,
        duration: Duration,
        cx: &mut Context<'_>,
    ) -> (Option<WriteFut<'a>>, Option<PollResult>) {
        match line_stream.poll_next(cx) {
            Poll::Ready(item) => match item {
                // Push the iterm from the underlying stream onto our buffer
                Some(item) => {
                    if timer.is_none() {
                        // Note this means we delay restarting the timer until we have a buffer to write to
                        *timer = Some(Delay::new(duration));
                    }

                    // Safe as we can't get this far through the loop without a buffer ready
                    let mut current = current.take().unwrap();
                    let fut = async move {
                        let res = current.write_line(item).await;
                        if let Err(e) = res {
                            Err(e)
                        } else {
                            Ok(current)
                        }
                    };
                    (Some(Box::pin(fut)), None)
                }

                // Since the underlying stream ran out of values, return what we
                // have buffered, if we have anything.
                None => {
                    if current.is_none() {
                        (None, Some(Poll::Ready(None)))
                    } else {
                        let full_buf = current.take();
                        if full_buf.as_ref().unwrap().count() == 0 {
                            (None, Some(Poll::Ready(None)))
                        } else {
                            (
                                None,
                                Some(Poll::Ready(full_buf.map(|r| {
                                    r.end()
                                        .map(IngestBodyBuffer::from_buffer)
                                        .map_err(|e| e.into())
                                }))),
                            )
                        }
                    }
                }
            },
            // Don't return here, as we need to need check the timer.
            Poll::Pending => (None, None),
        }
    }

    fn poll_check_timer(
        current_ptr: &mut Option<IngestBodySerializer>,
        mut timer: Pin<&mut Option<Delay>>,
        cx: &mut Context<'_>,
    ) -> Option<PollResult> {
        let res = timer.as_mut().as_pin_mut().map(|timer| timer.poll(cx));
        match res {
            Some(Poll::Ready(())) => {
                *timer.get_mut() = None;
                let ret = current_ptr.take();

                Some(Poll::Ready(ret.map(|r| {
                    r.end()
                        .map(IngestBodyBuffer::from_buffer)
                        .map_err(|e| e.into())
                })))
            }
            Some(Poll::Pending) => Some(Poll::Pending),
            None => None,
        }
    }
}

impl<'a, St: Stream> Stream for TimedRequestBatcher<'a, St>
where
    St::Item: IngestLineSerialize<String, bytes::Bytes, HashMap<String, String>, Ok = ()>
        + 'a
        + std::marker::Send
        + std::marker::Sync,
{
    type Item = Result<IngestBodyBuffer, TimedRequestBatcherError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut this = self.project();

        loop {
            // If we're writing a line keep going
            if let Some(poll) = Self::poll_write_fut(this.write_fut, this.current, cx) {
                return poll;
            };

            debug_assert!(this.write_fut.is_none(), "we should be writing");

            // Ensure we have a buffer to write to
            if let Some(poll) = Self::poll_populate_buffer(this.current, this.buffer_source, cx) {
                return poll;
            }

            debug_assert!(this.current.is_some(), "current is missing");

            // If we've gotten this far then this.current is some,
            if let Some(poll) =
                Self::poll_check_capacity(this.current, this.timer.as_mut().get_mut(), *this.cap)
            {
                return poll;
            }

            debug_assert!(this.current.is_some(), "current is missing");

            // Check if there is anything on the underlying stream
            let stream: Pin<&mut futures::stream::Fuse<St>> = this.stream.as_mut();

            match Self::poll_process_stream(
                stream,
                this.current,
                this.timer.as_mut().get_mut(),
                *this.duration,
                cx,
            ) {
                // Got something and in process of writing to buffer
                (Some(wf), None) => {
                    *this.write_fut = Some(wf);
                    continue;
                }
                // End of Stream, returning last
                (None, Some(poll)) => return poll,
                // Nothing from the stream
                _ => {}
            }

            debug_assert!(this.current.is_some(), "current is missing");

            // Finally check if our timeout has expired
            if let Some(poll) = Self::poll_check_timer(this.current, this.timer.as_mut(), cx) {
                return poll;
            }

            return Poll::Pending;
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let batch_len = if self.current.is_none() { 0 } else { 1 };
        let (lower, upper) = self.stream.size_hint();
        let lower = lower.saturating_add(batch_len);
        let upper = match upper {
            Some(x) => x.checked_add(batch_len),
            None => None,
        };
        (lower, upper)
    }
}

impl<'a, St: FusedStream> FusedStream for TimedRequestBatcher<'a, St>
where
    St::Item: IngestLineSerialize<String, bytes::Bytes, HashMap<String, String>, Ok = ()>
        + 'a
        + std::marker::Send
        + std::marker::Sync,
{
    fn is_terminated(&self) -> bool {
        self.stream.is_terminated() & self.current.is_none()
    }
}

impl<S, Item> Sink<Item> for TimedRequestBatcher<S>
where
    S: Stream + Sink<Item>,
{
    type Error = S::Error;

    delegate_sink!(stream, Item);
}

#[cfg(test)]
mod tests {

    use super::*;

    use std::io::Read;
    use std::time::{Duration, Instant};

    use crate::types::body::{KeyValueMap, Line};

    use futures::{stream, FutureExt, StreamExt};

    use proptest::collection::hash_map;
    use proptest::option::of;
    use proptest::prelude::*;
    use proptest::string::string_regex;

    pub fn key_value_map_st(max_entries: usize) -> impl Strategy<Value = KeyValueMap> {
        hash_map(
            string_regex(".{1,64}").unwrap(),
            string_regex(".{1,64}").unwrap(),
            0..max_entries,
        )
        .prop_map(move |c| {
            let mut kv_map = KeyValueMap::new();
            for (k, v) in c.into_iter() {
                kv_map = kv_map.add(k, v);
            }
            kv_map
        })
    }

    //recursive JSON type
    pub fn json_st(depth: u32) -> impl Strategy<Value = serde_json::Value> {
        let leaf = prop_oneof![
            Just(serde_json::Value::Null),
            any::<bool>().prop_map(|o| serde_json::to_value(o).unwrap()),
            any::<f64>().prop_map(|o| serde_json::to_value(o).unwrap()),
            ".{1,64}".prop_map(|o| serde_json::to_value(o).unwrap()),
        ];
        leaf.prop_recursive(depth, 256, 10, |inner| {
            prop_oneof![
                prop::collection::vec(inner.clone(), 0..10)
                    .prop_map(|o| serde_json::to_value(o).unwrap()),
                prop::collection::hash_map(".*", inner, 0..10)
                    .prop_map(|o| serde_json::to_value(o).unwrap()),
            ]
        })
    }
    pub fn line_st() -> impl Strategy<Value = Line> {
        (
            of(key_value_map_st(5)),
            of(string_regex(".{1,64}").unwrap()),
            of(string_regex(".{1,64}").unwrap()),
            of(string_regex(".{1,64}").unwrap()),
            of(string_regex(".{1,64}").unwrap()),
            of(key_value_map_st(5)),
            of(string_regex(".{1,64}").unwrap()),
            of(json_st(3)),
            string_regex(".{1,64}").unwrap(),
            (0..i64::MAX),
        )
            .prop_map(
                |(annotations, app, env, file, host, labels, level, meta, line, timestamp)| Line {
                    annotations,
                    app,
                    env,
                    file,
                    host,
                    labels,
                    level,
                    meta,
                    line,
                    timestamp,
                },
            )
    }

    #[tokio::test]
    async fn messages_pass_through() {
        let input = vec![
            Line::builder().line("0".to_string()).build().unwrap(),
            Line::builder().line("1".to_string()).build().unwrap(),
            Line::builder().line("2".to_string()).build().unwrap(),
            Line::builder().line("3".to_string()).build().unwrap(),
            Line::builder().line("4".to_string()).build().unwrap(),
            Line::builder().line("5".to_string()).build().unwrap(),
            Line::builder().line("6".to_string()).build().unwrap(),
            Line::builder().line("7".to_string()).build().unwrap(),
            Line::builder().line("8".to_string()).build().unwrap(),
            Line::builder().line("9".to_string()).build().unwrap(),
        ];
        let results = stream::iter(input.iter())
            .timed_request_batches(350, Duration::new(1, 0))
            .collect::<Vec<_>>()
            .await;

        let mut buf = String::new();
        results[0]
            .as_ref()
            .unwrap()
            .reader()
            .read_to_string(&mut buf)
            .unwrap();

        let mut body: HashMap<String, Vec<Line>> = serde_json::from_str(&buf).unwrap();
        let lines = body.remove("lines").unwrap_or(vec![]);

        assert_eq!(input, lines);
    }

    #[tokio::test]
    async fn message_batchs() {
        let input = vec![
            Line::builder().line("0".to_string()).build().unwrap(),
            Line::builder().line("1".to_string()).build().unwrap(),
            Line::builder().line("2".to_string()).build().unwrap(),
            Line::builder().line("3".to_string()).build().unwrap(),
            Line::builder().line("4".to_string()).build().unwrap(),
            Line::builder().line("5".to_string()).build().unwrap(),
            Line::builder().line("6".to_string()).build().unwrap(),
            Line::builder().line("7".to_string()).build().unwrap(),
            Line::builder().line("8".to_string()).build().unwrap(),
            Line::builder().line("9".to_string()).build().unwrap(),
        ];

        let stream = stream::iter(input.iter());
        let batch_stream = TimedRequestBatcher::new(stream, 200, Duration::new(0, 250));
        let result = batch_stream.collect::<Vec<_>>().await;

        let mut buf = String::new();
        result[0]
            .as_ref()
            .unwrap()
            .reader()
            .read_to_string(&mut buf)
            .unwrap();
        let mut body: HashMap<String, Vec<Line>> = serde_json::from_str(&buf).unwrap();
        let lines0 = body.remove("lines").unwrap_or(vec![]);

        buf.clear();
        result[1]
            .as_ref()
            .unwrap()
            .reader()
            .read_to_string(&mut buf)
            .unwrap();
        let mut body: HashMap<String, Vec<Line>> = serde_json::from_str(&buf).unwrap();
        let lines1 = body.remove("lines").unwrap_or(vec![]);

        assert_eq!(input[..6], lines0);
        assert_eq!(input[6..], lines1);
    }

    #[tokio::test]
    async fn message_timeout() {
        let input0 = vec![
            Line::builder().line("0".to_string()).build().unwrap(),
            Line::builder().line("1".to_string()).build().unwrap(),
            Line::builder().line("2".to_string()).build().unwrap(),
            Line::builder().line("3".to_string()).build().unwrap(),
        ];

        let stream0 = stream::iter(input0.iter());

        let input1 = vec![Line::builder().line("4".to_string()).build().unwrap()];
        let stream1 = stream::iter(input1.iter())
            .then(move |n| Delay::new(Duration::from_millis(300)).map(move |_| n));

        let input2 = vec![
            Line::builder().line("5".to_string()).build().unwrap(),
            Line::builder().line("6".to_string()).build().unwrap(),
            Line::builder().line("7".to_string()).build().unwrap(),
            Line::builder().line("8".to_string()).build().unwrap(),
            Line::builder().line("9".to_string()).build().unwrap(),
        ];
        let stream2 = stream::iter(input2.iter());

        let stream = stream0.chain(stream1).chain(stream2);
        let batch_stream = TimedRequestBatcher::new(stream, 5000, Duration::from_millis(100));

        let now = Instant::now();
        let min_times = [Duration::from_millis(80), Duration::from_millis(150)];
        let max_times = [Duration::from_millis(350), Duration::from_millis(500)];

        let mut i = 0;

        let results = batch_stream
            .map(move |s| {
                let now2 = Instant::now();
                assert!((now2 - now) < max_times[i]);
                assert!((now2 - now) > min_times[i]);
                i += 1;
                s
            })
            .collect::<Vec<_>>()
            .await;

        let mut buf = String::new();
        results[0]
            .as_ref()
            .unwrap()
            .reader()
            .read_to_string(&mut buf)
            .unwrap();
        let mut body: HashMap<String, Vec<Line>> = serde_json::from_str(&buf).unwrap();
        let lines0 = body.remove("lines").unwrap_or(vec![]);
        assert_eq!(lines0, input0);

        let mut expected = input1.clone();
        expected.extend_from_slice(&input2[..]);

        buf.clear();
        results[1]
            .as_ref()
            .unwrap()
            .reader()
            .read_to_string(&mut buf)
            .unwrap();
        let mut body: HashMap<String, Vec<Line>> = serde_json::from_str(&buf).unwrap();
        let lines1 = body.remove("lines").unwrap_or(vec![]);
        assert_eq!(lines1, expected);
    }

    proptest! {
        #[test]
        fn roundtrip(
            inp in (0..1024usize)
                .prop_flat_map(|size|(Just(size),
                                      proptest::collection::vec(line_st(), size)))) {

            let (size, lines) = inp;
            let results =
                tokio_test::block_on(async {
                    let batch_stream = stream::iter(lines.iter()).timed_request_batches(5_000, Duration::new(1, 0));
                    batch_stream.collect::<Vec<_>>().await
                });

            let all_results: Vec<_> = results.into_iter().map(move |body|{
                let mut buf = String::new();
                body.as_ref()
                    .unwrap()
                    .reader()
                    .read_to_string(&mut buf)
                    .unwrap();
                let mut body: HashMap<String, Vec<Line>> = serde_json::from_str(&buf).unwrap();
                body.remove("lines").unwrap_or(vec![])
            })
                .into_iter()
                .flatten()
                .collect();

            assert_eq!(all_results.len(), size);
        }
    }
}

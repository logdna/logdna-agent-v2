use core::pin::Pin;

use std::collections::HashMap;
use std::time::Duration;

use futures::stream::{self, Fuse, Stream};
use futures::task::{Context, Poll};
use futures::StreamExt;

use thiserror::Error;
use tokio::time;

use state::GetOffset;

use crate::offsets::OffsetMap;
use crate::types::body::IngestBodyBuffer;
use crate::types::serialize::{
    body_serializer_source, IngestBodySerializer, IngestLineSerialize, IngestLineSerializeError,
};

/// A Stream extension trait allowing you to call `timed_request_batches` on any `Stream`
/// of objects implementing IngestLineSerialize
pub trait TimedRequestBatcherStreamExt: Stream {
    fn timed_request_batches<'a>(
        self,
        capacity: usize,
        duration: Duration,
    ) -> TimedRequestBatchStream<'a>
    where
        Self::Item: IngestLineSerialize<String, bytes::Bytes, HashMap<String, String>, Ok = ()>
            + GetOffset
            + std::marker::Send
            + std::marker::Sync
            + 'a,
        Self: Sized + 'a,
    {
        timed_request_batch_stream(self, capacity, duration)
    }
}

pub type BatchResult = Result<(IngestBodyBuffer, OffsetMap), TimedRequestBatcherError>;

pub struct TimedRequestBatchStream<'a> {
    stream: Pin<Box<dyn Stream<Item = BatchResult> + 'a>>,
}

impl<'a> TimedRequestBatchStream<'a> {
    fn new(stream: Pin<Box<dyn Stream<Item = BatchResult> + 'a>>) -> Self {
        Self { stream }
    }
}

impl<'a> Stream for TimedRequestBatchStream<'a> {
    type Item = BatchResult;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.stream.as_mut().poll_next(cx)
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

#[derive(Debug, Error)]
pub enum TimedRequestBatcherError {
    #[error("{0}")]
    BufferError(#[from] IngestLineSerializeError),
    #[error("No buffers available from buffer source")]
    BufferStreamError,
}

/// A Stream of batchs.
pub struct TimedRequestBatcherState<St: Stream> {
    stream: Pin<Box<Fuse<St>>>,
    cap: usize,
    duration: Duration,
    buffer_source: BufferSource,
}

impl<'a, St: Stream> TimedRequestBatcherState<St>
where
    St::Item: IngestLineSerialize<String, bytes::Bytes, HashMap<String, String>, Ok = ()>
        + GetOffset
        + std::marker::Send
        + std::marker::Sync
        + 'a,
{
    pub fn new(stream: St, capacity: usize, duration: Duration) -> TimedRequestBatcherState<St> {
        assert!(capacity > 0);

        // TODO expose
        let buffer_source = Box::pin(body_serializer_source(
            16 * 1024, /* 16 KB segments */
            50,        /* 16KB * 50 = 256 KB initial capacity */
            None,      /* No max size */
            Some(100), /* max 512KB idle buffers */
        ));

        TimedRequestBatcherState {
            stream: Box::pin(stream.fuse()),
            cap: capacity,
            duration,
            buffer_source,
        }
    }
}

pub fn timed_request_batch_stream<'a>(
    stream: impl Stream<
            Item = impl IngestLineSerialize<String, bytes::Bytes, HashMap<String, String>, Ok = ()>
                       + GetOffset
                       + std::marker::Send
                       + std::marker::Sync
                       + 'a,
        > + 'a,
    capacity: usize,
    duration: Duration,
) -> TimedRequestBatchStream<'a> {
    TimedRequestBatchStream::new(Box::pin(stream::unfold(
        TimedRequestBatcherState::new(stream, capacity, duration),
        |mut state| async move {
            // Get a new buffer
            let buffer = state.buffer_source.next().await;
            let mut current: IngestBodySerializer = match buffer {
                Some(buffer) => match buffer {
                    Ok(buffer) => buffer,
                    Err(e) => return Some((Err(e.into()), state)),
                },
                None => return Some((Err(TimedRequestBatcherError::BufferStreamError), state)),
            };

            let mut offsets: OffsetMap = OffsetMap::default();

            // Set the initial timeout
            let timeout = time::sleep(state.duration);
            tokio::pin!(timeout);

            loop {
                tokio::select! {
                    item = state.stream.next() => {
                        match item {
                            Some(item) => {
                                if let (Some(key), Some(offset)) = (item.get_key(), item.get_offset()) {
                                    let _ = offsets.insert(key, offset);
                                }

                                // Write line to buffer
                                if let Err(e) = current.write_line(item).await {
                                    return Some((Err(e.into()), state));
                                };

                                // check if we've passed capacity
                                if current.count() > 0 && current.bytes_len() >= state.cap {
                                    // Stop our timer and return our current buffer

                                    return match current.end() {
                                        Ok(body) => Some((
                                            Ok((IngestBodyBuffer::from_buffer(body), offsets)),
                                            state,
                                        )),
                                        Err(e) => Some((Err(e.into()), state)),
                                    };
                                }
                            }
                            None => {
                                // Underlying stream is done. Return the buffer if we have any data
                                return if current.count() > 0 {
                                    match current.end() {
                                        Ok(body) => Some((
                                            Ok((IngestBodyBuffer::from_buffer(body), offsets)),
                                            state,
                                        )),
                                        Err(e) => Some((Err(e.into()), state)),
                                    }
                                } else {
                                    None
                                };
                            }
                        }
                    },
                    _ = &mut timeout => {
                        // Timed out. If we have a buffer return it
                        if current.count() > 0 {
                            return match current.end() {
                                Ok(body) => Some((
                                    Ok((IngestBodyBuffer::from_buffer(body), offsets)),
                                    state,
                                )),
                                Err(e) => Some((Err(e.into()), state)),
                            };
                        }
                        // No logs, reset timeout and keep waiting
                        timeout.as_mut().reset(tokio::time::Instant::now() + state.duration);
                    }
                }
            }
        },
    )))
}

#[cfg(test)]
mod tests {

    use super::*;

    use std::collections::HashMap;
    use std::io::Read;
    use std::time::{Duration, Instant};

    use futures::stream;
    use futures::FutureExt;

    use futures_timer::Delay;

    use proptest::prelude::*;

    use crate::types::body::Line;
    use test_types::strategies::{line_st, offset_st, OffsetLine};

    #[tokio::test]
    async fn messages_pass_through() {
        let input = vec![
            OffsetLine::new(Line::builder().line("0".to_string()).build().unwrap(), None),
            OffsetLine::new(Line::builder().line("1".to_string()).build().unwrap(), None),
            OffsetLine::new(Line::builder().line("2".to_string()).build().unwrap(), None),
            OffsetLine::new(Line::builder().line("3".to_string()).build().unwrap(), None),
            OffsetLine::new(Line::builder().line("4".to_string()).build().unwrap(), None),
            OffsetLine::new(Line::builder().line("5".to_string()).build().unwrap(), None),
            OffsetLine::new(Line::builder().line("6".to_string()).build().unwrap(), None),
            OffsetLine::new(Line::builder().line("7".to_string()).build().unwrap(), None),
            OffsetLine::new(Line::builder().line("8".to_string()).build().unwrap(), None),
            OffsetLine::new(Line::builder().line("9".to_string()).build().unwrap(), None),
        ];
        let results = stream::iter(input.iter())
            .timed_request_batches(350, Duration::new(1, 0))
            .collect::<Vec<_>>()
            .await;

        let mut buf = String::new();
        results[0]
            .as_ref()
            .unwrap()
            .0
            .reader()
            .read_to_string(&mut buf)
            .unwrap();

        let mut body: HashMap<String, Vec<Line>> = serde_json::from_str(&buf).unwrap();
        let lines: Vec<OffsetLine> = body
            .remove("lines")
            .unwrap_or_default()
            .into_iter()
            .map(|l| OffsetLine::new(l, None))
            .collect();

        assert_eq!(input, lines);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 3)]
    async fn message_batchs() {
        let input = vec![
            OffsetLine::new(Line::builder().line("0".to_string()).build().unwrap(), None),
            OffsetLine::new(Line::builder().line("1".to_string()).build().unwrap(), None),
            OffsetLine::new(Line::builder().line("2".to_string()).build().unwrap(), None),
            OffsetLine::new(Line::builder().line("3".to_string()).build().unwrap(), None),
            OffsetLine::new(Line::builder().line("4".to_string()).build().unwrap(), None),
            OffsetLine::new(Line::builder().line("5".to_string()).build().unwrap(), None),
            OffsetLine::new(Line::builder().line("6".to_string()).build().unwrap(), None),
            OffsetLine::new(Line::builder().line("7".to_string()).build().unwrap(), None),
            OffsetLine::new(Line::builder().line("8".to_string()).build().unwrap(), None),
            OffsetLine::new(Line::builder().line("9".to_string()).build().unwrap(), None),
        ];

        let stream = stream::iter(input.iter());
        let batch_stream = stream.timed_request_batches(200, Duration::new(0, 250));
        let result = batch_stream.collect::<Vec<_>>().await;

        let mut buf = String::new();
        result[0]
            .as_ref()
            .unwrap()
            .0
            .reader()
            .read_to_string(&mut buf)
            .unwrap();
        let mut body: HashMap<String, Vec<Line>> = serde_json::from_str(&buf).unwrap();
        let lines0: Vec<OffsetLine> = body
            .remove("lines")
            .unwrap_or_default()
            .into_iter()
            .map(|l| OffsetLine::new(l, None))
            .collect();

        buf.clear();
        result[1]
            .as_ref()
            .unwrap()
            .0
            .reader()
            .read_to_string(&mut buf)
            .unwrap();
        let mut body: HashMap<String, Vec<Line>> = serde_json::from_str(&buf).unwrap();
        let lines1: Vec<OffsetLine> = body
            .remove("lines")
            .unwrap_or_default()
            .into_iter()
            .map(|l| OffsetLine::new(l, None))
            .collect();

        assert_eq!(input[..6], lines0);
        assert_eq!(input[6..], lines1);
    }

    #[tokio::test]
    async fn message_timeout() {
        let input0 = vec![
            OffsetLine::new(Line::builder().line("0".to_string()).build().unwrap(), None),
            OffsetLine::new(Line::builder().line("1".to_string()).build().unwrap(), None),
            OffsetLine::new(Line::builder().line("2".to_string()).build().unwrap(), None),
            OffsetLine::new(Line::builder().line("3".to_string()).build().unwrap(), None),
        ];

        let stream0 = stream::iter(input0.iter());

        let input1 = vec![OffsetLine::new(
            Line::builder().line("4".to_string()).build().unwrap(),
            None,
        )];
        let stream1 = stream::iter(input1.iter())
            .then(move |n| Delay::new(Duration::from_millis(300)).map(move |_| n));

        let input2 = vec![
            OffsetLine::new(Line::builder().line("5".to_string()).build().unwrap(), None),
            OffsetLine::new(Line::builder().line("6".to_string()).build().unwrap(), None),
            OffsetLine::new(Line::builder().line("7".to_string()).build().unwrap(), None),
            OffsetLine::new(Line::builder().line("8".to_string()).build().unwrap(), None),
            OffsetLine::new(Line::builder().line("9".to_string()).build().unwrap(), None),
        ];
        let stream2 = stream::iter(input2.iter());

        let stream = stream0.chain(stream1).chain(stream2);

        let batch_stream = stream.timed_request_batches(5000, Duration::from_millis(100));

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
            .0
            .reader()
            .read_to_string(&mut buf)
            .unwrap();
        let mut body: HashMap<String, Vec<Line>> = serde_json::from_str(&buf).unwrap();
        let lines0: Vec<OffsetLine> = body
            .remove("lines")
            .unwrap_or_default()
            .into_iter()
            .map(|l| OffsetLine::new(l, None))
            .collect();
        assert_eq!(
            input0.len(),
            lines0.len(),
            "input {:#?}\nresult {:#?}",
            input0,
            lines0
        );
        assert_eq!(input0, lines0);

        let mut expected = input1.clone();
        expected.extend_from_slice(&input2[..]);

        buf.clear();
        results[1]
            .as_ref()
            .unwrap()
            .0
            .reader()
            .read_to_string(&mut buf)
            .unwrap();
        let mut body: HashMap<String, Vec<Line>> = serde_json::from_str(&buf).unwrap();
        let lines1: Vec<OffsetLine> = body
            .remove("lines")
            .unwrap_or_default()
            .into_iter()
            .map(|l| OffsetLine::new(l, None))
            .collect();

        assert_eq!(expected, lines1);
    }

    proptest! {
        #![proptest_config(ProptestConfig {
          cases: 10, .. ProptestConfig::default()
        })]

        #[test]
        fn roundtrip(
            inp in (0..1024usize)
                .prop_flat_map(|size|(Just(size),
                                      proptest::collection::vec(line_st(offset_st(2048)), size)))) {

            let (size, lines) = inp;
            let results =
                tokio_test::block_on(async {
                    let batch_stream = stream::iter(lines.iter()).timed_request_batches(5_000, Duration::new(1, 0));
                    batch_stream.collect::<Vec<_>>().await
                });

            let all_results = results.into_iter().map(move |body|{
                let mut buf = String::new();
                body.as_ref()
                    .unwrap()
                    .0
                    .reader()
                    .read_to_string(&mut buf)
                    .unwrap();
                let mut body: HashMap<String, Vec<Line>> = serde_json::from_str(&buf).unwrap();
                body.remove("lines").unwrap_or_default()
            })
                .into_iter()
                .flatten();

            assert_eq!(all_results.count(), size);
        }
    }
}

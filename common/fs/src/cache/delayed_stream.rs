use std::cmp::{Ordering, Reverse};
use std::collections::BinaryHeap;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use futures::stream::ReadyChunks;
use futures::stream::{self, Stream, StreamExt};

use pin_project_lite::pin_project;
use tokio::time::timeout_at;

pin_project! {
    #[must_use = "streams do nothing unless polled"]
    struct ReadyChunksEager<St: Stream> {
        #[pin]
        stream: ReadyChunks<St>,
        should_early_return: bool
    }
}

impl<St> std::convert::From<ReadyChunks<St>> for ReadyChunksEager<St>
where
    St: Stream,
{
    fn from(stream: ReadyChunks<St>) -> Self {
        ReadyChunksEager {
            stream,
            should_early_return: true,
        }
    }
}

impl<St: Stream> Stream for ReadyChunksEager<St> {
    type Item = Option<Vec<St::Item>>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut this = self.project();
        match this.stream.as_mut().poll_next(cx) {
            // If this is the first time the stream has been polled or if the
            // we have previously returned data we should return Some(None) to
            // signal that we don't have any data available and need to wait
            // for the underlying stream
            Poll::Pending => {
                if std::mem::replace(this.should_early_return, false) {
                    Poll::Ready(Some(None))
                } else {
                    Poll::Pending
                }
            }

            Poll::Ready(Some(item)) => {
                // We successfullly got data we should early return on the next poll
                *this.should_early_return = true;
                Poll::Ready(Some(Some(item)))
            }

            Poll::Ready(None) => Poll::Ready(None),
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.stream.size_hint()
    }
}

trait ReadyChunksEagerExt<St: Stream> {
    fn eager(self) -> ReadyChunksEager<St>;
}

impl<St> ReadyChunksEagerExt<St> for ReadyChunks<St>
where
    St: Stream,
{
    fn eager(self: ReadyChunks<St>) -> ReadyChunksEager<St> {
        ReadyChunksEager::from(self)
    }
}

struct DelayedEvent<T> {
    delayed_until: std::time::Instant,
    event: T,
}

impl<T> PartialEq for DelayedEvent<T> {
    fn eq(&self, other: &DelayedEvent<T>) -> bool {
        self.delayed_until == other.delayed_until
    }
}

impl<T> Eq for DelayedEvent<T> {}

impl<T> PartialOrd for DelayedEvent<T> {
    fn partial_cmp(&self, other: &DelayedEvent<T>) -> Option<Ordering> {
        self.delayed_until.partial_cmp(&other.delayed_until)
    }
}

impl<T> Ord for DelayedEvent<T> {
    fn cmp(&self, other: &DelayedEvent<T>) -> Ordering {
        self.delayed_until.cmp(&other.delayed_until)
    }
}

impl<T> From<(Instant, T)> for DelayedEvent<T> {
    fn from(args: (Instant, T)) -> DelayedEvent<T> {
        DelayedEvent {
            delayed_until: args.0,
            event: args.1,
        }
    }
}

pub fn delayed_stream<T>(
    st: impl Stream<Item = T> + Unpin,
    delay: Duration,
) -> impl Stream<Item = T> {
    stream::unfold((st.ready_chunks(100).eager(), BinaryHeap::new(), false), {
        move |(mut st, mut delayed_events, mut stream_done)| {
            async move {
                loop {
                    if stream_done && delayed_events.is_empty() {
                        return None;
                    }

                    // If any delayed events have expired we should return those
                    // before awaiting the retry_stream
                    let mut ready_events = Vec::new();
                    let now = Instant::now();
                    while delayed_events
                        .peek()
                        .map_or(false, |de: &Reverse<DelayedEvent<_>>| {
                            now > de.0.delayed_until
                        })
                    {
                        ready_events.push(delayed_events.pop().unwrap().0.event);
                    }
                    // We can be productive, return the delayed event
                    if !ready_events.is_empty() {
                        return Some((
                            stream::iter(ready_events),
                            (st, delayed_events, stream_done),
                        ));
                    }

                    // Eagerly grab any retry events that are already on the retry_stream
                    loop {
                        match st.next().await {
                            Some(Some(c)) => {
                                for t in c {
                                    delayed_events.push(Reverse(DelayedEvent::from((
                                        Instant::now() + delay,
                                        t,
                                    ))));
                                }
                                // We got something, poll again
                                continue;
                            }
                            None => {
                                stream_done = false;
                            }
                            _ => {}
                        }
                        // Got nothing, break
                        break;
                    }
                    // If we have any delay events set a timeout for when the next delay
                    // expires and await in case we get any retry events in the interim
                    if let Some(Reverse(de)) = delayed_events.peek() {
                        if let Ok(Some(Some(ts))) =
                            timeout_at((de.delayed_until).into(), st.next()).await
                        {
                            for t in ts {
                                delayed_events.push(Reverse((Instant::now() + delay, t).into()));
                            }
                        }
                    }
                }
            }
        }
    })
    .map(std::future::ready)
    .buffered(1)
    .flatten()
}

#[cfg(test)]
mod test {
    use super::*;

    #[tokio::test]
    async fn test_eager_ready_chunks() {
        let (retry_events_send, retry_events_recv) = async_channel::unbounded();

        let mut eager_ready_chunks_stream = retry_events_recv.ready_chunks(100).eager();
        assert!(matches!(eager_ready_chunks_stream.next().await, Some(None)));

        retry_events_send.send(1).await.unwrap();

        assert!(matches!(
            eager_ready_chunks_stream.next().await,
            Some(Some(_))
        ));
        assert!(matches!(eager_ready_chunks_stream.next().await, Some(None)));
    }

    #[tokio::test]
    async fn test_delayed_queue() {
        let (retry_events_send, retry_events_recv) = async_channel::unbounded();

        let mut delay_queue = Box::pin(delayed_stream(retry_events_recv, Duration::from_secs(2)));

        let start = Instant::now();
        for i in 0..100 {
            retry_events_send.send(i).await.unwrap();
        }

        let _ = delay_queue.next().await;

        let next = Instant::now();
        assert!(next - start > Duration::from_secs(1));

        delay_queue.as_mut().take(50).collect::<Vec<_>>().await;
        let delta = Instant::now() - next;
        let next = Instant::now();
        assert!(delta < Duration::from_millis(3), "{:?}", delta);

        retry_events_send.send(100).await.unwrap();
        delay_queue.as_mut().take(49).collect::<Vec<_>>().await;
        let delta = Instant::now() - next;
        let next = Instant::now();
        assert!(delta < Duration::from_millis(1), "{:?}", delta);

        delay_queue.as_mut().take(1).collect::<Vec<_>>().await;
        let delta = Instant::now() - next;
        let next = Instant::now();
        assert!(delta > Duration::from_secs(1), "{:?}", delta);

        for i in 0..102 {
            retry_events_send.send(i).await.unwrap();
        }

        delay_queue.as_mut().take(10).collect::<Vec<_>>().await;
        let delta = Instant::now() - next;
        let next = Instant::now();
        assert!(delta > Duration::from_secs(1), "{:?}", delta);

        assert!(!delay_queue
            .as_mut()
            .take(1)
            .collect::<Vec<_>>()
            .await
            .is_empty());
        let delta = Instant::now() - next;

        // Test that we aren't hitting a tokio::timeout which takes somewhere in the region of 1ms
        assert!(delta < Duration::from_micros(25), "{:?}", delta);
    }
}

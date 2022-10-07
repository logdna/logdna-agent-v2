use std::cmp::{Ordering, Reverse};
use std::collections::BinaryHeap;
use std::time::{Duration, Instant};

use futures::stream::{self, Stream, StreamExt};

use tokio::time::{timeout, timeout_at};

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
    stream::unfold((st, BinaryHeap::new(), false), {
        move |(mut st, mut delayed_events, mut stream_done)| {
            async move {
                let mut return_val = None;
                loop {
                    if stream_done && delayed_events.is_empty() {
                        return None;
                    }
                    // If any delayed events have expired we should return those
                    // before awaiting the retry_stream
                    let now = Instant::now();
                    while delayed_events
                        .peek()
                        .map_or(false, |de: &Reverse<DelayedEvent<_>>| {
                            now > de.0.delayed_until
                        })
                    {
                        return_val
                            .get_or_insert_with(Vec::new)
                            .push(delayed_events.pop().unwrap().0.event);
                    }
                    // Grab any retry events that are already on the retry_stream
                    let mut st_ready_chunks = st.ready_chunks(100);
                    loop {
                        match timeout(Duration::from_millis(0), st_ready_chunks.next()).await {
                            Ok(Some(c)) => {
                                for t in c {
                                    delayed_events.push(Reverse(DelayedEvent::from((
                                        Instant::now() + delay,
                                        t,
                                    ))));
                                }
                                // We got something, poll again
                                continue;
                            }
                            Ok(None) => {
                                stream_done = false;
                            }
                            _ => {}
                        }
                        // Got nothing, break
                        break;
                    }
                    st = st_ready_chunks.into_inner();
                    // We can be productive, return the delayed event
                    if let Some(ret) = return_val {
                        return Some((stream::iter(ret), (st, delayed_events, stream_done)));
                    }
                    if let Some(Reverse(de)) = delayed_events.peek() {
                        if let Ok(Some(t)) = timeout_at((de.delayed_until).into(), st.next()).await
                        {
                            delayed_events.push(Reverse((Instant::now() + delay, t).into()));
                        }
                    }
                }
            }
        }
    })
    .flatten()
}

#[cfg(test)]
mod test {
    use super::*;

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
        assert!(delta < Duration::from_millis(1), "{:?}", delta);

        retry_events_send.send(100).await.unwrap();
        delay_queue.as_mut().take(49).collect::<Vec<_>>().await;
        let delta = Instant::now() - next;
        let next = Instant::now();
        assert!(delta < Duration::from_millis(1), "{:?}", delta);

        delay_queue.as_mut().take(1).collect::<Vec<_>>().await;
        let delta = Instant::now() - next;
        assert!(delta > Duration::from_secs(1), "{:?}", delta);
    }
}

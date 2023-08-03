use crate::cache::EventTimestamp;

use notify_stream::Event as WatchEvent;

use futures::{
    future::FutureExt,
    stream::{self, Stream, StreamExt},
};

use std::cmp::Reverse;
use std::path::PathBuf;

#[derive(Default)]
struct PrioritisedEventQueue<T: std::cmp::Eq + std::hash::Hash> {
    pq: priority_queue::PriorityQueue<T, Reverse<i64>>,
}

impl<T: std::cmp::Eq + std::hash::Hash> PrioritisedEventQueue<T> {
    fn new() -> Self {
        Self {
            pq: priority_queue::PriorityQueue::default(),
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.pq.len()
    }
}

impl PrioritisedEventQueue<PrioritisedEvents<(WatchEvent, i64)>> {
    // Event should go on the back of the queue
    fn insert_event(&mut self, event: WatchEvent, ts: i64) {
        let p_event_priority = event
            .path_id()
            .and_then(|key| self.pq.get_priority(key).copied());

        if let Some(priority) = p_event_priority {
            if let Some((p_event, _)) = event.path_id().and_then(|key| {
                self.pq
                    .change_priority(key, std::cmp::max(Reverse(ts), priority));
                self.pq.get_mut(key)
            }) {
                p_event.update_events_from_back((event, ts));
            }
        } else if let Some(path) = event.path_id() {
            self.pq.push(
                PrioritisedEvents::new_with_event(path.clone(), (event, ts)),
                Reverse(ts),
            );
        };
    }

    // Event should go on the front of the queue
    fn requeue_event(&mut self, event: WatchEvent, ts: i64) {
        let p_event_priority = event
            .path_id()
            .and_then(|key| self.pq.get_priority(key).copied());

        if let Some(priority) = p_event_priority {
            if let Some((p_event, _)) = event.path_id().and_then(|key| {
                self.pq
                    .change_priority(key, std::cmp::max(Reverse(ts), priority));
                self.pq.get_mut(key)
            }) {
                p_event.update_events_from_front((event, ts));
            }
        } else if let Some(path) = event.path_id() {
            self.pq.push(
                PrioritisedEvents::new_with_event(path.clone(), (event, ts)),
                Reverse(ts),
            );
        };
    }

    #[cfg(test)]
    fn get(&self, key: &PathBuf) -> Option<(&PrioritisedEvents<(WatchEvent, i64)>, i64)> {
        self.pq.get(key).map(|(ret, priority)| (ret, priority.0))
    }

    fn pop(&mut self) -> Option<(WatchEvent, i64)> {
        let maybe_prioritised_event = self.pq.peek_mut();

        let (ret, new_priority) = maybe_prioritised_event?.0.pop()?;
        if let Some(key) = ret.0.path_id() {
            self.pq
                .change_priority(key, Reverse(new_priority.unwrap_or(i64::MAX)));
        }
        Some(ret)
    }
}

#[derive(Debug)]
struct PrioritisedEvents<T> {
    key: std::path::PathBuf, // What filepath is this talking about
    // Coalescing FIFO queue of events. end is head, start is tail
    pub events: smallvec::SmallVec<[T; 4]>, // Debounced events
}

impl PrioritisedEvents<(WatchEvent, i64)> {
    fn new_with_event(key: PathBuf, event: (WatchEvent, i64)) -> Self {
        Self {
            key,
            events: smallvec::smallvec![event],
        }
    }

    fn update_events_from_back(&mut self, event: (WatchEvent, i64)) -> Option<i64> {
        // Inserts at the end of the queue, ie the start of the smallvec
        use WatchEvent::*;
        let event = match event {
            (Remove(_) | Rename(_, _) | Create(_), _) => event,
            (Write(_), ts) => {
                // If the first element is already a write make sure it's the
                // oldest write we know about
                if let Some((Write(_), ref mut prev_ts)) = self.events.first_mut() {
                    *prev_ts = std::cmp::min(*prev_ts, ts);
                    return Some(*prev_ts);
                } else {
                    event
                }
            }
            // Err or Rescan
            _ => return None,
        };
        // New type of event, shift the rest of the events right and insert
        self.events.insert(0, event);
        None
    }

    fn update_events_from_front(&mut self, event: (WatchEvent, i64)) -> Option<i64> {
        use WatchEvent::*;
        // If the event is a write and the final entry is a write then make
        // sure it's the oldest write we know about
        let ts = event.1;
        if let ((Write(_), ts), Some((Write(_), ref mut prev_ts))) =
            (&event, self.events.last_mut())
        {
            *prev_ts = std::cmp::min(*prev_ts, *ts);
            Some(*prev_ts)
        } else {
            self.events.push(event);
            Some(ts)
        }
    }

    fn pop(&mut self) -> Option<((WatchEvent, i64), Option<i64>)> {
        self.events.pop().zip(
            self.events
                .last()
                .map_or(Some(None), |(_, ts)| Some(Some(*ts))),
        )
    }
}

impl<T> std::cmp::PartialEq for PrioritisedEvents<T> {
    #[inline]
    fn eq(&self, other: &PrioritisedEvents<T>) -> bool {
        self.key == other.key
    }
}

impl<T> std::cmp::Eq for PrioritisedEvents<T> {}

impl<T> std::hash::Hash for PrioritisedEvents<T> {
    fn hash<H: std::hash::Hasher>(&self, into: &mut H) {
        self.key.hash(into);
    }
}

impl<T> std::borrow::Borrow<PathBuf> for PrioritisedEvents<T> {
    fn borrow(&self) -> &PathBuf {
        &self.key
    }
}

pub fn debounce_fs_events<S1, S2>(
    notify_events: S1,
    resume_events: S2,
) -> impl Stream<Item = (WatchEvent, EventTimestamp)>
where
    S1: Stream<Item = (WatchEvent, EventTimestamp)> + Unpin,
    S2: Stream<Item = (WatchEvent, EventTimestamp)> + Unpin,
{
    stream::unfold(
        (
            PrioritisedEventQueue::<PrioritisedEvents<(WatchEvent, i64)>>::new(),
            notify_events.ready_chunks(100).peekable(),
            resume_events.ready_chunks(100).peekable(),
        ),
        move |(mut pq, mut notify_events_st, mut resume_events_st)| async move {
            loop {
                if let Some(Some(resume_events)) = resume_events_st.next().now_or_never() {
                    for (event, ts) in resume_events {
                        pq.requeue_event(event, ts.unix_timestamp())
                    }
                }

                if let Some(Some(notify_events)) = notify_events_st.next().now_or_never() {
                    for (event, ts) in notify_events {
                        pq.insert_event(event, ts.unix_timestamp())
                    }
                }
                let mut events: smallvec::SmallVec<[_; 10]> = smallvec::smallvec![];
                while events.len() < 10 {
                    if let Some((event, ts)) = pq.pop() {
                        events.push((event, EventTimestamp::from_unix_timestamp(ts).unwrap()));
                    } else {
                        break;
                    }
                }
                if !events.is_empty() {
                    return Some((
                        stream::iter(events),
                        (pq, notify_events_st, resume_events_st),
                    ));
                }

                futures::select_biased! {
                    _ = std::pin::Pin::new(&mut resume_events_st).peek() => (),
                    _ = std::pin::Pin::new(&mut notify_events_st).peek() => (),
                }
            }
        },
    )
    .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prioritised_event_comparators() {
        let mut pq: PrioritisedEventQueue<PrioritisedEvents<(WatchEvent, i64)>> =
            PrioritisedEventQueue::new();

        pq.insert_event(WatchEvent::Write("/tmp/test.path".into()), 0);
        assert_eq!(pq.len(), 1);

        pq.insert_event(WatchEvent::Write("/tmp/test.path".into()), 3);
        let event = pq.get(&PathBuf::from("/tmp/test.path")).unwrap().0;
        assert_eq!(event.events.len(), 1, "{event:#?}");

        assert_eq!(pq.len(), 1);
        assert!(pq.len() != 0);
        assert!(pq.len() != 2);

        pq.insert_event(WatchEvent::Create("/tmp/test.path".into()), 0);
        let event = pq.get(&PathBuf::from("/tmp/test.path")).unwrap().0;
        assert_eq!(event.events.len(), 2, "{event:#?}");
        assert_eq!(pq.len(), 1);
        pq.insert_event(WatchEvent::Write("/tmp/test.path".into()), 7);

        assert_eq!(pq.len(), 1);
        let event = pq.get(&PathBuf::from("/tmp/test.path")).unwrap().0;
        assert_eq!(event.events.len(), 3, "{event:#?}");
    }

    #[test]
    fn test_prioritised_event_insert() {
        let mut pq: PrioritisedEventQueue<PrioritisedEvents<(WatchEvent, i64)>> =
            PrioritisedEventQueue::new();

        pq.insert_event(WatchEvent::Write("/tmp/test.path".into()), 0);
        assert_eq!(pq.len(), 1);

        pq.insert_event(WatchEvent::Write("/tmp/test.path".into()), 3);
        let event = pq.get(&PathBuf::from("/tmp/test.path")).unwrap().0;
        assert_eq!(event.events.len(), 1, "{event:#?}");

        assert_eq!(pq.len(), 1);
        assert!(pq.len() != 0);
        assert!(pq.len() != 2);

        pq.insert_event(WatchEvent::Create("/tmp/test.path".into()), 0);
        let event = pq.get(&PathBuf::from("/tmp/test.path")).unwrap().0;
        assert_eq!(event.events.len(), 2, "{event:#?}");
        assert_eq!(pq.len(), 1);
        pq.insert_event(WatchEvent::Write("/tmp/test.path".into()), 7);

        pq.insert_event(WatchEvent::Remove("/tmp/test.path".into()), 7);
        pq.insert_event(WatchEvent::Create("/tmp/test.path".into()), 0);
        pq.insert_event(WatchEvent::Write("/tmp/test.path".into()), 7);

        assert_eq!(pq.len(), 1);
        let event = pq.get(&PathBuf::from("/tmp/test.path")).unwrap().0;
        assert_eq!(event.events.len(), 6, "{event:#?}");

        assert!(matches!(pq.pop(), Some((WatchEvent::Write(_), _))));
        assert!(matches!(pq.pop(), Some((WatchEvent::Create(_), _))));
        assert!(matches!(pq.pop(), Some((WatchEvent::Write(_), _))));
        assert!(matches!(pq.pop(), Some((WatchEvent::Remove(_), _))));
        assert!(matches!(pq.pop(), Some((WatchEvent::Create(_), _))));
        assert!(matches!(pq.pop(), Some((WatchEvent::Write(_), _))));
        assert!(pq.pop().is_none());
    }

    #[test]
    fn test_prioritised_event_requeue() {
        let mut pq: PrioritisedEventQueue<PrioritisedEvents<(WatchEvent, i64)>> =
            PrioritisedEventQueue::new();

        pq.insert_event(WatchEvent::Write("/tmp/test.path".into()), 0);
        assert_eq!(pq.len(), 1);

        pq.insert_event(WatchEvent::Write("/tmp/test.path".into()), 3);
        let event = pq.get(&PathBuf::from("/tmp/test.path")).unwrap().0;
        assert_eq!(event.events.len(), 1, "{event:#?}");

        assert_eq!(pq.len(), 1);
        assert!(pq.len() != 0);
        assert!(pq.len() != 2);

        pq.insert_event(WatchEvent::Create("/tmp/test.path".into()), 0);
        let event = pq.get(&PathBuf::from("/tmp/test.path")).unwrap().0;
        assert_eq!(event.events.len(), 2, "{event:#?}");
        assert_eq!(pq.len(), 1);
        pq.requeue_event(WatchEvent::Write("/tmp/test.path".into()), 7);
        assert_eq!(pq.len(), 1);
        let event = pq.get(&PathBuf::from("/tmp/test.path")).unwrap().0;
        assert_eq!(event.events.len(), 2, "{event:#?}");
        pq.insert_event(WatchEvent::Write("/tmp/test.path".into()), 7);
        let event = pq.get(&PathBuf::from("/tmp/test.path")).unwrap().0;
        assert_eq!(event.events.len(), 3, "{event:#?}");

        assert!(matches!(pq.pop(), Some((WatchEvent::Write(_), _))));
        pq.requeue_event(WatchEvent::Write("/tmp/test.path".into()), 7);
        assert!(matches!(pq.pop(), Some((WatchEvent::Write(_), _))));
        assert!(matches!(pq.pop(), Some((WatchEvent::Create(_), _))));
        assert!(matches!(pq.pop(), Some((WatchEvent::Write(_), _))));
        pq.requeue_event(WatchEvent::Create("/tmp/test.path".into()), 0);
        assert!(matches!(pq.pop(), Some((WatchEvent::Create(_), _))));
        pq.requeue_event(WatchEvent::Write("/tmp/test.path".into()), 7);
        assert!(matches!(pq.pop(), Some((WatchEvent::Write(_), _))));
        assert!(pq.pop().is_none());
    }

    #[test]
    fn test_prioritised_event_priorities() {
        let mut pq: PrioritisedEventQueue<PrioritisedEvents<(WatchEvent, i64)>> =
            PrioritisedEventQueue::new();

        pq.insert_event(WatchEvent::Write("/tmp/test.path1".into()), 1);
        assert_eq!(pq.len(), 1);

        pq.insert_event(WatchEvent::Write("/tmp/test.path2".into()), 3);
        let event = pq.get(&PathBuf::from("/tmp/test.path1")).unwrap().0;
        assert_eq!(event.events.len(), 1, "{event:#?}");

        assert_eq!(pq.len(), 2);
        pq.insert_event(WatchEvent::Create("/tmp/test.path1".into()), 2);
        pq.insert_event(WatchEvent::Write("/tmp/test.path1".into()), 9);

        pq.insert_event(WatchEvent::Write("/tmp/test.path2".into()), 3);
        pq.insert_event(WatchEvent::Remove("/tmp/test.path2".into()), 3);
        pq.requeue_event(WatchEvent::Write("/tmp/test.path2".into()), 3);
        pq.insert_event(WatchEvent::Write("/tmp/test.path2".into()), 3);
        pq.insert_event(WatchEvent::Remove("/tmp/test.path2".into()), 3);
        pq.insert_event(WatchEvent::Write("/tmp/test.path2".into()), 3);
        pq.insert_event(WatchEvent::Remove("/tmp/test.path2".into()), 3);
        pq.insert_event(WatchEvent::Write("/tmp/test.path2".into()), 3);

        let event = pq.get(&PathBuf::from("/tmp/test.path1")).unwrap().0;
        assert_eq!(event.events.len(), 3, "{event:#?}");
        assert_eq!(pq.len(), 2);

        pq.requeue_event(WatchEvent::Write("/tmp/test.path2".into()), 6);
        assert_eq!(pq.len(), 2);
        let event = pq.get(&PathBuf::from("/tmp/test.path2")).unwrap().0;

        assert_eq!(event.events.len(), 7, "{event:#?}");

        let test_path1 = PathBuf::from("/tmp/test.path1");
        let test_path2 = PathBuf::from("/tmp/test.path2");

        let ret = pq.pop();
        assert!(
            matches!(&ret, Some((WatchEvent::Write(path), _)) if path == &test_path1),
            "{:#?}",
            ret
        );
        pq.requeue_event(WatchEvent::Write("/tmp/test.path1".into()), 2);
        assert!(matches!(pq.pop(), Some((WatchEvent::Write(path), _)) if path == test_path1));
        assert!(matches!(pq.pop(), Some((WatchEvent::Create(path), _)) if path == test_path1));

        assert!(matches!(pq.pop(), Some((WatchEvent::Write(path), _)) if path == test_path2));
        pq.requeue_event(WatchEvent::Create("/tmp/test.path2".into()), 0);
        pq.insert_event(WatchEvent::Write("/tmp/test.path1".into()), 2);
        assert!(matches!(pq.pop(), Some((WatchEvent::Create(path), _)) if path == test_path2));
        assert!(matches!(pq.pop(), Some((WatchEvent::Write(path), _)) if path == test_path1));
        pq.insert_event(WatchEvent::Write("/tmp/test.path1".into()), 9);

        let event = pq.get(&test_path2).unwrap().0;
        assert_eq!(event.events.len(), 6, "{event:#?}");
        assert_eq!(pq.len(), 2);

        assert!(matches!(pq.pop(), Some((WatchEvent::Remove(path), _)) if path == test_path2));
        let ret = pq.pop();
        assert!(
            matches!(&ret, Some((WatchEvent::Write(path), _)) if path == &test_path2),
            "{:#?}",
            ret
        );
        assert!(matches!(pq.pop(), Some((WatchEvent::Remove(path), _)) if path == test_path2));
        assert!(matches!(pq.pop(), Some((WatchEvent::Write(path), _)) if path == test_path2));
        assert!(matches!(pq.pop(), Some((WatchEvent::Remove(path), _)) if path == test_path2));
        assert!(matches!(pq.pop(), Some((WatchEvent::Write(path), _)) if path == test_path2));
        assert!(matches!(pq.pop(), Some((WatchEvent::Write(path), _)) if path == test_path1));
        assert!(pq.pop().is_none());
        pq.insert_event(WatchEvent::Write("/tmp/test.path1".into()), 9);
        assert!(matches!(pq.pop(), Some((WatchEvent::Write(path), _)) if path == test_path1));
        assert!(pq.pop().is_none());
    }
}

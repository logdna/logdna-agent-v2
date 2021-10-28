use futures::stream::StreamExt;
use futures::Stream;
use std::sync::{Arc, Mutex};

pub struct Recover<St: Stream + Unpin, C, P> {
    inner: Arc<Mutex<St>>,
    recover_check: C,
    producer: Arc<Mutex<P>>,
}

/// Constructs a new stream that obtains elements from an underlying stream, created from
/// the `produce` function and will create new streams using `produce` when the `recover_check`
/// returns true.
pub fn recover<St: Stream + Unpin, C: Fn(&St::Item) -> bool, P: FnMut() -> St>(
    recover_check: C,
    producer: P,
) -> Recover<St, C, P> {
    let producer = Arc::new(Mutex::new(producer));
    let inner = {
        let mut produce_fn = producer.lock().unwrap();
        Arc::new(Mutex::new(produce_fn()))
    };

    Recover {
        inner,
        recover_check,
        producer,
    }
}

impl<St: Stream + Unpin, C: Fn(&St::Item) -> bool, P: FnMut() -> St> Stream for Recover<St, C, P> {
    type Item = St::Item;

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.lock().unwrap().size_hint()
    }

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        let mut state = self.inner.lock().unwrap();
        let _ret = state.poll_next_unpin(cx);

        if let std::task::Poll::Ready(Some(value)) = &_ret {
            if (self.recover_check)(value) {
                *state = {
                    // Obtain the new stream but in a small scope to limit the lock on the mutex
                    let mut produce_fn = self.producer.lock().unwrap();
                    produce_fn()
                };

                // Notify caller that they should poll the stream again now that the underlying
                // stream was recreated.
                let waker = cx.waker().clone();
                waker.wake();
                return std::task::Poll::Pending;
            }
        }

        return _ret;
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    #[test]
    fn with_empty_stream() {
        let should_recover = |_: &String| false;
        let data_producer = || futures::stream::empty();

        let test_st = recover(should_recover, data_producer).collect::<Vec<String>>();
        let result = tokio_test::block_on(test_st);

        assert!(result.is_empty());
    }

    #[test]
    fn exhaust_normal_stream() {
        let should_recover = |_: &usize| false;
        let data_producer = || futures::stream::iter(vec![1, 2, 3]);

        let test_st = recover(should_recover, data_producer).collect::<Vec<usize>>();
        let result = tokio_test::block_on(test_st);

        assert_eq!(result, vec![1, 2, 3]);
    }

    #[test]
    fn when_multiple_recoveries() {
        // Builds a closure to produce a stream of incrementing integers using a global
        // state such that a number is only produced once. A number, once produced, will
        // never be produced again across all instances of streams obtained from the closure.
        let global_stream_count: Rc<Cell<usize>> = Rc::new(Cell::new(1));
        let data_producer = || {
            let state = global_stream_count.clone();
            Box::pin(futures::stream::unfold(state, |state| async {
                let cur = state.get();
                state.set(cur + 1);
                Some((cur, state))
            }))
        };

        // For this test, any number divisible by 4 should trigger the recovery behavior.
        // Those values will be dropped from the collected output.
        let should_recover = |n: &usize| *n % 4 == 0;

        // Limit the recovery stream to producing 9 elements. The value at 4 and 8 will be skipped
        // which verifies that the stream was able to correctly recover more than once.
        let test_st = recover(should_recover, data_producer)
            .take(8)
            .collect::<Vec<usize>>();
        let result = tokio_test::block_on(test_st);

        assert_eq!(result, vec![1, 2, 3, 5, 6, 7, 9, 10]);
    }

    #[test]
    fn successive_recoveries() {
        // Builds a closure to produce a stream of incrementing integers up to 6, exclusive,
        // using a global state such that a number is only produced once. A number, once produced,
        // will never be produced again across all instances of streams obtained from the closure.
        let global_stream_count: Rc<Cell<usize>> = Rc::new(Cell::new(1));
        let data_producer = || {
            let state = global_stream_count.clone();
            Box::pin(futures::stream::unfold(state, |state| async {
                let cur = state.get();
                if cur < 6 {
                    state.set(cur + 1);
                    Some((cur, state))
                } else {
                    None
                }
            }))
        };

        // For this test, the recovery is triggered on both 2 and 3. This will cause two recovery
        // attempts in succession.
        let should_recover = |n: &usize| *n == 2 || *n == 3;

        let test_st = recover(should_recover, data_producer).collect::<Vec<usize>>();
        let result = tokio_test::block_on(test_st);

        assert_eq!(result, vec![1, 4, 5]);
    }
}

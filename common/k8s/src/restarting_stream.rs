use std::pin::Pin;
use std::task::{Context, Poll};

use pin_project_lite::pin_project;

use futures::{ready, Future, Stream};

pub enum RequiresRestart {
    Yes,
    No,
}

pin_project! {
    #[must_use = "streams do nothing unless polled"]
    pub struct RestartingStream<F, R, S, Fut> {
        f: F,
        r: R,
        #[pin]
        stream: S,
        #[pin]
        pending: Option<Fut>,
    }
}

impl<F, R, S, T, Fut> RestartingStream<F, R, S, Fut>
where
    F: FnMut() -> Fut,
    R: FnMut(&T) -> RequiresRestart,
    S: Stream<Item = T>,
    Fut: Future<Output = S>,
{
    pub async fn new(mut f: F, r: R) -> Self {
        let stream = f().await;
        Self {
            f,
            r,
            stream,
            pending: None,
        }
    }
}

impl<F, R, S, T, Fut> Stream for RestartingStream<F, R, S, Fut>
where
    F: FnMut() -> Fut,
    R: FnMut(&T) -> RequiresRestart,
    S: Stream<Item = T>,
    Fut: Future<Output = S>,
{
    type Item = T;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<T>> {
        let mut this = self.project();

        Poll::Ready(loop {
            if let Some(p) = this.pending.as_mut().as_pin_mut() {
                // We have an item in progress, poll that until it's done
                let stream = ready!(p.poll(cx));
                this.pending.set(None);
                this.stream.set(stream);
            } else if let Some(item) = ready!(this.stream.as_mut().poll_next(cx)) {
                match (this.r)(&item) {
                    RequiresRestart::Yes => {
                        this.pending.set(Some((this.f)()));
                    }
                    RequiresRestart::No => {
                        break Some(item);
                    }
                }
            } else {
                break None;
            }
        })
    }
}

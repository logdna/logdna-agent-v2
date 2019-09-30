use std::mem::replace;
use std::time::{Duration, Instant};

use crossbeam::{after, bounded, Receiver, Sender};
use either::Either;
use tokio::prelude::Future;
use tokio::runtime::Runtime;

use crate::limit::RateLimiter;
use crate::types::body::{IngestBody, Line, LineBuilder};
use crate::types::client::Client as HttpClient;
use crate::types::error::HttpError;
use crate::types::request::RequestTemplate;
use crate::types::response::Response;

/// Http(s) client used to send logs to the Ingest API
pub struct Client {
    inner: HttpClient,
    runtime: Runtime,
    line_sender: Sender<LineBuilder>,
    line_receiver: Receiver<LineBuilder>,
    retry_in_sender: Sender<IngestBody>,
    retry_in_receiver: Receiver<IngestBody>,
    retry_out_sender: Sender<IngestBody>,

    buffer: Vec<Line>,
    buffer_max_size: usize,
    buffer_bytes: usize,
    buffer_timeout: Receiver<Instant>,
    limiter: RateLimiter,
}

impl Client {
    /// Used to create a new instance of client, requiring a channel sender for retry
    /// and a request template for building ingest requests
    pub fn new(template: RequestTemplate) -> Self {
        let mut runtime = Runtime::new().expect("Runtime::new()");
        let (s, r) = bounded(0);
        let (temp, _) = bounded(0);
        let (retry_in_sender, retry_in_receiver) = bounded(0);
        Self {
            inner: HttpClient::new(template),
            runtime,
            line_sender: s,
            line_receiver: r,
            retry_in_sender,
            retry_in_receiver,
            retry_out_sender: temp,

            buffer: Vec::new(),
            buffer_max_size: 2 * 1024 * 1024,
            buffer_bytes: 0,
            buffer_timeout: new_timeout(),
            limiter: RateLimiter::new(10),
        }
    }
    /// Returns the channel senders used to send data from other threads
    pub fn sender(&self) -> (Sender<LineBuilder>, Sender<IngestBody>) {
        (self.line_sender.clone(), self.retry_in_sender.clone())
    }

    pub fn retry_sender(&self) -> Sender<LineBuilder> {
        self.line_sender.clone()
    }

    /// The main logic loop, consumes self because it should only be called once
    pub fn run(mut self, retry_sender: Sender<IngestBody>) {
        self.retry_out_sender = retry_sender;

        loop {
            if self.buffer_bytes < self.buffer_max_size {
                let msg = select! {
                    recv(self.line_receiver) -> msg => msg.map(Either::Left),
                    recv(self.retry_in_receiver) -> msg => msg.map(Either::Right),
                    recv(self.buffer_timeout) -> _ => {
                        self.flush();
                        continue;
                    },
                };
                // The left hand side of the either is new lines the come from the Tailer
                // The right hand side of the either is ingest bodies that are ready for retry
                match msg {
                    Ok(Either::Left(line)) => {
                        if let Ok(line) = line.build() {
                            self.buffer_bytes += line.line.len();
                            self.buffer.push(line);
                        }
                        continue;
                    }
                    Ok(Either::Right(body)) => {
                        self.send(body);
                        continue;
                    }
                    Err(_) => {}
                };
            } else {
                self.flush()
            }
        }
    }

    pub fn set_max_buffer_size(&mut self, size: usize) {
        self.buffer_max_size = size;
    }

    pub fn set_timeout(&mut self, timeout: Duration) {
        self.inner.set_timeout(timeout)
    }

    fn flush(&mut self) {
        let buffer = replace(&mut self.buffer, Vec::new());
        self.buffer_bytes = 0;
        self.buffer_timeout = new_timeout();

        if buffer.is_empty() {
            return;
        }

        self.send(IngestBody::new(buffer));
    }

    fn send(&mut self, body: IngestBody) {
        let sender = self.retry_out_sender.clone();
        let fut = self.inner.send(self.limiter.get_slot(body)).then(move |r| {
            match r {
                Ok(Response::Failed(_, s, r)) => warn!("bad response {}: {}", s, r),
                Err(HttpError::Send(body, e)) => {
                    warn!("failed sending http request, retrying: {}", e);
                    sender.send(body.into_inner()).unwrap();
                }
                Err(HttpError::Timeout(body)) => {
                    warn!("failed sending http request, retrying: request timed out!");
                    sender.send(body.into_inner()).unwrap();
                }
                Err(e) => {
                    warn!("failed sending http request: {}", e);
                }
                Ok(Response::Sent) => {} //success
            };
            Ok(())
        });
        self.runtime.spawn(fut);
    }
}

fn new_timeout() -> Receiver<Instant> {
    after(Duration::from_millis(250))
}

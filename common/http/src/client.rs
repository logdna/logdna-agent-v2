use std::mem::replace;
use std::time::{Duration, Instant};

use futures::FutureExt;

use crate::limit::RateLimiter;
use crate::retry::Retry;
use crate::types::body::{IngestBody, Line, LineBuilder};
use crate::types::client::Client as HttpClient;
use crate::types::error::HttpError;
use crate::types::request::RequestTemplate;
use crate::types::response::Response;
use metrics::Metrics;
use std::sync::Arc;

/// Http(s) client used to send logs to the Ingest API
pub struct Client {
    inner: HttpClient,
    limiter: RateLimiter,
    retry: Arc<Retry>,

    buffer: Vec<Line>,
    buffer_max_size: usize,
    buffer_bytes: usize,
    last_flush: Instant,
    last_retry: Instant,
}

impl Client {
    /// Used to create a new instance of client, requiring a channel sender for retry
    /// and a request template for building ingest requests
    pub fn new(template: RequestTemplate) -> Self {
        Self {
            inner: HttpClient::new(template),
            limiter: RateLimiter::new(10),
            retry: Arc::new(Retry::new()),
            buffer: Vec::new(),
            buffer_max_size: 2 * 1024 * 1024,
            buffer_bytes: 0,
            last_flush: Instant::now(),
            last_retry: Instant::now(),
        }
    }

    pub async fn poll(&mut self) {
        if self.should_retry() {
            self.last_retry = Instant::now();
            match self.retry.poll() {
                Ok(Some(body)) => self.make_request(body).await,
                Err(e) => error!("error polling retry: {}", e),
                _ => {}
            };
        }

        if self.should_flush() {
            self.flush().await
        }
    }
    /// The main logic loop, consumes self because it should only be called once
    pub async fn send(&mut self, line: LineBuilder) {
        self.poll().await;
        if let Ok(line) = line.build() {
            self.buffer_bytes += line.line.len();
            self.buffer.push(line);
        }
    }

    pub fn set_max_buffer_size(&mut self, size: usize) {
        self.buffer_max_size = size;
    }

    pub fn set_timeout(&mut self, timeout: Duration) {
        self.inner.set_timeout(timeout)
    }

    fn should_flush(&self) -> bool {
        self.buffer_bytes >= self.buffer_max_size
            || self.last_flush.elapsed() > Duration::from_millis(250)
    }

    fn should_retry(&self) -> bool {
        self.last_retry.elapsed() > Duration::from_secs(3)
    }

    async fn flush(&mut self) {
        let buffer = replace(&mut self.buffer, Vec::new());
        let buffer_size = self.buffer_bytes as u64;
        self.buffer_bytes = 0;
        self.last_flush = Instant::now();

        if buffer.is_empty() {
            return;
        }

        Metrics::http().add_request_size(buffer_size);
        Metrics::http().increment_requests();
        self.make_request(IngestBody::new(buffer)).await;
    }

    async fn make_request(&mut self, body: IngestBody) {
        let retry = self.retry.clone();
        self.inner
            .send(self.limiter.get_slot(body).as_ref())
            .map(move |r| {
                match r {
                    Ok(Response::Failed(_, s, r)) => warn!("bad response {}: {}", s, r),
                    Err(HttpError::Send(body, e)) => {
                        warn!("failed sending http request, retrying: {}", e);
                        if let Err(e) = retry.retry(body) {
                            error!("failed to retry request: {}", e)
                        }
                    }
                    Err(HttpError::Timeout(body)) => {
                        warn!("failed sending http request, retrying: request timed out!");
                        if let Err(e) = retry.retry(body) {
                            error!("failed to retry request: {}", e)
                        };
                    }
                    Err(e) => {
                        warn!("failed sending http request: {}", e);
                    }
                    Ok(Response::Sent) => {} //success
                }
            })
            .await
    }
}

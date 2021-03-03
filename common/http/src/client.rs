use std::collections::HashMap;
use std::pin::Pin;
use std::time::{Duration, Instant};

use futures::{FutureExt, Stream, StreamExt};

use crate::limit::RateLimiter;
use crate::retry::Retry;
use crate::types::body::IngestBodyBuffer;
use crate::types::client::Client as HttpClient;
use crate::types::error::HttpError;
use crate::types::request::RequestTemplate;
use crate::types::response::Response;
use crate::types::serialize::{
    body_serializer_source, IngestBodySerializer, IngestLineSerialize, IngestLineSerializeError,
};
use metrics::Metrics;
use std::sync::Arc;

/// Http(s) client used to send logs to the Ingest API
pub struct Client {
    inner: HttpClient,
    buffer_source:
        Pin<Box<dyn Stream<Item = Result<IngestBodySerializer, IngestLineSerializeError>>>>,
    limiter: RateLimiter,
    retry: Arc<Retry>,

    buffer: Option<IngestBodySerializer>,
    buffer_max_size: usize,
    buffer_bytes: usize,
    last_flush: Instant,
    last_retry: Instant,
}

impl Client {
    /// Used to create a new instance of client, requiring a channel sender for retry
    /// and a request template for building ingest requests
    pub fn new(template: RequestTemplate) -> Self {
        let buffer_source = Box::pin(body_serializer_source(
            2 * 1024 * 1024, /* 2MB */
            100 * 1024,      /*100 KB*/
        ));
        Self {
            inner: HttpClient::new(template),
            buffer_source,
            limiter: RateLimiter::new(10),
            retry: Arc::new(Retry::new()),
            buffer: None,
            buffer_max_size: 2 * 1024 * 1024,
            buffer_bytes: 0,
            last_flush: Instant::now(),
            last_retry: Instant::now(),
        }
    }

    pub async fn poll(&mut self) {
        if self.buffer.is_none() {
            match self.buffer_source.next().await {
                Some(Ok(buf)) => self.buffer = Some(buf),
                Some(Err(e)) => error!("{}", e),
                None => {
                    error!("client buffer stream shut down");
                    panic!("client buffer stream shut down");
                }
            }
        }
        if self.should_retry() {
            self.last_retry = Instant::now();
            match self.retry.poll().await {
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
    pub async fn send(
        &mut self,
        line: impl IngestLineSerialize<String, bytes::Bytes, HashMap<String, String>, Ok = ()>,
    ) {
        self.poll().await;
        self.buffer.as_mut().unwrap(/* poll will panic is this isn't set */).write_line(line).await.unwrap_or_else(|e| error!("{:?}", e));
        self.buffer_bytes = self.buffer.as_ref().map(|b| b.bytes_len()).unwrap_or(0);
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
        if self.buffer.is_none() || self.buffer.as_ref().unwrap().count() == 0 {
            return;
        }

        let buffer = {
            match self.buffer_source.next().await {
                Some(Ok(buf)) => buf,
                Some(Err(e)) => {
                    error!("{}", e);
                    return;
                }
                None => {
                    error!("client buffer stream shut down");
                    panic!("client buffer stream shut down");
                }
            }
        };
        let buffer = self.buffer.replace(buffer).unwrap();
        let buffer_size = self.buffer_bytes as u64;
        self.buffer_bytes = 0;
        self.last_flush = Instant::now();

        Metrics::http().add_request_size(buffer_size);
        Metrics::http().increment_requests();
        let body = buffer.end().expect("Failed to close ingest buffer");
        self.make_request(IngestBodyBuffer::from_buffer(body)).await;
    }

    async fn make_request(&mut self, body: IngestBodyBuffer) {
        let retry = self.retry.clone();
        self.inner
            .send(self.limiter.get_slot(body).as_ref().clone())
            .map(move |r| {
                match r {
                    Ok(Response::Failed(_, s, r)) => warn!("bad response {}: {}", s, r),
                    Err(HttpError::Send(body, e)) => {
                        warn!("failed sending http request, retrying: {}", e);
                        if let Err(e) = retry.retry(&body) {
                            error!("failed to retry request: {}", e)
                        }
                    }
                    Err(HttpError::Timeout(body)) => {
                        warn!("failed sending http request, retrying: request timed out!");
                        if let Err(e) = retry.retry(&body) {
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

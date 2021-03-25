use std::collections::HashMap;
use std::pin::Pin;
use std::time::{Duration, Instant};

use futures::{Stream, StreamExt};

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
use crate::Offset;
use metrics::Metrics;
use state::{FileOffsetFlushHandle, FileOffsetWriteHandle, GetOffset};
use std::sync::Arc;

/// Http(s) client used to send logs to the Ingest API
pub struct Client {
    inner: HttpClient,
    buffer_source:
        Pin<Box<dyn Stream<Item = Result<IngestBodySerializer, IngestLineSerializeError>>>>,
    limiter: RateLimiter,
    retry: Arc<Retry>,

    buffer: Option<IngestBodySerializer>,
    offsets: Option<Vec<Offset>>,
    buffer_max_size: usize,
    buffer_bytes: usize,
    last_flush: Instant,
    last_retry: Instant,
    state_write: Option<FileOffsetWriteHandle>,
    state_flush: Option<FileOffsetFlushHandle>,
}

impl Client {
    /// Used to create a new instance of client, requiring a channel sender for retry
    /// and a request template for building ingest requests
    pub fn new(
        template: RequestTemplate,
        state_handles: Option<(FileOffsetWriteHandle, FileOffsetFlushHandle)>,
    ) -> Self {
        let buffer_source = Box::pin(body_serializer_source(
            16 * 1024, /* 16 KB segments */
            50,        /* 16KB * 50 = 256 KB initial capacity */
            None,      /* No max size */
            Some(100), /* max 512KB idle buffers */
        ));
        let (offsets, state_write, state_flush) = state_handles
            .map(|(sw, sf)| (Some(Vec::new()), Some(sw), Some(sf)))
            .unwrap_or((None, None, None));
        Self {
            inner: HttpClient::new(template),
            buffer_source,
            limiter: RateLimiter::new(10),
            retry: Arc::new(Retry::new()),
            buffer: None,
            offsets,
            buffer_max_size: 2 * 1024 * 1024,
            buffer_bytes: 0,
            last_flush: Instant::now(),
            last_retry: Instant::now(),
            state_write,
            state_flush,
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
                Ok((offsets, Some(body))) => {
                    if let (Some(sw), Some(offsets)) = (self.state_write.as_ref(), &offsets) {
                        for (file_name, offset) in offsets {
                            debug!("Updating offset for {:?} to {}", file_name, *offset);
                            if let Err(e) = sw.update(file_name, *offset).await {
                                error!("Unable to write offsets. error: {}", e);
                            };
                        }
                    }
                    self.make_request(body).await
                }
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
        line: impl IngestLineSerialize<String, bytes::Bytes, HashMap<String, String>, Ok = ()>
            + GetOffset,
    ) {
        let key = line.get_key().map(bytes::Bytes::copy_from_slice);
        let offset = line.get_offset();
        self.poll().await;
        match self.buffer.as_mut().unwrap(/* poll will panic if this isn't set */).write_line(line).await
        {
            Ok(_) => {
                if let Some(wh) = self.state_write.as_ref() {
                    if let (Some(key), Some(offset)) = (key.as_ref(), offset) {
                        debug!("Updating offset for {:?} to {}", key, offset);
                        wh.update(key, offset).await.unwrap();
                    }
                }
                self.buffer_bytes = self.buffer.as_ref().map(|b| b.bytes_len()).unwrap_or(0);
            }
            Err(e) => error!("{:?}", e),
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
        let sf = self.state_flush.as_ref();
        match self
            .inner
            .send(self.limiter.get_slot(body).as_ref().clone())
            .await
        {
            Ok(Response::Failed(_, s, r)) => warn!("bad response {}: {}", s, r),
            Err(HttpError::Send(body, e)) => {
                warn!("failed sending http request, retrying: {}", e);
                if let Err(e) = retry.retry(self.offsets.as_ref(), &body) {
                    error!("failed to retry request: {}", e)
                }
            }
            Err(HttpError::Timeout(body)) => {
                warn!("failed sending http request, retrying: request timed out!");
                if let Err(e) = retry.retry(self.offsets.as_ref(), &body) {
                    error!("failed to retry request: {}", e)
                };
            }
            Err(e) => {
                warn!("failed sending http request: {}", e);
            }
            Ok(Response::Sent) => {
                if let Some(sf) = sf {
                    // Flush the state
                    if let Err(e) = sf.flush().await {
                        error!("Unable to flush state to disk. error: {}", e);
                    } else if let Some(offsets) = self.offsets.as_mut() {
                        offsets.clear()
                    }
                }
            } //success
        }
    }
}

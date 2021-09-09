use std::convert::TryInto;
use std::time::{Duration, Instant};

use crate::limit::RateLimiter;
use crate::offsets::Offset;
use crate::retry;
use crate::types::body::IngestBodyBuffer;
use crate::types::client::Client as HttpClient;
use crate::types::error::HttpError;
use crate::types::request::RequestTemplate;
use crate::types::response::Response;

use metrics::Metrics;
use state::{FileOffsetFlushHandle, FileOffsetWriteHandle};

/// Http(s) client used to send logs to the Ingest API
pub struct Client {
    inner: HttpClient,
    limiter: RateLimiter,
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
        let (state_write, state_flush) = state_handles
            .map(|(sw, sf)| (Some(sw), Some(sf)))
            .unwrap_or((None, None));
        Self {
            inner: HttpClient::new(template),
            limiter: RateLimiter::new(10),
            state_write,
            state_flush,
        }
    }

    pub async fn send(&self, body: IngestBodyBuffer, file_offsets: Option<&[Offset]>) {
        Metrics::http().add_request_size(body.len().try_into().unwrap());
        if let (Some(wh), Some(offsets)) = (self.state_write.as_ref(), file_offsets) {
            for (key, offset) in offsets {
                trace!("Updating offset for {:?} to {}", key, offset);
                if let Err(e) = wh.update(key, *offset).await {
                    error!("Unable to write offsets. error: {}", e);
                }
            }
        }
        self.make_request(body, file_offsets.as_deref()).await
    }

    async fn make_request(&self, body: IngestBodyBuffer, file_offsets: Option<&[Offset]>) {
        let sf = self.state_flush.as_ref();
        let start = Instant::now();
        match self
            .inner
            .send(self.limiter.get_slot(body).as_ref().clone())
            .await
        {
            Ok(Response::Failed(_, s, r)) => {
                Metrics::http().add_request_failure(start);
                warn!("bad response {}: {}", s, r);
            }
            Err(HttpError::Send(body, e)) => {
                Metrics::http().add_request_failure(start);
                warn!("failed sending http request, retrying: {}", e);
                if let Err(e) = retry::retry(file_offsets, &body).await {
                    error!("failed to retry request: {}", e)
                }
            }
            Err(HttpError::Timeout(body)) => {
                Metrics::http().add_request_timeout(start);
                warn!("failed sending http request, retrying: request timed out!");
                if let Err(e) = retry::retry(file_offsets, &body).await {
                    error!("failed to retry request: {}", e)
                };
            }
            Err(e) => {
                Metrics::http().add_request_failure(start);
                warn!("failed sending http request: {}", e);
            }
            Ok(Response::Sent) => {
                Metrics::http().add_request_success(start);
                if let Some(sf) = sf {
                    // Flush the state
                    if let Err(e) = sf.flush().await {
                        error!("Unable to flush state to disk. error: {}", e);
                    }
                }
            } //success
        }
    }

    pub fn set_timeout(&mut self, timeout: Duration) {
        self.inner.set_timeout(timeout)
    }
}

use std::convert::TryInto;
use std::time::{Duration, Instant};

use crate::limit::RateLimiter;
use crate::offsets::Offset;
use crate::retry::{self, RetrySender};
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
    retry: RetrySender,
    state_write: Option<FileOffsetWriteHandle>,
    state_flush: Option<FileOffsetFlushHandle>,
}

pub enum SendStatus {
    Sent,
    Retry(hyper::Error),
    RetryTimeout,
}

#[derive(Debug, thiserror::Error)]
pub enum ClientError<T>
where
    T: Send + 'static,
{
    #[error("{0}")]
    BadRequest(hyper::StatusCode),
    #[error("{0}")]
    Http(#[from] HttpError<T>),
    #[error("{0}")]
    Retry(#[from] retry::Error),
    #[error("{0}")]
    State(#[from] state::FileOffsetStateError),
}

impl Client {
    /// Used to create a new instance of client, requiring a channel sender for retry
    /// and a request template for building ingest requests
    pub fn new(
        template: RequestTemplate,
        retry: RetrySender,
        state_handles: Option<(FileOffsetWriteHandle, FileOffsetFlushHandle)>,
    ) -> Self {
        let (state_write, state_flush) = state_handles
            .map(|(sw, sf)| (Some(sw), Some(sf)))
            .unwrap_or((None, None));
        Self {
            inner: HttpClient::new(template),
            limiter: RateLimiter::new(10),
            retry,
            state_write,
            state_flush,
        }
    }

    pub async fn send<T>(
        &self,
        body: IngestBodyBuffer,
        file_offsets: Option<&[Offset]>,
    ) -> Result<SendStatus, ClientError<T>>
    where
        T: Send + 'static,
        ClientError<T>: From<HttpError<IngestBodyBuffer>>,
    {
        Metrics::http().add_request_size(body.len().try_into().unwrap());
        if let (Some(wh), Some(offsets)) = (self.state_write.as_ref(), file_offsets) {
            for (key, offset) in offsets {
                trace!("Updating offset for {:?} to {}", key, offset);
                if let Err(e) = wh.update(&[(*key, *offset)]).await {
                    error!("Unable to write offsets. error: {}", e);
                }
            }
        }
        let sf = self.state_flush.as_ref();
        let start = Instant::now();
        match self
            .inner
            .send(self.limiter.get_slot(body).as_ref().clone())
            .await
        {
            Ok(Response::Failed(_, s, r)) => {
                Metrics::http().add_request_failure(start);
                debug!("Failed request: {}", r);
                Err(ClientError::BadRequest(s))
            }
            Err(HttpError::Send(body, e)) => {
                Metrics::http().add_request_failure(start);
                warn!("failed sending http request, retrying: {}", e);
                self.retry.retry(file_offsets, &body).await?;
                Ok(SendStatus::Retry(e))
            }
            Err(HttpError::Timeout(body)) => {
                Metrics::http().add_request_timeout(start);
                self.retry.retry(file_offsets, &body).await?;
                Ok(SendStatus::RetryTimeout)
            }
            Err(e) => {
                Metrics::http().add_request_failure(start);
                Err(e.into())
            }
            Ok(Response::Sent) => {
                Metrics::http().add_request_success(start);
                if let Some(sf) = sf {
                    // Flush the state
                    sf.flush().await?
                }
                Ok(SendStatus::Sent)
            } //success
        }
    }

    pub fn set_timeout(&mut self, timeout: Duration) {
        self.inner.set_timeout(timeout)
    }
}

use std::convert::TryInto;
use std::time::{Duration, Instant};

use crate::limit::RateLimiter;
use crate::retry::{self, RetrySender};
use crate::types::body::IngestBodyBuffer;
use crate::types::client::Client as HttpClient;
use crate::types::error::HttpError;
use crate::types::request::RequestTemplate;
use crate::types::response::Response;

use metrics::Metrics;
use state::{FileOffsetFlushHandle, FileOffsetWriteHandle, OffsetMap};
use tracing::{debug, error};

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
    Retry(hyper_util::client::legacy::Error),
    RetryServerError(hyper::StatusCode, String),
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
        require_ssl: Option<bool>,
        concurrency_limit: Option<usize>,
        fo_state_handles: Option<(FileOffsetWriteHandle, FileOffsetFlushHandle)>,
    ) -> Self {
        let (state_write, state_flush) = fo_state_handles
            .map(|(sw, sf)| (Some(sw), Some(sf)))
            .unwrap_or((None, None));
        Self {
            inner: HttpClient::new(template, require_ssl).expect("failed to create client"),
            limiter: RateLimiter::new(concurrency_limit.unwrap_or(10)),
            retry,
            state_write,
            state_flush,
        }
    }

    pub async fn send<T>(
        &self,
        body: IngestBodyBuffer,
        file_offsets: Option<OffsetMap>,
    ) -> Result<SendStatus, ClientError<T>>
    where
        T: Send + 'static,
        ClientError<T>: From<HttpError<IngestBodyBuffer>> + Send + 'static,
        SendStatus: Send + 'static,
    {
        Metrics::http().add_request_size(body.len().try_into().unwrap());
        let update_key =
            if let (Some(wh), Some(offsets)) = (self.state_write.as_ref(), file_offsets.as_ref()) {
                wh.update(offsets.clone())
                    .await
                    .map_err(|e| {
                        error!("Unable to write offsets. error: {}", e);
                    })
                    .ok()
            } else {
                None
            };
        let sf = self.state_flush.as_ref();
        let start = Instant::now();
        match self
            .inner
            .send(self.limiter.get_slot(body).await.as_ref().clone())
            .await
        {
            Ok(Response::Failed(body, s, r))
                if [500, 501, 502, 503, 504, 507, 429].contains(&s.as_u16()) =>
            {
                Metrics::http().add_request_failure(start);
                debug!("failed request, retrying: {} {}", s, r);
                self.retry.retry(file_offsets, &body).await?;
                Ok(SendStatus::RetryServerError(s, r))
            }
            Ok(Response::Failed(_, s, r)) => {
                Metrics::http().add_request_failure(start);
                debug!("failed request: {} {}", s, r);
                Err(ClientError::BadRequest(s))
            }
            Err(HttpError::Send(body, e)) => {
                Metrics::http().add_request_failure(start);
                debug!("failed sending http request, retrying: {}", e);
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
                    sf.flush(update_key).await?
                }
                Ok(SendStatus::Sent)
            } //success
        }
    }

    pub fn set_timeout(&mut self, timeout: Duration) {
        self.inner.set_timeout(timeout)
    }
}

use thiserror::Error;
use tracing::warn;

#[derive(Debug, Error)]
pub enum SourceError {
    #[error("line needs to be retried for processing again later")]
    Retry(String),
    #[error("failed to commit new offset")]
    Offset(String),
}

pub trait RetryableSource<T> {
    // RetryableEvent must implement Retry
    type RetryableLine: RetryableLine;
    fn retryable(&self, _line: T) -> Self::RetryableLine;
}

pub trait SourceRetry {
    type RetryableLine;
    fn retry(
        &self,
        _line: &Self::RetryableLine,
        delay: std::time::Duration,
    ) -> Result<(), SourceError>;

    fn commit(&self, _line: &Self::RetryableLine) -> Result<(), SourceError>;
}

pub trait RetryableLine {
    fn retries_remaining(&self) -> u32;
    fn retry_at(&self) -> time::OffsetDateTime;
    fn retry_after(&self) -> std::time::Duration;
    fn retry(&self, delay: Option<std::time::Duration>) -> Result<(), SourceError>;
    fn commit(&self) -> Result<(), SourceError>;
}

impl<T> SourceRetry for std::sync::Weak<T>
where
    T: SourceRetry,
{
    type RetryableLine = T::RetryableLine;
    fn retry(
        &self,
        line: &Self::RetryableLine,
        delay: std::time::Duration,
    ) -> Result<(), SourceError> {
        if let Some(arc_self) = self.upgrade() {
            arc_self.retry(line, delay)
        } else {
            warn!("source is unavailable for retry");
            Ok(())
        }
    }

    fn commit(&self, line: &Self::RetryableLine) -> Result<(), SourceError> {
        if let Some(arc_self) = self.upgrade() {
            arc_self.commit(line)
        } else {
            warn!("source is unavailable for retry commit");
            Ok(())
        }
    }
}

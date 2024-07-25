use crate::cache::EntryKey;
use state::Span;

/// Represents a filesystem event
#[derive(Debug, Clone)]
pub enum Event {
    /// A file was created initialized
    Initialize(EntryKey),
    /// A new file was created
    New(EntryKey),
    /// A file was written too
    Write(EntryKey),
    /// A file was written too
    RetryWrite((EntryKey, Span)),
    /// A file was deleted
    Delete(EntryKey),
}

impl Event {
    pub(crate) fn key(&self) -> EntryKey {
        *match self {
            Event::Initialize(key) => key,
            Event::New(key) => key,
            Event::Write(key) => key,
            Event::RetryWrite((key, _)) => key,
            Event::Delete(key) => key,
        }
    }
}

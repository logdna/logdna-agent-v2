use crate::cache::EntryKey;
/// Represents a filesystem event
#[derive(Debug, Clone)]
pub enum Event {
    /// A file was created initialized
    Initialize(EntryKey),
    /// A new file was created
    New(EntryKey),
    /// A file was written too
    Write(EntryKey),
    /// A file was deleted
    Delete(EntryKey),
}

#[cfg(test)]
impl Event {
    pub(crate) fn key(&self) -> EntryKey {
        *match self {
            Event::Initialize(key) => key,
            Event::New(key) => key,
            Event::Write(key) => key,
            Event::Delete(key) => key,
        }
    }
}

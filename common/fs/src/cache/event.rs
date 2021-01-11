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

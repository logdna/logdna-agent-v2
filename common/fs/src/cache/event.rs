use slotmap::DefaultKey;
/// Represents a filesystem event
#[derive(Debug, Clone)]
pub enum Event {
    /// A file was created initialized
    Initialize(DefaultKey),
    /// A new file was created
    New(DefaultKey),
    /// A file was written too
    Write(DefaultKey),
    /// A file was deleted
    Delete(DefaultKey),
}

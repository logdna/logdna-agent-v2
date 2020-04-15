use crate::cache::entry::EntryPtr;

/// Represents a filesystem event
#[derive(Debug)]
pub enum Event<T> {
    /// A file was created initialized
    Initialize(EntryPtr<T>),
    /// A new file was created
    New(EntryPtr<T>),
    /// A file was written too
    Write(EntryPtr<T>),
    /// A file was deleted
    Delete(EntryPtr<T>),
}

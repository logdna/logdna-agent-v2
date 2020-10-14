use crate::cache::entry::EntryPtr;

/// Represents a filesystem event
#[derive(Debug, Clone)]
pub enum Event<T>
where
    T: Clone,
{
    /// A file was created initialized
    Initialize(EntryPtr<T>),
    /// A new file was created
    New(EntryPtr<T>),
    /// A file was written too
    Write(EntryPtr<T>),
    /// A file was deleted
    Delete(EntryPtr<T>),
}

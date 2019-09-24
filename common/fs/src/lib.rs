#[macro_use]
extern crate log;
#[macro_use]
extern crate quick_error;

use std::fmt::{Display, Error as FmtError, Formatter};
use std::path::PathBuf;

/// Contains the error type(s) for this crate
pub mod error;
/// Traits and types for defining exclusion and inclusion rules
pub mod rule;
/// Defines the tailer used to tail directories or single files
pub mod tail;
/// Defines the filesystem watcher
pub mod watch;

/// Represents a filesystem event
#[derive(Debug)]
pub enum Event {
    /// Sent on startup for each file currently being watched
    Initiate(PathBuf),
    /// A new file was created
    New(PathBuf),
    /// A file was deleted
    Delete(PathBuf),
    /// A file was written too
    Write(PathBuf),
}

impl Display for Event {
    fn fmt(&self, f: &mut Formatter) -> Result<(), FmtError> {
        match self {
            Event::Initiate(path) => write!(f, "INITIATE {:?}", path),
            Event::New(path) => write!(f, "NEW {:?}", path),
            Event::Delete(path) => write!(f, "DELETE {:?}", path),
            Event::Write(path) => write!(f, "WRITE {:?}", path),
        }
    }
}

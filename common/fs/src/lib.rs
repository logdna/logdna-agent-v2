#[macro_use]
extern crate log;
#[macro_use]
extern crate quick_error;
extern crate lazy_static;

/// Prototype
pub mod cache;
/// Contains the error type(s) for this crate
pub mod error;
/// Traits and types for defining exclusion and inclusion rules
pub mod rule;
/// The source for filesystem generated lines
pub mod source;
/// Defines the tailer used to tail directories or single files
pub mod tail;

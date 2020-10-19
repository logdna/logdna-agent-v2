#[macro_use]
extern crate log;
#[allow(unused_imports)]
#[macro_use]
extern crate lazy_static;

/// Prototype
pub mod cache;
/// Contains the error type(s) for this crate
pub mod error;
/// Traits and types for defining exclusion and inclusion rules
pub mod rule;
/// Defines the source implementation for fs
pub mod source;
/// Defines the tailer used to tail directories or single files
pub mod tail;

#[cfg(test)]
pub mod test {
    lazy_static! {
        pub static ref LOGGER: () = env_logger::init();
    }
}

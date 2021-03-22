#[macro_use]
extern crate log;

pub mod client;
pub mod limit;
pub mod retry;

pub mod types {
    pub use logdna_client::*;
}

type Offset = (Box<[u8]>, u64);

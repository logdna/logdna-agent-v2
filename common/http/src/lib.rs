#[macro_use]
extern crate log;

pub mod client;
pub mod limit;
pub mod metrics_endpoint;
pub mod retry;

pub mod types {
    pub use logdna_client::*;
}

type Offset = (u64, u64);

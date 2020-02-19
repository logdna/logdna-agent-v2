#[macro_use]
extern crate log;

use std::path::PathBuf;
use std::thread::{sleep, spawn};

use config::Config;
use fs::source::FSSource;
use http::client::Client;
use journald::JournaldSource;
use k8s::middleware::K8sMiddleware;
use metrics::Metrics;
use middleware::Executor;
use source::SourceReader;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

#[global_allocator]
static ALLOC: jemallocator::Jemalloc = jemallocator::Jemalloc;

fn main() {
    env_logger::init();
    info!("running version: {}", env!("CARGO_PKG_VERSION"));

    let config = match Config::new() {
        Ok(v) => v,
        Err(e) => {
            error!("config error: {}", e);
            std::process::exit(1);
        }
    };

    spawn(move || Metrics::start());

    let client = Rc::new(RefCell::new(Client::new(config.http.template)));
    client
        .borrow_mut()
        .set_max_buffer_size(config.http.body_size);
    client.borrow_mut().set_timeout(config.http.timeout);

    let mut executor = Executor::new();
    if PathBuf::from("/var/log/containers/").exists() {
        executor.register(K8sMiddleware::new());
    }

    let mut source_reader = SourceReader::new();
    source_reader.register(JournaldSource::new());
    source_reader.register(FSSource::new(config.log.dirs, config.log.rules));

    executor.init();

    loop {
        source_reader.drain(Box::new(|line| {
            if let Some(line) = executor.process(line) {
                client.borrow_mut().send(line)
            }
        }));
        client.borrow_mut().poll();
        sleep(Duration::from_millis(50));
    }
}

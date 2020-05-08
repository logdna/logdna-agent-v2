#[macro_use]
extern crate log;

use std::path::PathBuf;
use std::thread::spawn;

use config::Config;
use fs::source::FSSource;
use http::client::Client;
#[cfg(use_systemd)]
use journald::source::JournaldSource;
use k8s::middleware::{K8sDeduplication, K8sMetadata};
use metrics::Metrics;
use middleware::Executor;
use source::SourceReader;
use std::cell::RefCell;
use std::rc::Rc;
use std::thread::sleep;
use std::time::Duration;

#[global_allocator]
static ALLOC: jemallocator::Jemalloc = jemallocator::Jemalloc;

static SLEEP_DURATION: Duration = Duration::from_millis(10);

#[cfg(use_systemd)]
fn register_journald_source(source_reader: &mut SourceReader) {
    source_reader.register(JournaldSource::new());
}

#[cfg(not(use_systemd))]
fn register_journald_source(_source_reader: &mut SourceReader) {}

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
        executor.register(K8sDeduplication::new());
        match K8sMetadata::new() {
            Ok(v) => executor.register(v),
            Err(e) => warn!("{}", e),
        };
    }

    let mut source_reader = SourceReader::new();
    register_journald_source(&mut source_reader);
    source_reader.register(FSSource::new(config.log.dirs, config.log.rules));

    executor.init();

    loop {
        source_reader.drain(Box::new(|lines| {
            if let Some(lines) = executor.process(lines) {
                for line in lines {
                    client.borrow_mut().send(line)
                }
            }
        }));
        client.borrow_mut().poll();
        sleep(SLEEP_DURATION);
    }
}

#[macro_use]
extern crate log;

use std::path::PathBuf;
use std::thread::{sleep, spawn};

use config::Config;
use fs::tail::Tailer;
use fs::watch::Watcher;
use http::client::Client;
use k8s::K8s;
use metrics::Metrics;
use middleware::Executor;
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

    let mut watcher = Watcher::builder()
        .add_all(config.log.dirs)
        .append_all(config.log.rules)
        .build()
        .unwrap();

    let mut tailer = Tailer::new();

    let mut client = Client::new(config.http.template);
    client.set_max_buffer_size(config.http.body_size);
    client.set_timeout(config.http.timeout);

    let mut executor = Executor::new();
    if PathBuf::from("/var/log/containers/").exists() {
        executor.register(K8s::new());
    }

    watcher.init();
    executor.init();

    loop {
        let events = watcher.read_events();
        if events.is_empty() {
            client.poll();
            sleep(Duration::from_millis(50));
            continue;
        }

        events
            .into_iter()
            .map(|event| tailer.process(event))
            .flatten()
            .filter_map(|line| executor.process(line))
            .for_each(|line| client.send(line))
    }
}

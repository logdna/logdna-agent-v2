#[macro_use]
extern crate log;

use std::path::PathBuf;
use std::thread::spawn;

use config::Config;
use fs::tail::Tailer as FSSource;
use futures::StreamExt;
use http::client::Client;
#[cfg(use_systemd)]
use journald::source::JournaldSource;
use k8s::middleware::K8sMetadata;
use metrics::Metrics;
use middleware::Executor;
use pin_utils::pin_mut;
use std::cell::RefCell;
use std::rc::Rc;

use tokio::runtime::Runtime;

#[global_allocator]
static ALLOC: jemallocator::Jemalloc = jemallocator::Jemalloc;

// Statically include the CARGO_PKG_NAME and CARGO_PKG_VERSIONs in the binary
// and export under the PKG_NAME and PKG_VERSION symbols.
// These are used to identify the application and version, for example as part
// of the user agent string.
#[no_mangle]
pub static PKG_NAME: &str = env!("CARGO_PKG_NAME");
#[no_mangle]
pub static PKG_VERSION: &str = env!("CARGO_PKG_VERSION");

// #[cfg(use_systemd)]
// fn register_journald_source(source_reader: &mut SourceReader) {
//     source_reader.register(JournaldSource::new());
// }

// #[cfg(not(use_systemd))]
// fn register_journald_source(_source_reader: &mut SourceReader) {}

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

    spawn(Metrics::start);

    let client = Rc::new(RefCell::new(Client::new(config.http.template)));
    client
        .borrow_mut()
        .set_max_buffer_size(config.http.body_size);
    client.borrow_mut().set_timeout(config.http.timeout);

    let mut executor = Executor::new();
    if PathBuf::from("/var/log/containers/").exists() {
        match K8sMetadata::new() {
            Ok(v) => executor.register(v),
            Err(e) => warn!("{}", e),
        };
    }
    executor.init();

    let mut fs_tailer_buf = [0u8; 4096];
    let mut fs_source = FSSource::new(config.log.dirs, config.log.rules);
    // Create the runtime
    let mut rt = Runtime::new().unwrap();

    // Execute the future, blocking the current thread until completion
    rt.block_on(async {
        let fs_source = fs_source
            .process(&mut fs_tailer_buf)
            .expect("except Failed to create FS Tailer");
        pin_mut!(fs_source);

        let mut sources = futures::stream::SelectAll::new();
        sources.push(&mut fs_source);

        sources
            .for_each(|lines| async {
                if let Some(lines) = executor.process(lines) {
                    for line in lines {
                        // TODO upgrade to async hyper
                        client.borrow_mut().send(line)
                    }
                }
            })
            .await
    });
}

#[macro_use]
extern crate log;

use std::path::PathBuf;
use std::thread::spawn;

use futures::Stream;

use config::Config;
use env_logger::Env;
use fs::tail::Tailer as FSSource;
use futures::StreamExt;
use http::client::Client;

use journald::source::create_source;

use k8s::event_source::K8sEventStream;

use k8s::middleware::K8sMetadata;
use k8s::K8sEventLogConf;
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

fn main() {
    env_logger::from_env(Env::default().default_filter_or("info")).init();
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
    let mut fs_source = FSSource::new(config.log.dirs, config.log.rules, config.log.lookback);

    let journald_source = create_source(&config.journald.paths);

    let k8s_event_stream = match config.log.log_k8s_events {
        K8sEventLogConf::Never => None,
        K8sEventLogConf::Always => Some(
            K8sEventStream::try_default(
                std::env::var("POD_NAME").ok(),
                std::env::var("NAMESPACE").ok(),
            )
            .map_err(|e| {
                warn!("Failed to create kubernetes event stream: {}", e);
                e
            }),
        ),
    };
    // Create the runtime
    let mut rt = Runtime::new().unwrap();

    // Execute the future, blocking the current thread until completion
    rt.block_on(async move {
        let fs_source = fs_source
            .process(&mut fs_tailer_buf)
            .expect("except Failed to create FS Tailer");

        let journald_source = journald_source;

        let k8s_event_source: Option<_> = if let Some(fut) = k8s_event_stream
            .map(|e| e.ok().map(|e| e.event_stream()))
            .flatten()
        {
            Some(fut.await.expect("Failed to create stream"))
        } else {
            None
        };

        pin_mut!(fs_source);
        pin_mut!(journald_source);
        pin_mut!(k8s_event_source);

        let mut k8s_event_source: Option<std::pin::Pin<&mut _>> = k8s_event_source.as_pin_mut();

        let mut sources: futures::stream::SelectAll<&mut (dyn Stream<Item = Vec<_>> + Unpin)> =
            futures::stream::SelectAll::new();

        info!("Enabling filesystem");
        sources.push(&mut fs_source);
        sources.push(&mut journald_source);

        if let Some(k) = k8s_event_source.as_mut() {
            info!("Enabling k8s_event_source");
            sources.push(k)
        };

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

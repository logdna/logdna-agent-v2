#[macro_use]
extern crate log;

use std::path::PathBuf;

use futures::Stream;

use crate::stream_adapter::{StrictOrLazyLineBuilder, StrictOrLazyLines};
use config::{Config, DbPath};
use env_logger::Env;
use fs::tail::Tailer as FSSource;
use futures::StreamExt;
use http::batch::TimedRequestBatcherStreamExt;
use http::client::Client;
use http::retry::retry_stream;

#[cfg(feature = "libjournald")]
use journald::libjournald::source::create_source;

use journald::journalctl::create_journalctl_source;

use k8s::event_source::K8sEventStream;

use k8s::middleware::K8sMetadata;
use k8s::K8sTrackingConf;
use metrics::Metrics;
use middleware::line_rules::LineRules;
use middleware::Executor;

use pin_utils::pin_mut;
use state::AgentState;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;
use tokio::signal::*;

mod dep_audit;
mod stream_adapter;

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

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    env_logger::Builder::from_env(Env::default().default_filter_or("info")).init();
    info!("running version: {}", env!("CARGO_PKG_VERSION"));

    // Actually use the data to work around a bug in rustc:
    // https://github.com/rust-lang/rust/issues/47384
    dep_audit::get_auditable_dependency_list()
        .map_or_else(|e| trace!("{}", e), |d| trace!("{}", d));

    let config = match Config::new() {
        Ok(v) => v,
        Err(e) => {
            error!("config error: {}", e);
            std::process::exit(1);
        }
    };

    let mut _agent_state = None;
    let mut offset_state = None;
    let mut initial_offsets = None;
    if let DbPath::Path(path) = config.log.db_path {
        if path.is_dir() {
            match AgentState::new(path) {
                Ok(agent_state) => {
                    let _offset_state = agent_state.get_offset_state();
                    let offsets = _offset_state.offsets();
                    _agent_state = Some(agent_state);
                    offset_state = Some(_offset_state);
                    match offsets {
                        Ok(os) => {
                            initial_offsets =
                                Some(os.into_iter().map(|fo| (fo.key, fo.offset)).collect());
                        }
                        Err(e) => warn!("couldn't retrieve offsets from agent state, {:?}", e),
                    }
                }
                Err(e) => {
                    error!("Failed to open agent state db {}", e);
                }
            }
        } else {
            error!("{} is not a directory", path.to_string_lossy());
        }
    }

    let handles = offset_state
        .as_ref()
        .map(|os| (os.write_handle(), os.flush_handle()));
    let client = Rc::new(RefCell::new(Client::new(config.http.template, handles)));

    client.borrow_mut().set_timeout(config.http.timeout);

    let mut executor = Executor::new();
    if config.log.use_k8s_enrichment == K8sTrackingConf::Always
        && PathBuf::from("/var/log/containers/").exists()
    {
        match K8sMetadata::new().await {
            Ok(v) => {
                executor.register(v);
                info!("Registered k8s metadata middleware");
            }
            Err(e) => {
                let message = format!(
                    "The agent could not access k8s api after several attempts: {}",
                    e
                );
                error!("{}", message);
                panic!("{}", message);
            }
        };
    }

    match LineRules::new(
        &config.log.line_exclusion_regex,
        &config.log.line_inclusion_regex,
        &config.log.line_redact_regex,
    ) {
        Ok(v) => executor.register(v),
        Err(e) => {
            error!("line regex is invalid: {}", e);
            std::process::exit(1);
        }
    };

    executor.init();

    let mut fs_tailer_buf = [0u8; 4096];
    let mut fs_source = FSSource::new(
        config.log.dirs,
        config.log.rules,
        config.log.lookback,
        initial_offsets,
    );

    #[cfg(feature = "libjournald")]
    let (journalctl_source, journald_source) = if config.journald.paths.is_empty() {
        let journalctl_source = create_journalctl_source()
            .map(|s| s.map(StrictOrLazyLineBuilder::Strict))
            .map_err(|e| {
                info!("Journalctl source was not initialized");
                debug!("Journalctl source initialization error: {}", e);
            });
        (journalctl_source.ok(), None)
    } else {
        (
            None,
            Some(create_source(&config.journald.paths).map(StrictOrLazyLineBuilder::Strict)),
        )
    };

    #[cfg(not(feature = "libjournald"))]
    let journalctl_source = create_journalctl_source()
        .map(|s| s.map(StrictOrLazyLineBuilder::Strict))
        .map_err(|e| warn!("Error initializing journalctl source: {}", e))
        .ok();

    if let Some(offset_state) = offset_state {
        tokio::spawn(offset_state.run().unwrap());
    }

    let fs_source = fs_source
        .process(&mut fs_tailer_buf)
        .expect("except Failed to create FS Tailer")
        .map(StrictOrLazyLineBuilder::Lazy);

    let k8s_event_stream = match config.log.log_k8s_events {
        K8sTrackingConf::Never => None,
        K8sTrackingConf::Always => {
            let pod_name = std::env::var("POD_NAME").ok();
            let namespace = std::env::var("NAMESPACE").ok();
            let pod_label = std::env::var("POD_APP_LABEL").ok();
            match (pod_name, namespace, pod_label) {
                (Some(pod_name), Some(namespace), Some(pod_label)) => {
                    K8sEventStream::try_default(pod_name, namespace, pod_label)
                        .map_err(|e| warn!("Error initialising Kubernetes event logging: {}", e))
                        .ok()
                }
                (pn, n, pl) => {
                    if pn.is_none() {
                        warn!("Kubernetes event logging is configured, but POD_NAME env is not set")
                    }
                    if n.is_none() {
                        warn!(
                            "Kubernetes event logging is configured, but NAMESPACE env is not set"
                        )
                    }
                    if pl.is_none() {
                        warn!("Kubernetes event logging is configured, but POD_APP_LABEL env is not set")
                    }
                    warn!("Kubernetes event logging disabled");
                    None
                }
            }
        }
    };

    let k8s_event_source: Option<_> = if let Some(fut) = k8s_event_stream.map(|e| e.event_stream())
    {
        Some(
            fut.await
                .expect("Failed to create stream")
                .map(StrictOrLazyLineBuilder::Strict),
        )
    } else {
        None
    };

    pin_mut!(fs_source);
    pin_mut!(k8s_event_source);
    pin_mut!(journalctl_source);

    #[cfg(feature = "libjournald")]
    pin_mut!(journald_source);

    let mut k8s_event_source: Option<std::pin::Pin<&mut _>> = k8s_event_source.as_pin_mut();
    let mut journalctl_source: Option<std::pin::Pin<&mut _>> = journalctl_source.as_pin_mut();

    #[cfg(feature = "libjournald")]
    let mut journald_source: Option<std::pin::Pin<&mut _>> = journald_source.as_pin_mut();

    let mut sources: futures::stream::SelectAll<&mut (dyn Stream<Item = _> + Unpin)> =
        futures::stream::SelectAll::new();

    info!("Enabling filesystem");
    sources.push(&mut fs_source);

    #[cfg(feature = "libjournald")]
    if let Some(s) = journald_source.as_mut() {
        info!("Enabling journald event source");
        sources.push(s)
    } else if let Some(s) = journalctl_source.as_mut() {
        info!("Enabling journalctl event source");
        sources.push(s)
    }
    #[cfg(not(feature = "libjournald"))]
    if let Some(s) = journalctl_source.as_mut() {
        info!("Enabling journalctl event source");
        sources.push(s)
    }

    if let Some(k) = k8s_event_source.as_mut() {
        info!("Enabling k8s_event_source");
        sources.push(k)
    };

    let lines_stream = sources.map(|line| match line {
        StrictOrLazyLineBuilder::Strict(mut line) => {
            if executor.process(&mut line).is_some() {
                match line.build() {
                    Ok(line) => Some(StrictOrLazyLines::Strict(line)),
                    Err(e) => {
                        error!("Couldn't build line from linebuilder {:?}", e);
                        None
                    }
                }
            } else {
                None
            }
        }
        StrictOrLazyLineBuilder::Lazy(mut line) => {
            if executor.process(&mut line).is_some() {
                Some(StrictOrLazyLines::Lazy(line))
            } else {
                None
            }
        }
    });

    let body_offsets_stream = lines_stream
        .filter_map(|l| async { l })
        // TODO: paramaterise the flush frequency
        .timed_request_batches(config.http.body_size, Duration::from_millis(250));

    let lines_driver = body_offsets_stream.for_each(|body_offsets| async {
        match body_offsets {
            Ok((body, offsets)) => {
                client
                    .borrow()
                    .send(body, Some(offsets.items_as_ref()))
                    .await
            }
            Err(e) => error!("Couldn't batch lines {:?}", e),
        }
    });

    let retry_driver = retry_stream(
        config.http.retry_base_delay,
        // TODO: use config.http.retry_step_delay,
    )
    .for_each(|body_offsets| async {
        match body_offsets {
            Ok((body, offsets)) => {
                client
                    .borrow()
                    .send(body, offsets.as_ref().map(|o| o.as_ref()))
                    .await
            }
            Err(e) => error!("Couldn't batch lines {:?}", e),
        }
    });

    tokio::spawn(async {
        Metrics::log_periodically().await;
    });

    if let Some(port) = config.log.metrics_port {
        info!("Enabling prometheus endpoint with agent metrics");
        tokio::spawn(async move {
            // Should panic when server exits
            http::metrics_endpoint::serve(&port)
                .await
                .expect("metrics server error");
        });
    }

    // Concurrently run the line streams and listen for the `shutdown` signal
    tokio::select! {
        _ = lines_driver => {}
        _ = retry_driver => {}
        signal_name = get_signal() => {
            info!("Received {} signal, shutting down", signal_name)
        }
    }
}

#[cfg(unix)]
async fn get_signal() -> &'static str {
    let mut interrupt_signal = unix::signal(unix::SignalKind::interrupt()).unwrap();
    let mut quit_signal = unix::signal(unix::SignalKind::quit()).unwrap();
    let mut term_signal = unix::signal(unix::SignalKind::terminate()).unwrap();

    return tokio::select! {
        _ = interrupt_signal.recv() => { "SIGINT" }
        _ = quit_signal.recv() => { "SIGQUIT"  }
        _ = term_signal.recv() => { "SIGTERM" }
    };
}

#[cfg(windows)]
async fn get_signal() -> &'static str {
    ctrl_c().await.unwrap();
    "CTRL+C"
}

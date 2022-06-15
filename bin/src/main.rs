#[macro_use]
extern crate log;

use futures::Stream;

use crate::stream_adapter::{StrictOrLazyLineBuilder, StrictOrLazyLines};
use config::{Config, DbPath};
use env_logger::Env;
use fs::tail;
use futures::StreamExt;
use http::batch::TimedRequestBatcherStreamExt;
use http::client::{Client, ClientError, SendStatus};
use http::retry::{retry, RetryItem};

#[cfg(feature = "libjournald")]
use journald::libjournald::source::create_source;

use journald::journalctl::create_journalctl_source;

use k8s::event_source::K8sEventStream;
use k8s::lease::{get_available_lease, K8S_STARTUP_LEASE_LABEL, K8S_STARTUP_LEASE_RETRY_ATTEMPTS};

use k8s::middleware::K8sMetadata;
use k8s::{create_k8s_client_default_from_env, K8sTrackingConf};
use kube::Client as Kube_Client;
use metrics::Metrics;
use middleware::k8s_line_rules::K8sLineFilter;
use middleware::line_rules::LineRules;
use middleware::meta_rules::{MetaRules, MetaRulesConfig};
use middleware::Executor;

use pin_utils::pin_mut;
use state::{AgentState, FileId, SpanVec};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::signal::*;
use tokio::sync::Mutex;
use tokio::time::Duration;

mod dep_audit;
mod stream_adapter;

#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

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

    let config = match Config::new(std::env::args_os()) {
        Ok(v) => v,
        Err(e) => {
            error!("config error: {}", e);
            std::process::exit(1);
        }
    };

    let mut _agent_state = None;
    let mut offset_state = None;
    let mut initial_offsets: Option<HashMap<FileId, SpanVec>> = None;

    if let DbPath::Path(db_path) = &config.log.db_path {
        match AgentState::new(db_path) {
            Ok(agent_state) => {
                let _offset_state = agent_state.get_offset_state();
                let offsets = _offset_state.offsets();
                _agent_state = Some(agent_state);
                offset_state = Some(_offset_state);
                match offsets {
                    Ok(os) => {
                        initial_offsets =
                            Some(os.into_iter().map(|fo| (fo.key, fo.offsets)).collect());
                    }
                    Err(e) => warn!("couldn't retrieve offsets from agent state, {:?}", e),
                }
            }
            Err(e) => {
                error!("Failed to open agent state db {}", e);
            }
        }
    }

    let handles = offset_state
        .as_ref()
        .map(|os| (os.write_handle(), os.flush_handle()));

    let user_agent = config.http.template.user_agent.clone();
    let (retry, retry_stream) = retry(
        config.http.retry_dir,
        config.http.retry_base_delay,
        config.http.retry_step_delay,
        config.http.retry_disk_limit,
    );

    let concurrency_limit = Some(100);
    let mut client = Arc::new(Client::new(
        config.http.template,
        retry,
        Some(config.http.require_ssl),
        concurrency_limit,
        handles,
    ));

    if let Some(client) = Arc::get_mut(&mut client) {
        client.set_timeout(config.http.timeout);
    }

    let mut executor = Executor::new();

    let mut k8s_claimed_lease: Option<String> = None;
    let k8s_event_stream = match create_k8s_client_default_from_env(user_agent) {
        Ok(k8s_client) => {
            info!("K8s Config Startup Option: {:?}", &config.startup.option);
            check_startup_lease_status(
                Some(&config.startup.option),
                &mut k8s_claimed_lease,
                k8s_client.clone(),
            )
            .await;
            if config.log.use_k8s_enrichment == K8sTrackingConf::Always
                && std::env::var_os("KUBERNETES_SERVICE_HOST").is_some()
            {
                let node_name = std::env::var("NODE_NAME").ok();
                match K8sMetadata::new(k8s_client.clone(), node_name.as_deref()).await {
                    Ok((driver, v)) => {
                        tokio::spawn(driver);
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

            let k8s_event_stream = match config.log.log_k8s_events {
                K8sTrackingConf::Never => None,
                K8sTrackingConf::Always => {
                    let pod_name = std::env::var("POD_NAME").ok();
                    let namespace = std::env::var("NAMESPACE").ok();
                    let pod_label = std::env::var("POD_APP_LABEL").ok();
                    match (pod_name, namespace, pod_label) {
                        (Some(pod_name), Some(namespace), Some(pod_label)) => Some(
                            K8sEventStream::new(k8s_client.clone(), pod_name, namespace, pod_label),
                        ),
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

            match k8s_claimed_lease.as_ref() {
                Some(lease) => {
                    info!("Releasing lease: {:?}", lease);
                    let k8s_lease_api = k8s::lease::get_k8s_lease_api(
                        &std::env::var("NAMESPACE").unwrap(),
                        k8s_client.clone(),
                    )
                    .await;
                    k8s::lease::release_lease(lease, &k8s_lease_api).await;
                }
                None => {
                    info!("No K8s lease claimed during startup.");
                }
            }

            k8s_event_stream
        }
        Err(e) => {
            warn!("Unable to initialize kubernetes client: {}", e);
            None
        }
    };

    if config.log.use_k8s_enrichment == K8sTrackingConf::Always {
        println!(
            "\n*** MAIN CONFIG: {:?} {:?}\n",
            config.log.k8s_metadata_exclude, config.log.k8s_metadata_include
        );
        match K8sLineFilter::new(
            &config.log.k8s_metadata_exclude,
            &config.log.k8s_metadata_include,
        ) {
            Ok(v) => {
                println!("*** REGISTERING: {:?}", v);
                executor.register(v)
            }
            Err(e) => {
                error!("k8s line rule is invalid {}", e);
                std::process::exit(1);
            }
        }
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

    match MetaRules::new(MetaRulesConfig::from_env()) {
        Ok(v) => executor.register(v),
        Err(e) => {
            error!("line regex is invalid: {}", e);
            std::process::exit(1);
        }
    };

    executor.init();

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

    let ds_source_params = (
        config.log.dirs.clone(),
        config.log.rules.clone(),
        config.log.lookback.clone(),
        initial_offsets.clone(),
    );

    let fs_source = tail::RestartingTailer::new(
        ds_source_params,
        |item| match item {
            Err(fs::cache::Error::WatchOverflow) => {
                warn!("overflowed kernel queue, restarting stream");
                true
            }
            _ => false,
        },
        |params| {
            let watched_dirs = params.0.clone();
            let rules = params.1.clone();
            let lookback = params.2.clone();
            let offsets = params.3.clone();
            let tailer = tail::Tailer::new(watched_dirs, rules, lookback, offsets);
            async move { tail::process(tailer).expect("except Failed to create FS Tailer") }
        },
    )
    .await
    .filter_map(|r| async {
        match r {
            Err(e) => {
                match e {
                    fs::cache::Error::PathNotValid(path) => {
                        debug!("Path is not longer valid: {:?}", path);
                    }
                    _ => {
                        warn!("Processing inotify event resulted in error: {}", e);
                    }
                };
                None
            }
            Ok(lazy_lin_ser) => Some(StrictOrLazyLineBuilder::Lazy(lazy_lin_ser)),
        }
    });

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
        .timed_request_batches(config.http.body_size, Duration::from_millis(250))
        .map(|b| async { b })
        .buffered(10);

    async fn handle_client_error<T>(
        e: ClientError<T>,
        shutdown_tx: Arc<Mutex<Option<tokio::sync::oneshot::Sender<()>>>>,
    ) where
        T: Send + 'static,
    {
        match e {
            ClientError::BadRequest(s) => {
                if s.is_client_error() {
                    error!("bad request, check configuration: {}", s);
                    shutdown_tx.lock().await.take().unwrap().send(()).unwrap();
                } else {
                    warn!("bad http request: {}", s);
                }
            }
            ClientError::Http(e) => {
                warn!("failed sending http request: {}", e);
            }
            ClientError::Retry(r) => {
                error!("failed to retry request: {}", r);
            }
            ClientError::State(s) => {
                error!("Unable to flush state to disk. error: {}", s);
            }
        }
    }

    fn handle_send_status(s: SendStatus) {
        match s {
            SendStatus::Retry(e) => {
                warn!("failed sending http request, retrying: {}", e);
            }
            SendStatus::RetryTimeout => {
                warn!("failed sending http request, retrying: request timed out!");
            }
            _ => {}
        }
    }

    let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel();
    let shutdown_tx = Arc::new(Mutex::new(Some(shutdown_tx)));

    let lines_client = client.clone();
    let lines_driver = body_offsets_stream.for_each_concurrent(None, {
        let shutdown_tx = shutdown_tx.clone();
        move |body_offsets| {
            let client = lines_client.clone();
            let shutdown_tx = shutdown_tx.clone();
            async {
                tokio::spawn(async move {
                    match body_offsets {
                        Ok((body, offsets)) => match client.send(body, Some(offsets)).await {
                            Ok(s) => handle_send_status(s),
                            Err(e) => handle_client_error(e, shutdown_tx).await,
                        },
                        Err(e) => error!("Couldn't batch lines {:?}", e),
                    }
                })
                .await
                .expect("Join Error")
            }
        }
    });

    let retry_driver = retry_stream
        .into_stream()
        .for_each_concurrent(None, move |body_offsets| {
            let shutdown_tx = shutdown_tx.clone();
            let client = client.clone();

            async move {
                tokio::spawn({
                    let shutdown_tx = shutdown_tx.clone();
                    async move {
                        match body_offsets {
                            Ok(item) => {
                                let RetryItem {
                                    body_buffer,
                                    offsets,
                                    path: _,
                                } = item;
                                match client.send(body_buffer, offsets).await {
                                    Ok(s) => match s {
                                        SendStatus::Sent => {
                                            Metrics::http().increment_retries_success()
                                        }
                                        _ => {
                                            Metrics::http().increment_retries_failure();
                                            handle_send_status(s)
                                        }
                                    },
                                    Err(e) => handle_client_error(e, shutdown_tx).await,
                                }
                            }
                            Err(e) => error!("Couldn't batch lines {:?}", e),
                        }
                    }
                })
                .await
                .expect("Join Error")
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
        _ = &mut shutdown_rx => {}
        signal_name = get_signal() => {
            info!("Received {} signal, shutting down", signal_name)
        }
    }
}

async fn check_startup_lease_status(
    start_option: Option<&str>,
    claimed_lease_ref: &mut Option<String>,
    client: Kube_Client,
) {
    let max_attempts = match start_option {
        Some("attempt") => {
            info!("Getting agent-startup-lease (making limited attempts)");
            K8S_STARTUP_LEASE_RETRY_ATTEMPTS
        }
        Some("always") => {
            info!("Getting agent-startup-lease (trying forever)");
            -1
        }
        _ => {
            info!(
                "Kubernetes cluster initialised, K8s startup lease set to: {:?}",
                start_option
            );
            return;
        }
    };

    let k8s_lease_api =
        k8s::lease::get_k8s_lease_api(&std::env::var("NAMESPACE").unwrap(), client).await;
    let mut attempts = 0;
    while (max_attempts == -1) || (attempts < max_attempts) {
        info!("Attempting connection: {}", attempts);
        match get_available_lease(K8S_STARTUP_LEASE_LABEL, &k8s_lease_api).await {
            Some(available_lease) => {
                info!("Lease available: {:?}", available_lease);
                k8s::lease::claim_lease(
                    available_lease,
                    std::env::var("POD_NAME").unwrap(),
                    &k8s_lease_api,
                    claimed_lease_ref,
                )
                .await;
                break;
            }
            None => {
                attempts += 1;
                info!("No lease availabe at this time. Waiting 1 second...");
                tokio::time::sleep(Duration::from_millis(1000)).await;
            }
        };
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

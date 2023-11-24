use futures::Stream;

use config::{self, ArgumentOptions, Config, DbPath, K8sLeaseConf, K8sTrackingConf};
use fs::lookback::Lookback;
use fs::tail;
use futures::{stream, StreamExt};
use http::batch::TimedRequestBatcherStreamExt;
use http::client::{Client, ClientError, SendStatus};
use http::retry::{retry, RetryItem};
use k8s::feature_leader::FeatureLeader;
use k8s::feature_leader::FeatureLeaderMeta;
use k8s::metrics_stats_stream::MetricsStatsStream;
use middleware::MiddlewareError;
use rate_limit_macro::rate_limit;

use fs::cache::delayed_stream::delayed_stream;

#[cfg(all(feature = "libjournald", target_os = "linux"))]
use journald::libjournald::source::create_source;

#[cfg(target_os = "linux")]
use journald::journalctl::create_journalctl_source;

use api::tailer::create_tailer_source;

use k8s::errors::K8sError;
use k8s::event_source::K8sEventStream;
use k8s::lease::{get_available_lease, K8S_STARTUP_LEASE_LABEL, K8S_STARTUP_LEASE_RETRY_ATTEMPTS};

use k8s::create_k8s_client_default_from_env;
use k8s::middleware::metadata_runner;
use kube::Client as Kube_Client;
use metrics::Metrics;
use middleware::k8s_line_rules::K8sLineFilter;
use middleware::line_rules::LineRules;
use middleware::meta_rules::{MetaRules, MetaRulesConfig};
use middleware::Executor;

use pin_utils::pin_mut;
use rand::Rng;
use state::{AgentState, FileId, GetOffset, Span, SpanVec};
use std::collections::HashMap;
use std::convert::TryFrom;
use std::path::Path;
use std::sync::Arc;
use tokio::signal::*;
use tokio::sync::Mutex;
use tokio::time::sleep;
use tokio::time::Duration;
use tracing::{debug, error, info, trace, warn};

#[cfg(feature = "dep_audit")]
use crate::dep_audit;
use crate::stream_adapter::{StrictOrLazyLineBuilder, StrictOrLazyLines};

pub static REPORTER_LEASE_NAME: &str = "logdna-agent-reporter-lease";
pub static K8S_EVENTS_LEASE_NAME: &str = "logdna-agent-k8-events-lease";
pub static DEFAULT_CHECK_FOR_LEADER_S: i32 = 300;

/// Debounce filesystem event
static FS_EVENT_DELAY: Duration = Duration::from_millis(10);

// Statically include the CARGO_PKG_NAME and CARGO_PKG_VERSIONs in the binary
// and export under the PKG_NAME and PKG_VERSION symbols.
// These are used to identify the application and version, for example as part
// of the user agent string.
#[no_mangle]
pub static PKG_NAME: &str = env!("CARGO_PKG_NAME");
#[no_mangle]
pub static PKG_VERSION: &str = env!("CARGO_PKG_VERSION");

pub async fn _main(
    shutdown_tx: Arc<Mutex<Option<tokio::sync::oneshot::Sender<()>>>>,
    mut shutdown_rx: tokio::sync::oneshot::Receiver<()>,
) -> anyhow::Result<()> {
    // Actually use the data to work around a bug in rustc:
    // https://github.com/rust-lang/rust/issues/47384
    #[cfg(feature = "dep_audit")]
    dep_audit::get_auditable_dependency_list()
        .map_or_else(|e| trace!("{}", e), |d| trace!("{}", d));

    let args = std::env::args_os();
    let argv_options = ArgumentOptions::from_args_with_all_env_vars(args);
    let print_settings_and_exit = argv_options.list_settings;
    let config = Config::new_from_options(argv_options).unwrap_or_else(|e| {
        error!("Configuration error: {}", e);
        // config errors are fatal
        std::process::exit(consts::exit_codes::EINVAL);
    });
    if print_settings_and_exit {
        std::process::exit(0);
    }

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

    let mut _agent_state = None;
    let mut offset_state = None;

    if config.log.lookback != Lookback::None {
        if let DbPath::Path(db_path) = &config.log.db_path {
            match AgentState::new(db_path) {
                Ok(agent_state) => {
                    let _offset_state = agent_state.get_offset_state();
                    _agent_state = Some(agent_state);
                    offset_state = Some(_offset_state);
                }
                Err(e) => {
                    error!("Failed to open agent state db {}", e);
                }
            }
        }
    }

    let fo_state_handles = offset_state
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
        fo_state_handles,
    ));

    if let Some(client) = Arc::get_mut(&mut client) {
        client.set_timeout(config.http.timeout);
    }

    let (delayed_lines_send, delayed_lines_recv) = async_channel::unbounded();
    let delayed_lines_source = if !config.log.metadata_retry_delay.is_zero() {
        Some(
            delayed_stream(delayed_lines_recv, config.log.metadata_retry_delay)
                .map(StrictOrLazyLineBuilder::LazyDelayed),
        )
    } else {
        None
    };
    let mut executor = Executor::new();

    let mut k8s_claimed_lease: Option<String> = None;
    let mut metrics_stream_feature_meta: Option<FeatureLeaderMeta> = None;

    let (deletion_ack_sender, deletion_ack_receiver) = async_channel::unbounded();
    let (k8s_event_stream, metric_stats_stream) =
        match create_k8s_client_default_from_env(user_agent.clone()) {
            Ok(k8s_client) => {
                info!("K8s Config Startup Option: {:?}", &config.startup);
                check_startup_lease_status(
                    Some(&config.startup),
                    &mut k8s_claimed_lease,
                    k8s_client.clone(),
                )
                .await;

                if config.log.use_k8s_enrichment == K8sTrackingConf::Always && k8s::is_in_cluster()
                {
                    let node_name = std::env::var("NODE_NAME").ok();
                    metadata_runner(deletion_ack_receiver, user_agent, node_name, &mut executor);
                }

                let pod_name = std::env::var("POD_NAME").ok();
                let namespace = std::env::var("NAMESPACE").ok();
                let pod_label = std::env::var("POD_APP_LABEL").ok();

                let k8s_event_stream = match config.log.log_k8s_events {
                    K8sTrackingConf::Never => None,
                    K8sTrackingConf::Always => {
                        let k8s_event_stream_feature_meta = set_up_leader(
                            &namespace,
                            &k8s_client,
                            &pod_name,
                            K8S_EVENTS_LEASE_NAME.to_string(),
                        )
                        .await;

                        Some(K8sEventStream::new(
                            k8s_client.clone(),
                            pod_name.unwrap_or_default(),
                            namespace.unwrap_or_default(),
                            pod_label.unwrap_or_default(),
                            Arc::new(k8s_event_stream_feature_meta),
                        ))
                    }
                };

                let pod_name = std::env::var("POD_NAME").ok();
                let namespace = std::env::var("NAMESPACE").ok();

                let metric_stats_stream = match config.log.log_metric_server_stats {
                    K8sTrackingConf::Never => None,
                    K8sTrackingConf::Always => {
                        let created_metrics_stream_feature_meta = set_up_leader(
                            &namespace,
                            &k8s_client,
                            &pod_name,
                            REPORTER_LEASE_NAME.to_string(),
                        )
                        .await;

                        let feature_leader = created_metrics_stream_feature_meta.leader.clone();
                        metrics_stream_feature_meta = Some(created_metrics_stream_feature_meta);

                        Some(MetricsStatsStream::new(k8s_client.clone(), feature_leader))
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
                };

                (k8s_event_stream, metric_stats_stream)
            }
            Err(K8sError::K8sNotInClusterError()) => {
                debug!("Not in a k8s cluster, not initializing kube client");
                (None, None)
            }
            Err(e) => {
                warn!("Unable to initialize kubernetes client: {}", e);
                (None, None)
            }
        };

    if config.log.use_k8s_enrichment == K8sTrackingConf::Always {
        match K8sLineFilter::new(
            &config.log.k8s_metadata_exclude,
            &config.log.k8s_metadata_include,
        ) {
            Ok(v) => executor.register(v),
            Err(e) => {
                error!("k8s line rule is invalid {}", e);
                std::process::exit(consts::exit_codes::EINVAL);
            }
        }
    }

    executor.register(
        LineRules::new(
            &config.log.line_exclusion_regex,
            &config.log.line_inclusion_regex,
            &config.log.line_redact_regex,
        )
        .map_err(|e| {
            error!("line regex is invalid: {}", e);
            e
        })?,
    );

    executor.register(MetaRules::new(MetaRulesConfig::from_env()).map_err(|e| {
        error!("line regex is invalid: {}", e);
        e
    })?);

    info!("initializing middleware executor");
    executor.init();

    // Use an internal env var to support running integration test w/o additional delays
    let _event_delay = std::env::var(config::env_vars::INTERNAL_FS_DELAY)
        .map(|s| Duration::from_millis(s.parse().unwrap()))
        .unwrap_or(FS_EVENT_DELAY);

    #[cfg(all(feature = "libjournald", target_os = "linux"))]
    let journald_source = match config.journald.systemd_journal_tailer {
        true => {
            if !config.journald.paths.is_empty() {
                Some(create_source(&config.journald.paths).map(StrictOrLazyLineBuilder::Strict))
            } else {
                None
            }
        }
        false => None,
    };

    #[cfg(all(feature = "libjournald", target_os = "linux"))]
    let journalctl_source = match config.journald.systemd_journal_tailer {
        true => {
            if config.journald.paths.is_empty() {
                create_journalctl_source()
                    .map(|s| s.map(StrictOrLazyLineBuilder::Strict))
                    .map_err(|e| {
                        info!("Journalctl source was not initialized");
                        debug!("Journalctl source initialization error: {}", e);
                    })
                    .ok()
            } else {
                None
            }
        }
        false => None,
    };

    #[cfg(all(not(feature = "libjournald"), target_os = "linux"))]
    let journalctl_source = match config.journald.systemd_journal_tailer {
        true => create_journalctl_source()
            .map(|s| s.map(StrictOrLazyLineBuilder::Strict))
            .map_err(|e| warn!("Error initializing journalctl source: {}", e))
            .ok(),
        false => None,
    };

    let tailer_source = match (config.log.tailer_cmd, config.log.tailer_args) {
        (Some(cmd), Some(args)) => {
            let tailer_cmd = cmd.as_str();
            if !(Path::new(tailer_cmd).is_file()) {
                error!(
                    "Error initializing api tailer source: 'log.tailer_cmd' file [{}] does not exist",
                    tailer_cmd
                );
                std::process::exit(1);
            } else {
                let tailer_args: Vec<String> =
                    shell_words::split(args.as_str()).unwrap_or_else(|_| {
                        error!("Malformed 'log.tailer_args' config option: '{}'", args);
                        std::process::exit(1);
                    });
                let src = create_tailer_source(
                    tailer_cmd,
                    tailer_args.iter().map(|s| s.as_ref()).collect(),
                    shutdown_tx.clone(),
                )
                .map(|s| s.map(StrictOrLazyLineBuilder::Strict))
                .map_err(|e| {
                    error!("Error initializing api tailer source: {}", e);
                    std::process::exit(1);
                })
                .ok();
                debug!("Initialised api tailer source");
                src
            }
        }
        (_, _) => None,
    };

    debug!("Initialising offset state");
    let fo_state_handles = offset_state
        .as_ref()
        .map(|os| (os.write_handle(), os.flush_handle()));
    if let Some(offset_state) = offset_state.clone() {
        tokio::spawn(offset_state.run().unwrap());
    }
    debug!("Initialised offset state");

    let mut initial_offsets: Option<HashMap<FileId, SpanVec>> = None;

    if let Some(offset_state) = offset_state {
        match offset_state.offsets() {
            Ok(offsets) => {
                initial_offsets =
                    Some(offsets.into_iter().map(|fo| (fo.key, fo.offsets)).collect());
            }
            Err(e) => warn!("couldn't retrieve offsets from agent state, {:?}", e),
        }
    }

    let fs_offsets: Arc<Mutex<HashMap<FileId, SpanVec>>> =
        Arc::new(Mutex::new(initial_offsets.unwrap_or_default()));

    let ds_source_params = (
        config.log.dirs.clone(),
        config.log.rules.clone(),
        config.log.lookback.clone(),
        fo_state_handles,
        fs_offsets,
    );

    debug!("Creating fs_source");
    let fs_source = tail::RestartingTailer::new(
        ds_source_params,
        // TODO check for any conditions that require the tailer to restart
        |item| match item {
            Err(fs::cache::Error::Rescan) => {
                warn!("rescanning stream");
                true
            }
            _ => false,
        },
        |(watched_dirs, rules, lookback, fo_state_handles, fs_offsets)| {
            let watched_dirs = watched_dirs.clone();
            let rules = rules.clone();
            let lookback = lookback.clone();
            let deletion_ack_sender = deletion_ack_sender.clone();
            let fo_state_handles = fo_state_handles.clone();
            let fs_offsets = fs_offsets.clone();
            async move {
                let tailer = tail::Tailer::new(
                    watched_dirs,
                    rules,
                    lookback,
                    Some(fs_offsets.lock().await.clone()),
                    fo_state_handles,
                    deletion_ack_sender,
                );

                tail::process(tailer)
                    .expect("Failed to create FS Tailer")
                    .filter(move |line| {
                        let mut pair = (None, None);
                        if let Ok(line) = line {
                            pair = (line.get_key(), line.get_offset());
                        }

                        let fs_offsets = fs_offsets.clone();
                        async move {
                            if let (Some(key), Some(offsets)) = pair {
                                let mut span_vec = SpanVec::new();
                                if let Ok(offsets) = Span::try_from(offsets) {
                                    span_vec.insert(offsets);
                                    fs_offsets.lock().await.insert(FileId::from(key), span_vec);
                                }
                            }
                            true
                        }
                    })
            }
        },
        config.log.clear_cache_interval, // we restart tailer to clear fs cache
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
    debug!("Created fs_source");

    debug!("Creating k8s_source");
    let k8s_event_source: Option<_> = if let Some(fut) = k8s_event_stream.map(|e| e.event_stream())
    {
        Some(fut.await.map(StrictOrLazyLineBuilder::Strict))
    } else {
        None
    };
    debug!("Created k8s_source");

    debug!("Creating metrics_stats_source");
    let metric_stats_source: Option<_> =
        if let Some(fut) = metric_stats_stream.map(|e| e.start_metrics_call_task()) {
            Some(
                // There is only a receiver if we aren't the leader.
                stream::unfold(metrics_stream_feature_meta, |feature_meta| async move {
                    if let Some(feature_meta) = feature_meta {
                        if !feature_meta.is_starting_leader {
                            loop {
                                sleep(Duration::from_secs(
                                    (feature_meta.interval as u64)
                                        + (rand::thread_rng().gen_range(0..=5) * 10),
                                ))
                                .await;

                                let result = feature_meta.leader.try_claim_feature_leader().await;

                                if result {
                                    break;
                                }
                            }
                        }
                    }

                    None
                })
                .chain(fut.await)
                .filter_map(|x| async { Some(x) })
                .map(StrictOrLazyLineBuilder::Strict),
            )
        } else {
            None
        };
    debug!("Created metrics_stats_source");

    pin_mut!(fs_source);
    pin_mut!(k8s_event_source);

    #[cfg(target_os = "linux")]
    pin_mut!(journalctl_source);

    pin_mut!(tailer_source);

    #[cfg(all(feature = "libjournald", target_os = "linux"))]
    pin_mut!(journald_source);

    pin_mut!(metric_stats_source);

    pin_mut!(delayed_lines_source);

    let mut delayed_lines_source: Option<std::pin::Pin<&mut _>> = delayed_lines_source.as_pin_mut();

    let mut k8s_event_source: Option<std::pin::Pin<&mut _>> = k8s_event_source.as_pin_mut();
    #[cfg(target_os = "linux")]
    let mut journalctl_source: Option<std::pin::Pin<&mut _>> = journalctl_source.as_pin_mut();

    #[cfg(all(feature = "libjournald", target_os = "linux"))]
    let mut journald_source: Option<std::pin::Pin<&mut _>> = journald_source.as_pin_mut();

    let mut tailer_source: Option<std::pin::Pin<&mut _>> = tailer_source.as_pin_mut();

    let mut metric_server_source: Option<std::pin::Pin<&mut _>> = metric_stats_source.as_pin_mut();

    let mut sources: futures::stream::SelectAll<&mut (dyn Stream<Item = _> + Unpin)> =
        futures::stream::SelectAll::new();

    info!("Enabling filesystem");
    sources.push(&mut fs_source);

    #[cfg(all(feature = "libjournald", target_os = "linux"))]
    if let Some(s) = journald_source.as_mut() {
        info!("Enabling journald event source");
        sources.push(s)
    } else if let Some(s) = journalctl_source.as_mut() {
        info!("Enabling journalctl event source");
        sources.push(s)
    }
    #[cfg(all(not(feature = "libjournald"), target_os = "linux"))]
    if let Some(s) = journalctl_source.as_mut() {
        info!("Enabling journalctl event source");
        sources.push(s)
    }

    let mut delayed_source_enabled = false;
    let metadata_retry_delay = config.log.metadata_retry_delay;
    if let Some(s) = delayed_lines_source.as_mut() {
        info!("Enabling delayed lines source");
        delayed_source_enabled = true;
        sources.push(s)
    }

    if let Some(s) = tailer_source.as_mut() {
        info!("Enabling api tailer source");
        sources.push(s)
    }

    if let Some(k) = k8s_event_source.as_mut() {
        info!("Enabling k8s_event_source");
        sources.push(k)
    };

    if let Some(k) = metric_server_source.as_mut() {
        info!("Enabling metrics_server_watcher");
        sources.push(k)
    };

    let lines_stream = sources.map(|line|
        {
            Metrics::middleware().increment_lines_ingress();
            match line {
                // Strict lines (concrete)
                StrictOrLazyLineBuilder::Strict(mut line) => match executor.process(&mut line) {
                    Ok(_) => match line.build() {
                        Ok(line) => Some(StrictOrLazyLines::Strict(line)),
                        Err(e) => {
                            rate_limit!(rate = 1, interval = 1 * 60, {
                                error!("Couldn't build line from linebuilder {:?}", e);
                            });
                            None
                        }
                    },
                    Err(MiddlewareError::Skip(name)) => {
                        debug!(
                            "Skipping line by {} at [{}:{}]: {:?}",
                            name,
                            file!(),
                            line!(),
                            line
                        );
                        None
                    }
                    Err(e) => {
                        rate_limit!(rate = 1, interval = 10 * 60, {
                                error!(
                                    "Unexpected error - skipping line by {:?} at [{}:{}]: {:?}",
                                    e,
                                    file!(),
                                    line!(),
                                    line
                            )});
                        None
                    }
                },
                // Lazy lines (from tailed files, not fetched yet)
                StrictOrLazyLineBuilder::Lazy(mut line) => {
                    match executor.validate(&line) {
                        Ok(_) => match executor.process(&mut line) {
                            Ok(_) => Some(StrictOrLazyLines::Lazy(line)),
                            Err(MiddlewareError::Skip(name)) => {
                                debug!(
                                    "Skipping line by {} at [{}:{}]: {:?}",
                                    name,
                                    file!(),
                                    line!(),
                                    line
                                );
                                Metrics::middleware().increment_lines_ignored();
                                None
                            }
                            Err(e) => {
                                rate_limit!(rate = 1, interval = 10 * 60, {
                                    error!(
                                        "Unexpected error - skipping line by {:?} at [{}:{}]: {:?}",
                                        e,
                                        file!(),
                                        line!(),
                                        line
                                )});
                                Metrics::middleware().increment_lines_ignored();
                                None
                            }
                        },
                        Err(MiddlewareError::Skip(name)) => {
                            debug!(
                                "Skipping line by {} at [{}:{}]: {:?}",
                                name,
                                file!(),
                                line!(),
                                line
                            );
                            Metrics::middleware().increment_lines_ignored();
                            None
                        }
                        Err(MiddlewareError::Retry(name)) => {
                            if delayed_source_enabled {
                                rate_limit!(rate = 1, interval = 10 * 60, {
                                    warn!("Pod metadata is missing for line (retries=1): {:?}", line);
                                    // here we delay all pod lines to catchup with k8s pod metadata if delay os configured
                                    debug!(
                                        "Retrying - delaying line processing by {} for {:?} seconds at [{}:{}]: {:?}",
                                        name,
                                        metadata_retry_delay,
                                        file!(),
                                        line!(),
                                        line
                                    );
                                });
                                delayed_lines_send.send_blocking(line).unwrap();
                                Metrics::middleware().increment_lines_delayed();
                                None
                            } else {
                                rate_limit!(rate = 1, interval = 10 * 60, {
                                    error!("Pod metadata is missing for line (retries=disabled): {:?}", line);
                                    debug!(
                                        "Retrying disabled, processing line by {} AS-IS at [{}:{}]: {:?}",
                                        name,
                                        file!(),
                                        line!(),
                                        line
                                    );
                                });
                                Metrics::middleware().increment_lines_no_k8s_meta();
                                Some(StrictOrLazyLines::Lazy(line))
                            }
                        }
                    }
                }
                // Lazy already delayed/retried lines (from tailed files, not fetched yet)
                StrictOrLazyLineBuilder::LazyDelayed(mut line) => {
                    match executor.validate(&line) {
                        Ok(_) => match executor.process(&mut line) {
                            Ok(_) => Some(StrictOrLazyLines::Lazy(line)),
                            Err(MiddlewareError::Skip(name)) => {
                                debug!("Skipping line by {} at [{}:{}]: {:?}", name, file!(), line!(), line);
                                Metrics::middleware().increment_lines_ignored();
                                None
                            }
                            Err(e) => {
                                rate_limit!(rate = 1, interval = 10 * 60, {
                                    error!(
                                        "Unexpected error - skipping line by {:?} at [{}:{}]: {:?}",
                                        e,
                                        file!(),
                                        line!(),
                                        line
                                    )
                                });
                                Metrics::middleware().increment_lines_ignored();
                                None
                            }
                        },
                        Err(MiddlewareError::Skip(name)) => {
                            debug!("Skipping line by {} at [{}:{}]: {:?}", name, file!(), line!(), line);
                            Metrics::middleware().increment_lines_ignored();
                            None
                        }
                        Err(MiddlewareError::Retry(name)) => {
                            // no more retries
                            rate_limit!(rate = 1, interval = 10 * 60, {
                                error!("Pod metadata is missing for line (retries=0): {:?}", line);
                                debug!(
                                    "Retries exhausted - processing line by {} AS-IS at [{}:{}]: {:?}",
                                    name,
                                    file!(),
                                    line!(),
                                    line
                                );
                            });
                            match executor.process(&mut line) {
                                Ok(_) => Some(StrictOrLazyLines::Lazy(line)),
                                Err(MiddlewareError::Skip(name)) => {
                                    debug!("Skipping line by {} at [{}:{}]: {:?}", name, file!(), line!(), line);
                                    Metrics::middleware().increment_lines_ignored();
                                    None
                                }
                                Err(e) => {
                                    rate_limit!(rate = 1, interval = 10 * 60, {
                                    error!(
                                        "Unexpected error - skipping line by {:?} at [{}:{}]: {:?}",
                                        e,
                                        file!(),
                                        line!(),
                                        line
                                    )
                                });
                                    Metrics::middleware().increment_lines_ignored();
                                    None
                                }
                            }
                        }
                    }
                }
            }
        });

    let body_offsets_stream = lines_stream
        .filter_map(|l| async {
            Metrics::middleware().increment_lines_egress();
            l
        })
        // TODO: parameterize the flush frequency
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
                    error!("bad http request: {}", s);
                }
            }
            ClientError::Http(e) => {
                error!("failed sending http request: {}", e);
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
                rate_limit!(rate = 1, interval = 1 * 60, {
                    warn!("failed sending http request, retrying: {}", e);
                });
            }
            SendStatus::RetryServerError(code, e) => {
                rate_limit!(rate = 1, interval = 1 * 60, {
                    warn!("failed sending http request, retrying: {} {}", code, e);
                });
            }
            SendStatus::RetryTimeout => {
                rate_limit!(rate = 1, interval = 1 * 60, {
                    warn!("failed sending http request, retrying: request timed out!");
                });
            }
            SendStatus::Sent => {}
        }
    }

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
                        Err(e) => error!("Couldn't batch lines in lines_driver: {:?}", e),
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
                            Err(http::retry::Error::Io(e))
                                if e.kind() == std::io::ErrorKind::NotFound =>
                            {
                                debug!("Couldn't batch lines in retry_stream: ignoring - {:?}", e)
                            }
                            Err(e) => {
                                error!("Couldn't batch lines in retry_stream: {:?}", e);
                                if e.to_string().contains("Too many open files")
                                    || e.to_string().contains("No file descriptors available")
                                {
                                    error!(
                            "Agent process has hit the limit of maximum number of open files. \
                            Try to increase the Open Files system limit."
                                    );
                                    std::process::exit(consts::exit_codes::EMFILE);
                                }
                            }
                        }
                    }
                })
                .await
                .expect("Join Error")
            }
        });

    // Concurrently run the line streams and listen for the `shutdown` signal
    tokio::select! {
        _ = lines_driver => {}
        _ = retry_driver => {}
        _ = &mut shutdown_rx => {
            info!("Received shutdown request")
        }
        signal_name = get_signal() => {
            info!("Received {} signal, shutting down", signal_name)
        }
    }
    Ok(())
}

async fn set_up_leader(
    namespace: &Option<String>,
    k8s_client: &Kube_Client,
    pod_name: &Option<String>,
    feature: String,
) -> FeatureLeaderMeta {
    let lease_api =
        k8s::lease::get_k8s_lease_api(&namespace.clone().unwrap_or_default(), k8s_client.clone())
            .await;
    let leader = FeatureLeader::new(
        namespace.clone().unwrap_or_default(),
        feature,
        pod_name.clone().unwrap_or_default(),
        lease_api,
    );

    let is_starting_leader = leader.try_claim_feature_leader().await;
    let leader_lease = leader.get_feature_leader_lease().await;
    let mut interval = DEFAULT_CHECK_FOR_LEADER_S;
    if let Ok(lease) = leader_lease {
        interval = lease
            .spec
            .map(|s| s.lease_duration_seconds.unwrap_or(0i32))
            .unwrap_or(0i32);

        if interval == 0 {
            interval = DEFAULT_CHECK_FOR_LEADER_S;
        }
    }
    FeatureLeaderMeta {
        interval,
        leader: leader.clone(),
        is_starting_leader,
    }
}

async fn check_startup_lease_status(
    start_option: Option<&K8sLeaseConf>,
    claimed_lease_ref: &mut Option<String>,
    client: Kube_Client,
) {
    let max_attempts = match start_option {
        Some(K8sLeaseConf::Attempt) => {
            info!("Getting agent-startup-lease (making limited attempts)");
            K8S_STARTUP_LEASE_RETRY_ATTEMPTS
        }
        Some(K8sLeaseConf::Always) => {
            info!("Getting agent-startup-lease (trying forever)");
            -1
        }
        _ => {
            info!(
                "Kubernetes cluster initialized, K8s startup lease set to: {:?}",
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
                info!("No lease available at this time. Waiting 1 second...");
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

    tokio::select! {
        _ = interrupt_signal.recv() => { "SIGINT" }
        _ = quit_signal.recv() => { "SIGQUIT"  }
        _ = term_signal.recv() => { "SIGTERM" }
    }
}

#[cfg(windows)]
async fn get_signal() -> &'static str {
    ctrl_c().await.unwrap();
    "CTRL+C"
}

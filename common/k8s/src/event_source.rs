use crate::errors::K8sEventStreamError;
use crate::feature_leader::{FeatureLeader, FeatureLeaderMeta};
use crate::restarting_stream::{RequiresRestart, RestartingStream};
use backoff::ExponentialBackoff;
use chrono::{Duration, Utc};
use core::time;
use crossbeam::atomic::AtomicCell;
use futures::{stream::try_unfold, Stream, StreamExt, TryStreamExt};
use http::types::body::LineBuilder;
use k8s_openapi::api::core::v1::{Event, ObjectReference, Pod};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::Time;
use kube::ResourceExt;
use kube::{
    runtime::{watcher, WatchStreamExt},
    Api, Client,
};
use metrics::Metrics;
use pin_utils::pin_mut;
use rand::Rng;
use regex::Regex;
use serde::Serialize;
use std::convert::TryInto;
use std::convert::{Into, TryFrom};
use std::num::NonZeroI64;
use std::sync::Arc;
use tokio::time::sleep;
use tracing::{debug, info, warn};

pub static K8S_EVENTS_LEASE_NAME: &str = "logdna-agent-k8-events-lease";

lazy_static! {
    static ref APP_REGEX: Regex = {
        match Regex::new(r"\{(.+?)\}") {
            Ok(regex) => regex,
            Err(e) => panic!("Unable to compile kube event source's app regex: {:?}", e),
        }
    };
}

impl From<Event> for EventLog {
    // Replicate the Reporter's formatting
    fn from(event: Event) -> Self {
        let Event {
            type_,
            action,
            reason,
            message,
            count,
            source,
            involved_object,
            last_timestamp,
            first_timestamp,
            event_time,
            ..
        } = event;
        let ObjectReference {
            kind,
            name,
            namespace,
            field_path,
            ..
        } = involved_object;
        let (node, component) = if let Some(s) = source {
            (s.host, s.component)
        } else {
            (None, None)
        };

        let (host, app) = if let Some(kind) = kind.as_ref() {
            if kind == "Pod" {
                (
                    name.clone(),
                    field_path.as_ref().and_then(|field_path| {
                        APP_REGEX.captures(field_path).and_then(|captured| {
                            captured.get(1).map(|app| app.as_str().to_string())
                        })
                    }),
                )
            } else if let Some(ref name) = name {
                (Some(format!("{}/{}", kind, name)), None)
            } else {
                (None, None)
            }
        } else {
            (None, None)
        };
        let app = app.or_else(|| component.clone());

        let age = match (last_timestamp.as_ref(), first_timestamp.as_ref()) {
            (Some(last), Some(first)) if last >= first => Some(last.0 - first.0),
            _ => None,
        };

        let duration = age.map(|age| {
            if age > Duration::weeks(0) {
                let age_val = age.to_std().map(|d|humantime::format_duration(d).to_string()).unwrap(/*Safe to unwrap as we checked it's positive*/);
                format!("over {}", age_val)
            } else {
                "just now".to_string()
            }
        });

        let default = "Normal".to_string();
        let log_level = type_.clone().unwrap_or(default);

        let line = EventLogLine {
            message: match (
                reason.as_ref(),
                count.as_ref(),
                duration.as_ref(),
                event_time.is_some(),
                message.as_ref(),
            ) {
                (Some(r), Some(c), Some(d), _, Some(m)) => {
                    Some(format!("{}  (x{} {})  {}", r, c, d, m))
                }
                (Some(r), None, None, true, Some(m)) => Some(format!("{}  {}", r, m)),
                _ => None,
            },
            level: log_level,
            kube: EventLogLineInner {
                type_: "event".to_string(),
                action,
                resource: kind,
                name,
                namespace,
                reason,
                message,
                component,
                node,
                first_time: first_timestamp,
                time: last_timestamp.or_else(|| event_time.map(|time| Time(time.0))),
                age: age.and_then(|age| {
                    age.num_seconds()
                        .try_into()
                        .map_err(|_| warn!("age too large, could not fit {} into i32", age))
                        .ok()
                }),
                count,
            },
        };

        EventLog {
            line,
            host,
            app,
            level: type_,
        }
    }
}

#[derive(Serialize, Debug)]
struct EventLogLine {
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    level: String,
    kube: EventLogLineInner,
}

#[derive(Serialize, Debug)]
struct EventLogLineInner {
    #[serde(rename = "type")]
    type_: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    action: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    resource: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    component: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    node: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    first_time: Option<Time>,
    #[serde(skip_serializing_if = "Option::is_none")]
    time: Option<Time>,
    #[serde(skip_serializing_if = "Option::is_none")]
    age: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    count: Option<i32>,
}

// TODO test from...
struct EventLog {
    line: EventLogLine,
    host: Option<String>,
    app: Option<String>,
    level: Option<String>,
}

impl TryFrom<EventLog> for LineBuilder {
    type Error = K8sEventStreamError;

    fn try_from(value: EventLog) -> Result<Self, Self::Error> {
        serde_json::to_string(&value.line)
            .map_err(K8sEventStreamError::SerializationError)
            .map(|e| {
                debug!("logging event: {}", e);
                let mut line = LineBuilder::new().line(e);
                if let Some(host) = &value.host {
                    line = line.host(host);
                }
                if let Some(app) = &value.app {
                    line = line.app(app);
                }
                if let Some(level) = &value.level {
                    line = line.level(level);
                }
                line
            })
    }
}

pub struct K8sEventStream {
    pub client: Client,
    leader_meta: Arc<FeatureLeaderMeta>,
    pod_name: String,
    namespace: String,
    pod_label: String,
}

pub enum StreamElem<T> {
    Waiting,
    Event(T),
}

impl K8sEventStream {
    pub fn new(
        client: Client,
        pod_name: String,
        namespace: String,
        pod_label: String,
        leader_meta: Arc<FeatureLeaderMeta>,
    ) -> Self {
        Self {
            client,
            leader_meta,
            pod_name,
            namespace,
            pod_label,
        }
    }

    fn waiter_stream<T>(
        pod_name: impl Into<String>,
        namespace: impl Into<String>,
        pod_label: impl Into<String>,
        client: Arc<Client>,
        delete_time: Arc<AtomicCell<Option<NonZeroI64>>>,
        leader_meta: Arc<FeatureLeaderMeta>,
    ) -> impl Stream<Item = Result<StreamElem<T>, K8sEventStreamError>> {
        let pod_name = pod_name.into();
        let namespace = namespace.into();
        let pod_label = pod_label.into();

        let waiter = move |_| {
            let pod_name = pod_name.clone();
            let namespace = namespace.clone();
            let pod_label = pod_label.clone();
            let client = client.clone();
            let leader_meta = leader_meta.clone();

            let delete_time = delete_time.clone();
            let pods: Api<Pod> = Api::namespaced(client.as_ref().clone(), &namespace);
            // subscribe to pod api and filter pods by POD_APP_LABEL

            #[allow(clippy::map_flatten)]
            async move {
                // Find the oldest pod of the latest generation
                enum Cont {
                    Cont,
                    Break,
                }

                let leader_pod = get_leader_pod_name(&leader_meta).await;

                if leader_pod == pod_name {
                    info!("begin logging k8s events");
                    start_renewal_task(leader_meta.leader.clone());
                    Ok(None)
                } else {
                    let stream = watcher(
                        pods.clone(),
                        kube::runtime::watcher::Config {
                            label_selector: Some(format!("app.kubernetes.io/name={}", &pod_label)),
                            timeout: Some(30),
                            ..Default::default()
                        },
                    )
                    .skip_while(|e| {
                        let matched = matches!(e, Ok(watcher::Event::<Pod>::Restarted(_)));
                        async move { matched }
                    })
                    .map({
                        move |e| match e {
                            Ok(watcher::Event::Deleted(e)) => {
                                if let Some(name) = e.metadata.name {
                                    info!("Agent Down {}", name);

                                    futures::executor::block_on(check_for_leader(
                                        leader_meta.clone(),
                                        name,
                                    ));

                                    delete_time.store(
                                        e.metadata
                                            .deletion_timestamp
                                            .map(|t| {
                                                info!(
                                                    "Ignoring k8s events before {}",
                                                    t.0 - chrono::Duration::seconds(2)
                                                );
                                                NonZeroI64::new(t.0.timestamp() - 2)
                                            })
                                            .flatten(),
                                    );
                                }
                                Cont::Break
                            }
                            _ => Cont::Cont,
                        }
                    })
                    .filter_map(|e| async move {
                        match e {
                            Cont::Cont => None,
                            Cont::Break => Some(((), ())),
                        }
                    });
                    pin_mut!(stream);
                    Ok(stream.next().await)
                }
            }
        };

        let waiter = Arc::new(waiter);

        try_unfold((), {
            let waiter = waiter.clone();
            move |_| {
                let waiter = waiter.clone();
                backoff::future::retry(ExponentialBackoff::default(), move || waiter(()))
            }
        })
        .map(|r: Result<(), K8sEventStreamError>| match r {
            Ok(_) => Ok(StreamElem::Waiting),
            Err(e) => Err(e),
        })
    }

    pub fn active_stream(
        client: Arc<Client>,
        latest_event_time: Arc<AtomicCell<Option<NonZeroI64>>>,
        previous_event_logger_delete_time: Arc<AtomicCell<Option<NonZeroI64>>>,
    ) -> impl Stream<Item = Result<StreamElem<LineBuilder>, K8sEventStreamError>> {
        let events: Api<Event> = Api::all(client.as_ref().clone());

        let latest_event_time_w = latest_event_time.clone();
        watcher(events, kube::runtime::watcher::Config::default())
            .touched_objects()
            .map_err(K8sEventStreamError::WatcherError)
            .filter({
                move |event| {
                    let latest_event_time = latest_event_time.clone();

                    let earliest = previous_event_logger_delete_time.clone();

                    let ret = latest_event_time
                        .load()
                        .or_else(|| earliest.load())
                        .and_then(|earliest| {
                            let earliest = chrono::DateTime::from_timestamp(earliest.into(), 0)
                                .expect("Timestamp Out of Range");

                            event.as_ref().ok().and_then(|e| {
                                if e.creation_timestamp().is_none()
                                    && latest_event_time.load().is_none()
                                {
                                    return Some(false);
                                }

                                e.creation_timestamp().as_ref().map(|l| earliest < l.0)
                            })
                        });

                    async move { ret.unwrap_or(true) }
                }
            })
            .map(move |event| {
                let event_time = event.map(|e| {
                    let latest_event_time = latest_event_time_w.clone();
                    let this_event_time = e
                        .creation_timestamp()
                        .as_ref()
                        .and_then(|t| NonZeroI64::new(t.0.timestamp() - 2));

                    let ret = LineBuilder::try_from(EventLog::from(e)).map(|l| {
                        Metrics::k8s().increment_lines();
                        l
                    });

                    if ret.is_ok() && this_event_time.is_some() {
                        latest_event_time.store(this_event_time);
                    };
                    ret
                });
                match event_time {
                    Ok(Ok(l)) => Ok(StreamElem::Event(l)),
                    Ok(Err(e)) => {
                        latest_event_time_w.store(NonZeroI64::new(Utc::now().timestamp()));
                        Err(e)
                    }
                    Err(e) => {
                        latest_event_time_w.store(NonZeroI64::new(Utc::now().timestamp()));
                        Err(e)
                    }
                }
            })
    }

    pub async fn create_stream(
        pod_name: impl Into<String>,
        namespace: impl Into<String>,
        pod_label: impl Into<String>,
        client: Arc<Client>,
        latest_event_time: Arc<AtomicCell<Option<NonZeroI64>>>,
        leader_meta: Arc<FeatureLeaderMeta>,
    ) -> impl Stream<Item = Result<LineBuilder, K8sEventStreamError>> {
        let pod_name = pod_name.into();
        let namespace = namespace.into();
        let previous_event_logger_delete_time: Arc<AtomicCell<Option<NonZeroI64>>> =
            Arc::new(AtomicCell::new(None));

        // Retry is handled internally with exponential backoff
        let waiting_stream = K8sEventStream::waiter_stream(
            pod_name,
            namespace,
            pod_label,
            client.clone(),
            previous_event_logger_delete_time.clone(),
            leader_meta,
        );

        let event_stream = K8sEventStream::active_stream(
            client,
            latest_event_time,
            previous_event_logger_delete_time,
        );

        waiting_stream.chain(event_stream).filter_map(|e| async {
            match e {
                Ok(StreamElem::Event(l)) => Some(Ok(l)),
                Ok(StreamElem::Waiting) => None,
                Err(e) => Some(Err(e)),
            }
        })
    }

    pub async fn event_stream(self) -> impl Stream<Item = LineBuilder> {
        let client = std::sync::Arc::new(self.client.clone());

        let latest_event_time: Arc<AtomicCell<Option<NonZeroI64>>> =
            Arc::new(AtomicCell::new(None));
        let pod_name = self.pod_name.clone();
        let namespace = self.namespace.clone();
        let pod_label = self.pod_label.clone();

        let _latest_event_time = latest_event_time.clone();
        let _client = client.clone();
        let start_stream = move || {
            K8sEventStream::create_stream(
                pod_name.clone(),
                namespace.clone(),
                pod_label.clone(),
                _client.clone(),
                _latest_event_time.clone(),
                self.leader_meta.clone(),
            )
        };

        let restarting_stream = RestartingStream::new(start_stream, |e| match e {
            Err(K8sEventStreamError::WatcherError(_)) => {
                warn!("Restarting Stream");
                RequiresRestart::Yes
            }
            _ => RequiresRestart::No,
        });

        restarting_stream.await.filter_map(|e| async { e.ok() })
    }
}

async fn get_leader_pod_name(leader_meta: &Arc<FeatureLeaderMeta>) -> String {
    let lease = leader_meta.leader.get_feature_leader_lease().await;

    if let Ok(lease) = lease {
        if let Some(spec) = lease.spec {
            if let Some(holder) = spec.holder_identity {
                return holder;
            }
        }
    }

    String::new()
}

fn start_renewal_task(leader: FeatureLeader) {
    tokio::spawn(async move {
        loop {
            sleep(time::Duration::from_secs(15)).await;
            let result = leader.renew_feature_leader().await;

            // lost leader
            if !result {
                break;
            }
        }
    });
}

async fn check_for_leader(leader_meta: Arc<FeatureLeaderMeta>, deleted_pod: String) {
    loop {
        sleep(time::Duration::from_millis(
            20000 + (rand::thread_rng().gen_range(1000..=10000)),
        ))
        .await;

        let leader_pod = get_leader_pod_name(&leader_meta).await;

        if deleted_pod != leader_pod {
            break;
        }

        let result = leader_meta.leader.try_claim_feature_leader().await;

        if result {
            start_renewal_task(leader_meta.leader.clone());
            break;
        } else {
            let current_lease_name = get_leader_pod_name(&leader_meta).await;

            // another pod grabbed the lease
            if leader_pod != current_lease_name {
                break;
            }
        }
    }
}

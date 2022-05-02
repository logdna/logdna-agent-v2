use core::cmp::{Ord, Ordering};
use std::convert::TryInto;
use std::convert::{Into, TryFrom};
use std::num::NonZeroI64;
use std::sync::Arc;

use backoff::ExponentialBackoff;
use crossbeam::atomic::AtomicCell;

use chrono::Duration;

use futures::{stream::try_unfold, Stream, StreamExt, TryStreamExt};

use k8s_openapi::api::core::v1::{Event, ObjectReference, Pod};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::Time;
use kube::api::ListParams;
use kube::{
    runtime::{utils::try_flatten_touched, watcher},
    Api, Client, Config,
};

use pin_utils::pin_mut;

use serde::Serialize;

use http::types::body::LineBuilder;

use metrics::Metrics;

use crate::errors::{K8sError, K8sEventStreamError};

use crate::restarting_stream::{RequiresRestart, RestartingStream};

use regex::Regex;

const CONTAINER_NAME: &str = "logdna-agent";

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
                age.to_std().map(|d|humantime::format_duration(d).to_string()).unwrap(/*Safe to unwrap as we checked it's positive*/)
            } else {
                "just now".to_string()
            }
        });

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
    pod_name: String,
    namespace: String,
    pod_label: String,
}

pub enum StreamElem<T> {
    Waiting,
    Event(T),
}

impl K8sEventStream {
    pub fn new(client: Client, pod_name: String, namespace: String, pod_label: String) -> Self {
        Self {
            client,
            pod_name,
            namespace,
            pod_label,
        }
    }

    pub fn try_default(
        pod_name: String,
        namespace: String,
        pod_label: String,
    ) -> Result<Self, K8sError> {
        let config = match Config::from_cluster_env() {
            Ok(v) => v,
            Err(e) => {
                return Err(K8sError::InitializationError(format!(
                    "unable to get cluster configuration info: {}",
                    e
                )))
            }
        };
        Ok(Self::new(
            Client::try_from(config)?,
            pod_name,
            namespace,
            pod_label,
        ))
    }

    #[allow(clippy::map_flatten)]
    async fn get_oldest_pod(
        api: Api<Pod>,
        label: &str,
    ) -> Result<Option<String>, K8sEventStreamError> {
        let lp = ListParams::default().labels(&format!("app.kubernetes.io/name={}", label)); // filter instances by label
        let pod_list = api.list(&lp).await.map_err(K8sEventStreamError::K8sError)?;
        let oldest_post = pod_list
            .iter()
            .filter_map(|p| -> Option<(Option<u64>, &Time, &str)> {
                if let (pod_gen, Some(pod_started_at), Some(pod_name)) =
                    // get pod generation, it will be there if there is an update strategy
                    (
                        p.metadata
                            .labels
                            .as_ref()
                            .map(|l| {
                                l.get("pod-template-generation")
                                    .and_then(|g| g.parse::<u64>().ok())
                            })
                            .flatten(),
                        get_pod_started_at(p),
                        p.metadata.name.as_ref(),
                    )
                {
                    Some((pod_gen, pod_started_at, pod_name))
                } else {
                    None
                }
            })
            // Keeps oldest pod
            .reduce(
                move |(gen, started_at, name),
                      (o_gen, o_started_at, o_name)|
                      -> (Option<u64>, &Time, &str) {
                    // For our purposes later generations should sort earlier than later ones
                    if Ordering::reverse(Ord::cmp(&gen, &o_gen))
                        .then(Ord::cmp(started_at, o_started_at))
                        .then(Ord::cmp(name, o_name))
                        == Ordering::Less
                    {
                        (o_gen, o_started_at, o_name)
                    } else {
                        (gen, started_at, name)
                    }
                },
            )
            .map(|(_, _, name)| name.to_string());
        Ok(oldest_post)
    }

    fn waiter_stream<T>(
        pod_name: impl Into<String>,
        namespace: impl Into<String>,
        pod_label: impl Into<String>,
        client: Arc<Client>,
        delete_time: Arc<AtomicCell<Option<NonZeroI64>>>,
    ) -> impl Stream<Item = Result<StreamElem<T>, K8sEventStreamError>> {
        let pod_name = pod_name.into();
        let namespace = namespace.into();
        let pod_label = pod_label.into();

        let waiter = move |_| {
            let pod_name = pod_name.clone();
            let namespace = namespace.clone();
            let pod_label = pod_label.clone();
            let client = client.clone();

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

                if let Some(oldest_pod_name) =
                    K8sEventStream::get_oldest_pod(pods.clone(), &pod_label).await?
                {
                    if oldest_pod_name == pod_name {
                        info!("begin logging k8s events");
                        Ok(None)
                    } else {
                        info!("watching pod {}", oldest_pod_name);
                        let params = ListParams::default()
                            .timeout(30)
                            .labels(&format!("app.kubernetes.io/name={}", &pod_label)) // filter instances by label
                            .fields(&format!("metadata.name={}", oldest_pod_name)); // filter instances by label
                        let stream = watcher(pods.clone(), params)
                            .skip_while(|e| {
                                let matched = matches!(e, Ok(watcher::Event::<Pod>::Restarted(_)));
                                async move { matched }
                            })
                            .map({
                                move |e| match e {
                                    Ok(watcher::Event::Deleted(e)) => {
                                        info!("previous k8s event logger deleted");
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
                } else {
                    Ok(Some(((), ())))
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
        let params = ListParams::default();

        let latest_event_time_w = latest_event_time.clone();
        try_flatten_touched(watcher(events, params))
            .map_err(K8sEventStreamError::WatcherError)
            .filter({
                move |event| {
                    let latest_event_time = latest_event_time.clone();
                    let earliest = previous_event_logger_delete_time.clone();
                    let ret = latest_event_time
                        .load()
                        .or_else(|| earliest.load())
                        .and_then(|earliest| {
                            let earliest =
                                chrono::NaiveDateTime::from_timestamp(earliest.into(), 0);
                            event.as_ref().ok().and_then(|e| {
                                e.last_timestamp
                                    .as_ref()
                                    .map(|l| earliest < l.0.naive_utc())
                            })
                        });
                    async move { ret.unwrap_or(true) }
                }
            })
            .map(move |event| {
                match event.map(|e| {
                    let latest_event_time = latest_event_time_w.clone();
                    let this_event_time = e
                        .last_timestamp
                        .as_ref()
                        .and_then(|t| NonZeroI64::new(t.0.timestamp() - 2));

                    let ret = LineBuilder::try_from(EventLog::from(e)).map(|l| {
                        Metrics::k8s().increment_lines();
                        l
                    });
                    if ret.is_ok() {
                        latest_event_time.store(this_event_time)
                    };
                    ret
                }) {
                    Ok(Ok(l)) => Ok(StreamElem::Event(l)),
                    Ok(Err(e)) => Err(e),
                    Err(e) => Err(e),
                }
            })
    }

    pub async fn create_stream(
        pod_name: impl Into<String>,
        namespace: impl Into<String>,
        pod_label: impl Into<String>,
        client: Arc<Client>,
        latest_event_time: Arc<AtomicCell<Option<NonZeroI64>>>,
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

    pub async fn event_stream(self) -> Result<impl Stream<Item = LineBuilder> + Send, String> {
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
            )
        };

        let restarting_stream = RestartingStream::new(start_stream, |e| match e {
            Err(K8sEventStreamError::WatcherError(_)) => {
                warn!("Restarting Stream");
                RequiresRestart::Yes
            }
            _ => RequiresRestart::No,
        });

        Ok(restarting_stream.await.filter_map(|e| async { e.ok() }))
    }
}

fn get_pod_started_at(p: &Pod) -> Option<&Time> {
    // WTB do notation
    p.status.as_ref().and_then(|s| {
        s.container_statuses.as_ref().and_then(|cs| {
            cs.iter()
                .filter_map(|c| {
                    if c.name == CONTAINER_NAME && c.ready {
                        Some(c.state.as_ref().and_then(|st| {
                            st.running.as_ref().and_then(|rs| rs.started_at.as_ref())
                        }))
                    } else {
                        None
                    }
                })
                .next()
                .flatten()
        })
    })
}

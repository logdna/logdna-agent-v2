use crate::{
    create_k8s_client_default_from_env,
    middleware::{K8sMetadata, StoreSwapIntent},
};
use async_channel::Receiver;
use futures::StreamExt;
use hyper::http::header::HeaderValue;
use middleware::Executor;
use std::{cmp::min, sync::Arc, time::SystemTime};
use tokio::{
    task::JoinHandle,
    time::{sleep, Duration},
};
use tracing::{error, info, trace, warn};

pub static SWAP_DELAY: Duration = Duration::from_secs(3);
pub static RESET_DURATION: Duration = Duration::from_secs(60);
pub static INITIAL_BACKOFF_SECS: u32 = 2;
pub static MAX_BACKOFF_SECS: u32 = 30;

enum State<'a> {
    Init,
    Creating(u32),
    Kicking(u32, kube::Client),
    Swapping(u32, JoinHandle<()>, StoreSwapIntent<'a>),
    Running(u32, JoinHandle<()>),
}

async fn step_trampoline<'a>(
    state: State<'a>,
    user_agent: &'a HeaderValue,
    node_name: &'a Option<String>,
    middleware: &'a K8sMetadata,
) -> State<'a> {
    match state {
        State::Init => State::Creating(0),
        State::Creating(attempts) => {
            let middleware_k8s_client = create_k8s_client_default_from_env(user_agent.clone())
                .unwrap_or_else(|e| {
                    let message =
                        format!("unable to create client for k8s metadata watcher: {e:?}");
                    panic!("{}", message);
                });
            State::Kicking(attempts, middleware_k8s_client)
        }
        State::Kicking(attempts, client) => {
            let (thread_handle, swap_intent) = match middleware
                .kick_over(client, node_name.as_deref())
            {
                Ok((stream, swap_intent)) => {
                    trace!("k8s metadata watcher kicked over!");
                    (
                        tokio::spawn(async move {
                            trace!("k8s metadata watcher thread started, processing events!");
                            stream.for_each(|_| async {}).await;
                            trace!("k8s metadata watcher thread exiting!");
                        }),
                        swap_intent,
                    )
                }
                Err(e) => {
                    let message =
                        format!("The agent could not access k8s api after several attempts: {e}");
                    error!("{}", message);
                    panic!("{}", message);
                }
            };
            State::Swapping(attempts, thread_handle, swap_intent)
        }
        State::Swapping(attempts, thread_handle, swap_intent) => {
            if attempts != 0 {
                trace!("delaying k8s metadata watcher rotation so new store can populate...");
                sleep(SWAP_DELAY).await;
            }
            if let Err(e) = swap_intent.swap() {
                error!("k8s metadata watcher store swap failed: {}", e);
            };
            trace!("k8s metadata watcher store swapped!");
            State::Running(attempts, thread_handle)
        }
        State::Running(mut attempts, thread_handle) => {
            let start = SystemTime::now();
            if let Err(e) = thread_handle.await {
                warn!(
                    "encountered error when refreshing k8s metadata watcher: {:?}",
                    e
                )
            };
            trace!("k8s metadata watcher stream exhausted, recreating...");
            if let Ok(elapsed) = start.elapsed() {
                if elapsed >= RESET_DURATION {
                    attempts = 0;
                } else {
                    let sleep_secs = min(
                        INITIAL_BACKOFF_SECS
                            .checked_pow(attempts)
                            .unwrap_or(MAX_BACKOFF_SECS),
                        MAX_BACKOFF_SECS,
                    );
                    let sleep_duration = Duration::from_secs(u64::from(sleep_secs));
                    warn!(
                        "delaying k8s metadata watcher rotation with backoff of {} seconds",
                        sleep_secs
                    );
                    sleep(sleep_duration).await;
                }
            }

            State::Creating(attempts + 1)
        }
    }
}

async fn trampoline<'a>(
    mut state: State<'a>,
    user_agent: &'a HeaderValue,
    node_name: &'a Option<String>,
    middleware: &'a K8sMetadata,
) {
    loop {
        state = step_trampoline(state, user_agent, node_name, middleware).await;
    }
}

pub fn metadata_runner(
    deletion_ack_receiver: Receiver<Vec<std::path::PathBuf>>,
    user_agent: HeaderValue,
    node_name: Option<String>,
    executor: &mut Executor,
) {
    let middleware = Arc::new(K8sMetadata::new(deletion_ack_receiver));
    executor.register(middleware.clone());
    info!("registered middleware: K8sMetadata");
    tokio::spawn(async move {
        trampoline(State::Init, &user_agent, &node_name, &middleware).await;
    });
}

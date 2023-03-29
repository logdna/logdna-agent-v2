use crate::{
    create_k8s_client_default_from_env,
    middleware::{K8sMetadata, StoreSwapIntent},
};
use async_channel::Receiver;
use futures::StreamExt;
use hyper::http::header::HeaderValue;
use middleware::Executor;
use std::sync::Arc;
use tokio::{
    task::JoinHandle,
    time::{sleep, Duration},
};
use tracing::{error, trace, warn};

pub static K8S_WATCHER_ROTATION_DELAY: Duration = Duration::from_secs(3);

enum State<'a> {
    Init,
    Creating(bool),
    Kicking(bool, kube::Client),
    Swapping(bool, JoinHandle<()>, StoreSwapIntent<'a>),
    Running(JoinHandle<()>),
}

async fn step_trampoline<'a>(
    state: State<'a>,
    user_agent: &'a HeaderValue,
    node_name: &'a Option<String>,
    middleware: &'a K8sMetadata,
) -> State<'a> {
    match state {
        State::Init => State::Creating(true),
        State::Creating(first) => {
            let middleware_k8s_client = create_k8s_client_default_from_env(user_agent.clone());
            if let Err(e) = middleware_k8s_client {
                let message = format!("unable to create client for k8s metadata watcher: {:?}", e);
                error!("{}", message);
                panic!("{}", message);
            }
            State::Kicking(first, middleware_k8s_client.unwrap())
        }
        State::Kicking(first, client) => {
            let (thread_handle, swap_intent) =
                match middleware.kick_over(client, node_name.as_deref()) {
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
                        let message = format!(
                            "The agent could not access k8s api after several attempts: {}",
                            e
                        );
                        error!("{}", message);
                        panic!("{}", message);
                    }
                };
            State::Swapping(first, thread_handle, swap_intent)
        }
        State::Swapping(first, thread_handle, swap_intent) => {
            if !first {
                trace!("delaying k8s metadata watcher rotation so new store can populate...");
                sleep(K8S_WATCHER_ROTATION_DELAY).await;
            }
            if let Err(e) = swap_intent.swap() {
                error!("k8s metadata watcher store swap failed: {}", e);
            };
            trace!("k8s metadata watcher store swapped!");
            State::Running(thread_handle)
        }
        State::Running(thread_handle) => {
            if let Err(e) = thread_handle.await {
                warn!(
                    "encountered error when refreshing k8s metadata watcher: {:?}",
                    e
                )
            };
            trace!("k8s metadata watcher stream exhausted, recreating...");
            State::Creating(false)
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

    tokio::spawn(async move {
        trampoline(State::Init, &user_agent, &node_name, &middleware).await;
    });
}

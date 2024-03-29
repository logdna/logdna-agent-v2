use std::collections::HashMap;
use std::net::{SocketAddr, ToSocketAddrs};

use futures::{AsyncBufReadExt, StreamExt, TryStreamExt};
use k8s_openapi::api::apps::v1::DaemonSet;
use k8s_openapi::api::coordination::v1::Lease;
use k8s_openapi::api::core::v1::{Endpoints, Namespace, Pod, Service, ServiceAccount};
use k8s_openapi::api::rbac::v1::{ClusterRole, ClusterRoleBinding, Role, RoleBinding};
use kube::api::{Api, ListParams, LogParams, PostParams, WatchEvent, WatchParams};
use kube::Client;

use test_log::test;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tracing::{debug, info};

use k8s::feature_leader::FeatureLeader;

// workaround for unused functions in different features: https://github.com/rust-lang/rust/issues/46379
use crate::common;

async fn print_pod_logs(client: Client, namespace: &str, label: &str) {
    let pods: Api<Pod> = Api::namespaced(client, namespace);
    let lp = ListParams::default().labels(label);
    pods.list(&lp).await.unwrap().into_iter().for_each(|p| {
        let pods = pods.clone();
        tokio::spawn({
            async move {
                let mut logs = pods
                    .log_stream(
                        &p.metadata.name.clone().unwrap(),
                        &LogParams {
                            follow: true,
                            tail_lines: None,
                            ..LogParams::default()
                        },
                    )
                    .await
                    .unwrap()
                    .lines();

                debug!("Logging agent pod {:?}", p.metadata.name);
                while let Some(line) = logs.try_next().await.unwrap() {
                    debug!("LOG [{:?}] {:?}", p.metadata.name, line);
                }
            }
        });
    })
}

async fn wait_for_pod_ready(
    client: Client,
    pod_name: &str,
    namespace: &str,
    timeout: std::time::Duration,
) -> bool {
    let pods: Api<k8s_openapi::api::core::v1::Pod> = Api::namespaced(client, namespace);
    let mut is_ready = false;
    let start_time = std::time::Instant::now();
    while !is_ready && start_time.elapsed() < timeout {
        match pods.get(pod_name).await {
            Ok(pod) => {
                if let Some(status) = &pod.status {
                    if let Some(conditions) = &status.conditions {
                        for condition in conditions {
                            if condition.type_ == "Ready" && condition.status == "True" {
                                is_ready = true;
                                break;
                            }
                        }
                    }
                }
            }
            Err(e) => {
                panic!("Error getting pod: {:?}", e);
            }
        }
        if !is_ready {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
    }
    is_ready
}

async fn wait_for_ds_ready(
    client: Client,
    ds_name: &str,
    namespace: &str,
    timeout: std::time::Duration,
) -> bool {
    let daemonsets: Api<k8s_openapi::api::apps::v1::DaemonSet> =
        Api::namespaced(client.clone(), namespace);
    let mut is_ready = false;
    let start_time = std::time::Instant::now();
    while !is_ready && start_time.elapsed() < timeout {
        match daemonsets.get(ds_name).await {
            Ok(ds) => {
                if let Some(status) = ds.status {
                    if status.number_ready == status.current_number_scheduled {
                        is_ready = true;
                    }
                }
            }
            Err(e) => {
                panic!("Error getting daemonset {}: {:?}", ds_name, e);
            }
        }
        if !is_ready {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
    }
    is_ready
}

async fn delete_pod(client: Client, pod_name: &str, namespace: &str) {
    let pods: Api<k8s_openapi::api::core::v1::Pod> = Api::namespaced(client, namespace);
    let dp = kube::api::DeleteParams::default();
    match pods.delete(pod_name, &dp).await {
        Ok(_) => {}
        Err(kube::Error::Api(ae)) if ae.code == 404 => {
            info!("Pod {} does not exist in {}.", pod_name, namespace);
            return;
        }
        Err(e) => {
            info!(
                "Failed to delete pod {} in {}: {:?}",
                pod_name, namespace, e
            );
        }
    }
    // wait for the pod to be completely deleted
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        match pods.get(pod_name).await {
            Ok(_) => {
                info!(
                    "Waiting for pod {} in {} to be deleted...",
                    pod_name, namespace
                );
            }
            Err(kube::Error::Api(ae)) if ae.code == 404 => {
                info!("Pod {} has been deleted in {}.", pod_name, namespace);
                break;
            }
            Err(e) => {
                // Some other error
                panic!(
                    "Failed to check status of pod {} in {}: {:?}",
                    pod_name, namespace, e
                );
            }
        }
    }
}

async fn delete_service(client: Client, service_name: &str, namespace: &str) {
    let services: Api<k8s_openapi::api::core::v1::Service> = Api::namespaced(client, namespace);
    let dp = kube::api::DeleteParams::default();
    match services.delete(service_name, &dp).await {
        Ok(_) => {}
        Err(kube::Error::Api(ae)) if ae.code == 404 => {
            info!("Service {} does not exist in {}.", service_name, namespace);
            return;
        }
        Err(e) => {
            info!(
                "Failed to delete service {} in {}: {:?}",
                service_name, namespace, e
            );
        }
    }
    // wait for the service to be completely deleted
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        match services.get(service_name).await {
            Ok(_) => {
                info!(
                    "Waiting for service {} in {} to be deleted...",
                    service_name, namespace
                );
            }
            Err(kube::Error::Api(ae)) if ae.code == 404 => {
                info!(
                    "Service {} has been deleted in {}.",
                    service_name, namespace
                );
                break;
            }
            Err(e) => {
                // Some other error
                panic!(
                    "Failed to check status of service {} in {}: {:?}",
                    service_name, namespace, e
                );
            }
        }
    }
}

async fn delete_daemonset(client: Client, daemonset_name: &str, namespace: &str) {
    let daemonsets: Api<k8s_openapi::api::apps::v1::DaemonSet> = Api::namespaced(client, namespace);
    let dp = kube::api::DeleteParams::default();

    match daemonsets.delete(daemonset_name, &dp).await {
        Ok(_) => {}
        Err(kube::Error::Api(ae)) if ae.code == 404 => {
            info!(
                "DaemonSet {} does not exist in {}.",
                daemonset_name, namespace
            );
            return;
        }
        Err(e) => {
            info!(
                "Failed to delete DaemonSet {} in {}: {:?}",
                daemonset_name, namespace, e
            );
        }
    }

    // wait for the DaemonSet to be completely deleted
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        match daemonsets.get(daemonset_name).await {
            Ok(_) => {
                info!(
                    "Waiting for DaemonSet {} in {} to be deleted...",
                    daemonset_name, namespace
                );
            }
            Err(kube::Error::Api(ae)) if ae.code == 404 => {
                info!(
                    "DaemonSet {} has been deleted in {}.",
                    daemonset_name, namespace
                );
                break;
            }
            Err(e) => {
                // Some other error
                panic!(
                    "Failed to check status of DaemonSet {} in {}: {:?}",
                    daemonset_name, namespace, e
                );
            }
        }
    }
}

async fn delete_namespace(client: Client, namespace_name: &str) {
    let namespaces: Api<k8s_openapi::api::core::v1::Namespace> = Api::all(client);
    let dp = kube::api::DeleteParams::default();

    match namespaces.delete(namespace_name, &dp).await {
        Ok(_) => {}
        Err(kube::Error::Api(ae)) if ae.code == 404 => {
            info!("Namespace {} does not exist.", namespace_name);
            return;
        }
        Err(e) => {
            info!("Failed to delete Namespace {}: {:?}", namespace_name, e);
        }
    }

    // wait for the namespace to be completely deleted
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        match namespaces.get(namespace_name).await {
            Ok(_) => {
                info!("Waiting for Namespace {} to be deleted...", namespace_name);
            }
            Err(kube::Error::Api(ae)) if ae.code == 404 => {
                info!("Namespace {} has been deleted.", namespace_name);
                break;
            }
            Err(e) => {
                // Some other error
                panic!(
                    "Failed to check status of Namespace {}: {:?}",
                    namespace_name, e
                );
            }
        }
    }
}

/// tail_lines = None - means no lookback
async fn assert_log_lines(
    client: Client,
    namespace: &str,
    label: &str,
    substrings: Vec<&str>,
    tail_lines: Option<i64>,
    print_log_lines: bool,
) {
    let pods: Api<Pod> = Api::namespaced(client, namespace);
    let lp = ListParams::default().labels(label);
    let mut handles = Vec::new();
    pods.list(&lp).await.unwrap().into_iter().for_each(|p| {
        let pods = pods.clone();
        let mut substrings_not_found: Vec<String> =
            substrings.iter().map(|s| s.to_string()).collect();
        substrings_not_found.reverse();
        let handle = tokio::spawn({
            async move {
                let mut logs = pods
                    .log_stream(
                        &p.metadata.name.clone().unwrap(),
                        &LogParams {
                            follow: false,
                            tail_lines,
                            ..LogParams::default()
                        },
                    )
                    .await
                    .unwrap()
                    .lines();

                while let Some(line) = logs.try_next().await.unwrap() {
                    if print_log_lines {
                        info!("LOG: {:?}", line);
                    }
                    if let Some(pat) = substrings_not_found.last() {
                        if line.contains(pat) {
                            substrings_not_found.pop();
                        }
                    } else if !print_log_lines {
                        break;
                    }
                }
                assert!(
                    substrings_not_found.is_empty(),
                    "not found substrings: {:?}",
                    substrings_not_found
                );
            }
        });
        handles.push(handle);
    });
    assert!(
        !handles.is_empty(),
        "not found substrings: {:?}",
        substrings
    );
    for handle in handles {
        handle.await.unwrap();
    }
}

async fn start_line_proxy_pod(
    client: Client,
    pod_name: &str,
    namespace: &str,
    node_port: u16,
) -> SocketAddr {
    //// Create socat pod
    let pods: Api<Pod> = Api::namespaced(client.clone(), namespace);
    let services: Api<Service> = Api::namespaced(client.clone(), namespace);

    let pod = serde_json::from_value(serde_json::json!({
        "apiVersion": "v1",
        "kind": "Pod",
        "metadata": {
            "name": pod_name,
            "labels": {
                "app": pod_name,
                "app.kubernetes.io/name": pod_name,
                "app.kubernetes.io/instance": format!("{}-instance", pod_name),
            },
        },
        "spec": {
            "containers": [
                {
                    "name": pod_name,
                    "image": "socat:local",
                    "ports": [
                        {
                            "name": "tcp-socat",
                            "containerPort": 80,
                            "protocol": "TCP"
                        },
                    ],
                    "readinessProbe": {
                        "tcpSocket": {
                            "port": 80
                        },
                        "periodSeconds": 1,
                    },
                    "livenessProbe": {
                        "tcpSocket" : {
                            "port": 80,
                        },
                        "periodSeconds": 1,
                    },
                    "command": [
                        "sh",
                        "-c",
                        "socat TCP-LISTEN:80,fork,reuseaddr stdio"
                    ],
                },
            ]
        }
    }))
    .unwrap();

    let service = serde_json::from_value(serde_json::json!({
        "apiVersion": "v1",
        "kind": "Service",
        "metadata": {
            "name": pod_name,
            "labels": {
                "app": pod_name,
            },
        },
        "spec": {
            "type": "NodePort",
            "selector": {
                "app": pod_name,
            },
            "ports": [
                {
                    "protocol": "TCP",
                    "port": 80,
                    "nodePort": node_port,
                }
            ],
        }
    }))
    .unwrap();

    // Create the pod
    pods.create(&PostParams::default(), &pod).await.unwrap();
    services
        .create(&PostParams::default(), &service)
        .await
        .unwrap();

    //// Wait for pod

    // Start a watch call for pods matching our name
    let wp = WatchParams::default()
        .fields(&format!("metadata.name={}", pod_name))
        .timeout(60);
    let mut stream = pods.watch(&wp, "0").await.unwrap().boxed();

    let lp = ListParams::default()
        .fields(&format!("metadata.name={}", pod_name))
        .timeout(60);

    // Observe the pods phase for up to 60 seconds
    'outer: while let Some(status) = stream.try_next().await.unwrap() {
        match status {
            WatchEvent::Added(o) => {
                let s = o.status.as_ref().expect("status exists on pod");

                if let Some(container_statuses) = s.container_statuses.as_ref() {
                    for status in container_statuses {
                        if status.ready {
                            break 'outer;
                        }
                    }
                }
            }
            WatchEvent::Modified(o) => {
                let s = o.status.as_ref().expect("status exists on pod");
                if let Some(container_statuses) = s.container_statuses.as_ref() {
                    for status in container_statuses {
                        if status.ready {
                            break 'outer;
                        }
                    }
                }
            }
            WatchEvent::Deleted(_o) => {}
            WatchEvent::Error(_e) => {}
            _ => {}
        }
    }

    let pods = pods.list(&lp).await.unwrap();
    let pod_addr = pods
        .iter()
        .next()
        .as_ref()
        .expect("pod exists")
        .status
        .as_ref()
        .expect("status exists for pod")
        .host_ip
        .as_ref()
        .expect("pod has a host ip")
        .parse::<std::net::IpAddr>()
        .expect("host IP should be an IP");
    // Get the IP for the node port
    SocketAddr::new(pod_addr, node_port)
}

fn ingester_public_addr(ingester_addr: impl ToSocketAddrs) -> SocketAddr {
    let default_ip = pnet_datalink::interfaces()
        .iter()
        .find(|e| e.is_up() && !e.is_loopback() && !e.ips.is_empty())
        .expect("container should have an interface")
        .ips
        .first()
        .expect("container should have an IP")
        .ip();
    let ingester_addr = ingester_addr
        .to_socket_addrs()
        .expect("Addr should be valid")
        .next()
        .unwrap();
    SocketAddr::new(default_ip, ingester_addr.port())
}

async fn create_mock_ingester_service(
    client: Client,
    ingester_addr: SocketAddr,
    service_name: &str,
    service_namespace: &str,
    service_port: u16,
) -> String {
    //// Set up mock ingester service
    let ingest_service = serde_json::from_value(serde_json::json!({
        "apiVersion": "v1",
        "kind": "Service",
        "metadata": {
            "name": service_name,
        },
        "spec": {
            "ports": [
                {
                    "name": service_name,
                    "protocol": "TCP",
                    "port": service_port,
                    "targetPort": ingester_addr.port(),
                    "nodePort": 0,
                }
            ],
        }
    }))
    .unwrap();

    let agent_services: Api<Service> = Api::namespaced(client.clone(), service_namespace);
    let ingest_service = agent_services
        .create(&PostParams::default(), &ingest_service)
        .await
        .unwrap();

    let ingest_cluster_ip = ingest_service
        .spec
        .as_ref()
        .unwrap()
        .cluster_ip
        .as_ref()
        .unwrap();

    let ingest_endpoint = serde_json::from_value(serde_json::json!({
        "apiVersion": "v1",
        "kind": "Endpoints",
        "metadata": {
            "name": service_name,
        },
        "subsets": [
            {
                "addresses": [
                    { "ip": ingester_addr.ip() }
                ],
                "ports": [
                    {
                        "name": service_name,
                        "port": ingester_addr.port(),
                    }
                ],
            },
        ],
    }))
    .unwrap();

    let agent_endpoints: Api<Endpoints> = Api::namespaced(client.clone(), service_namespace);
    agent_endpoints
        .create(&PostParams::default(), &ingest_endpoint)
        .await
        .unwrap();

    // TODO work out why this isn't working...
    // format!("{}.{}.svc.cluster.local:{}", service_name, service_namespace, service_port)
    format!("{}:{}", ingest_cluster_ip, service_port)
}

#[allow(clippy::too_many_arguments)]
async fn create_agent_ds(
    client: Client,
    agent_name: &str,
    agent_namespace: &str,
    ingester_addr: &str,
    log_k8s_events: &str,
    enrich_logs_with_k8s: &str,
    agent_log_level: &str,
    agent_startup_lease: &str,
    log_reporter_metrics: &str,
    env_vars: Option<Vec<(&str, &str)>>,
) {
    let sa = serde_json::from_value(serde_json::json!({
        "apiVersion": "v1",
        "kind": "ServiceAccount",
        "metadata": {
            "name": agent_name
        }
    }))
    .unwrap();
    let sas: Api<ServiceAccount> = Api::namespaced(client.clone(), agent_namespace);
    sas.create(&PostParams::default(), &sa).await.unwrap();
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    let r = serde_json::from_value(serde_json::json!({
        "apiVersion": "rbac.authorization.k8s.io/v1",
        "kind": "Role",
        "metadata": {
            "name": &format!("{}-logging", agent_name)
        },
        "rules": [
            {
                "apiGroups": [
                    ""
                ],
                "resources": [
                    "configmaps"
                ],
                "verbs": [
                    "get",
                    "list",
                    "create",
                    "watch"
                ]
            }
        ]
    }))
    .unwrap();
    let rs: Api<Role> = Api::namespaced(client.clone(), agent_namespace);
    rs.create(&PostParams::default(), &r).await.unwrap();

    let rb = serde_json::from_value(serde_json::json!({
        "apiVersion": "rbac.authorization.k8s.io/v1",
        "kind": "RoleBinding",
        "metadata": {
            "name": agent_name
        },
        "roleRef": {
            "apiGroup": "rbac.authorization.k8s.io",
            "kind": "Role",
            "name": &format!("{}-logging", agent_name)
        },
        "subjects": [
            {
                "kind": "ServiceAccount",
                "name": agent_name,
                "namespace": agent_namespace
            }
        ]
    }))
    .unwrap();
    let rbs: Api<RoleBinding> = Api::namespaced(client.clone(), agent_namespace);
    rbs.create(&PostParams::default(), &rb).await.unwrap();
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    let cr = serde_json::from_value(serde_json::json!({
        "apiVersion": "rbac.authorization.k8s.io/v1",
        "kind": "ClusterRole",
        "metadata": {
            "name": &format!("{}-logging", agent_name)
        },
        "rules": [
            {
                "apiGroups": [
                    ""
                ],
                "resources": [
                    "events"
                ],
                "verbs": [
                    "get",
                    "list",
                    "create",
                    "watch"
                ]
            },
            {
                "apiGroups": [
                    ""
                ],
                "resources": [
                    "pods"
                ],
                "verbs": [
                    "get",
                    "list",
                    "watch"
                ]
            },
            {
                "apiGroups": [
                    ""
                ],
                "resources": [
                    "nodes"
                ],
                "verbs": [
                    "get",
                    "list",
                    "watch"
                ]
            },
            {
                "apiGroups": [
                    "metrics.k8s.io"
                ],
                "resources": [
                    "pods"
                ],
                "verbs": [
                    "get",
                    "list",
                    "watch"
                ]
            },
            {
                "apiGroups": [
                    "metrics.k8s.io"
                ],
                "resources": [
                    "nodes"
                ],
                "verbs": [
                    "get",
                    "list",
                    "watch"
                ]
            }
        ]
    }))
    .unwrap();
    let crs: Api<ClusterRole> = Api::all(client.clone());
    crs.create(&PostParams::default(), &cr).await.unwrap();

    let crb = serde_json::from_value(serde_json::json!({
        "apiVersion": "rbac.authorization.k8s.io/v1",
        "kind": "ClusterRoleBinding",
        "metadata": {
            "name": agent_name
        },
        "roleRef": {
            "apiGroup": "rbac.authorization.k8s.io",
            "kind": "ClusterRole",
            "name": &format!("{}-logging", agent_name)
        },
        "subjects": [
            {
                "kind": "ServiceAccount",
                "name": agent_name,
                "namespace": agent_namespace
            }
        ]
    }))
    .unwrap();
    let crbs: Api<ClusterRoleBinding> = Api::all(client.clone());
    crbs.create(&PostParams::default(), &crb).await.unwrap();

    //// Set up agent
    let ds = get_agent_ds_yaml(
        "logdna-agent-v2:local",
        ingester_addr,
        "false",
        agent_name,
        agent_namespace,
        log_k8s_events,
        enrich_logs_with_k8s,
        agent_log_level,
        agent_startup_lease,
        log_reporter_metrics,
        env_vars,
    );
    //
    let dss: Api<DaemonSet> = Api::namespaced(client.clone(), agent_namespace);

    dss.create(&PostParams::default(), &ds).await.unwrap();
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    let agent_pods: Api<Pod> = Api::namespaced(client.clone(), agent_namespace);

    let wp = WatchParams::default()
        .labels(&format!("app={}", agent_name))
        .timeout(60);

    let mut stream = agent_pods.watch(&wp, "0").await.unwrap().boxed();

    // Observe the pods phase for up to 60 seconds
    'ds_outer: while let Some(status) = stream.try_next().await.unwrap() {
        match status {
            WatchEvent::Added(o) => {
                let s = o.status.as_ref().expect("status exists on pod");

                if let Some(container_statuses) = s.container_statuses.as_ref() {
                    for status in container_statuses {
                        if status.ready {
                            break 'ds_outer;
                        }
                    }
                }
            }
            WatchEvent::Modified(o) => {
                let s = o.status.as_ref().expect("status exists on pod");
                if let Some(container_statuses) = s.container_statuses.as_ref() {
                    for status in container_statuses {
                        if status.ready {
                            break 'ds_outer;
                        }
                    }
                }
            }
            WatchEvent::Deleted(o) => println!("Deleted {}", &o.metadata.name.unwrap()),
            WatchEvent::Error(e) => println!("Error {}", e),
            _ => {}
        }
    }
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
}

#[allow(clippy::too_many_arguments)]
fn get_agent_ds_yaml(
    image_name: &str,
    ingester_addr: &str,
    use_ssl: &str,
    agent_name: &str,
    agent_namespace: &str,
    log_k8s_events: &str,
    enrich_logs_with_k8s: &str,
    log_level: &str,
    startup_lease: &str,
    log_reporter_metrics: &str,
    env_vars: Option<Vec<(&str, &str)>>,
) -> DaemonSet {
    let mut cfgmap = serde_json::json!({
        "apiVersion": "apps/v1",
        "kind": "DaemonSet",
        "metadata": {
            "labels": {
                "app.kubernetes.io/instance": format!("{}-instance", agent_name),
                "app.kubernetes.io/name": agent_name,
            },
            "name": agent_name,
        },
        "spec": {
            "selector": {
                "matchLabels": {
                    "app": agent_name
                }
            },
            "template": {
                "metadata": {
                    "labels": {
                        "app.kubernetes.io/instance": format!("{}-instance", agent_name),
                        "app.kubernetes.io/name": agent_name,
                        "app": agent_name,
                    }
                },
                "spec": {
                    "containers": [
                        {
                            "env": [
                                {
                                    "name": "RUST_LOG",
                                    "value": log_level
                                },
                                {
                                    "name": "LOGDNA_HOST",
                                    "value": ingester_addr
                                },
                                {
                                    "name": "LOGDNA_USE_SSL",
                                    "value": use_ssl
                                },
                                {
                                    "name": "LOGDNA_AGENT_KEY",
                                    "value": "123456"
                                    /*"valueFrom": {
                                        "secretKeyRef": {
                                        "key": "logdna-agent-key",
                                        "name": "logdna-agent-key"
                                }
                                }*/
                                },
                                {
                                    "name": "LOGDNA_DB_PATH",
                                    "value": "/var/lib/logdna"
                                },
                                {
                                    "name": "LOGDNA_K8S_STARTUP_LEASE",
                                    "value": startup_lease
                                },
                                {
                                    "name": "LOGDNA_LOG_K8S_EVENTS",
                                    "value": log_k8s_events
                                },
                                {
                                    "name": "LOGDNA_USE_K8S_LOG_ENRICHMENT",
                                    "value": enrich_logs_with_k8s,
                                },
                                {
                                    "name": "LOGDNA_K8S_METADATA_LINE_INCLUSION",
                                    "value": format!("namespace:default, namespace:{}", agent_namespace)
                                },
                                {
                                    "name": "LOGDNA_K8S_METADATA_LINE_EXCLUSION",
                                    "value": "label.app.kubernetes.io/name:filter-pod"
                                },
                                {
                                    "name": "LOGDNA_LOG_METRIC_SERVER_STATS",
                                    "value": log_reporter_metrics,
                                },
                                {
                                    "name": "POD_APP_LABEL",
                                    "valueFrom": {
                                        "fieldRef": {
                                            "fieldPath": "metadata.labels['app.kubernetes.io/name']"
                                        }
                                    }
                                },
                                {
                                    "name": "POD_NAME",
                                    "valueFrom": {
                                        "fieldRef": {
                                            "fieldPath": "metadata.name"
                                        }
                                    }
                                },
                                {
                                    "name": "NODE_NAME",
                                    "valueFrom": {
                                        "fieldRef": {
                                            "fieldPath": "spec.nodeName"
                                        }
                                    }
                                },
                                {
                                    "name": "NAMESPACE",
                                    "valueFrom": {
                                        "fieldRef": {
                                            "fieldPath": "metadata.namespace"
                                        }
                                    }
                                }
                            ],
                            "image": image_name,
                            "imagePullPolicy": "IfNotPresent",
                            "name": "logdna-agent",
                            "resources": {
                                "limits": {
                                    "memory": "5000Mi"
                                },
                                "requests": {
                                    "cpu": "20m"
                                }
                            },
                            "securityContext": {
                                "runAsUser": 5000,
                                "runAsGroup": 5000,
                                "fsGroup": 5000,
                                "capabilities": {
                                    "add": [
                                        "DAC_READ_SEARCH"
                                    ],
                                    "drop": [
                                        "all"
                                    ]
                                }
                            },
                            "volumeMounts": [
                                {
                                    "mountPath": "/var/log",
                                    "name": "varlog"
                                },
                                {
                                    "mountPath": "/var/data",
                                    "name": "vardata"
                                },
                                {
                                    "mountPath": "/var/lib/logdna",
                                    "name": "varliblogdna"
                                },
                                {
                                    "mountPath": "/var/lib/docker/containers",
                                    "name": "varlibdockercontainers",
                                    "readOnly": true
                                },
                                {
                                    "mountPath": "/mnt",
                                    "name": "mnt",
                                    "readOnly": true
                                },
                                {
                                    "mountPath": "/etc/os-release",
                                    "name": "osrelease"
                                },
                                {
                                    "mountPath": "/etc/logdna-hostname",
                                    "name": "logdnahostname"
                                }
                            ]
                        }
                    ],
                    "serviceAccountName": agent_name,
                    "terminationGracePeriodSeconds": 2,
                    "volumes": [
                        {
                            "hostPath": {
                                "path": "/var/log"
                            },
                            "name": "varlog"
                        },
                        {
                            "hostPath": {
                                "path": "/var/data"
                            },
                            "name": "vardata"
                        },
                        {
                            "hostPath": {
                                "path": "/var/lib/logdna"
                            },
                    "name": "varliblogdna"
                },
                {
                    "hostPath": {
                    "path": "/var/lib/docker/containers"
                    },
                    "name": "varlibdockercontainers"
                },
                {
                    "hostPath": {
                    "path": "/mnt"
                    },
                    "name": "mnt"
                },
                {
                    "hostPath": {
                    "path": "/etc/os-release"
                    },
                    "name": "osrelease"
                },
                {
                    "hostPath": {
                    "path": "/etc/hostname"
                    },
                    "name": "logdnahostname"
                }
                ]
            }
            },
            "updateStrategy": {
            "rollingUpdate": {
                "maxUnavailable": "100%"
            },
            "type": "RollingUpdate"
            }
        }
        }
    );
    if let Some(env_vars) = env_vars {
        let env_vars: Vec<HashMap<&str, serde_json::Value>> = env_vars
            .into_iter()
            .map(|(k, v)| {
                let mut map = HashMap::new();
                map.insert(k, serde_json::json!(v));
                map
            })
            .collect();
        let existing_env_vars = &mut cfgmap["spec"]["template"]["spec"]["containers"][0]["env"]
            .as_array_mut()
            .unwrap();
        for env_var in env_vars {
            if let Some((k, v)) = env_var.iter().next() {
                // Create a new serde_json::Map to hold the restructured environment variable
                let mut new_map = serde_json::Map::new();
                new_map.insert("name".to_string(), serde_json::json!(k));
                new_map.insert("value".to_string(), v.clone());

                // Push the newly formatted environment variable into existing_env_vars
                existing_env_vars.push(serde_json::Value::Object(new_map));
            }
        }
    }
    serde_json::from_value(cfgmap).expect("failed to serialize DS manifest")
}

async fn create_agent_startup_lease_list(client: Client, name: &str, namespace: &str) {
    let r = serde_json::from_value(serde_json::json!({
        "apiVersion": "rbac.authorization.k8s.io/v1",
        "kind": "Role",
        "metadata": {
            "namespace": namespace,
            "name": format!("{}-role", name)
        },
        "rules": [
            {
                "apiGroups": [
                    "coordination.k8s.io"
                ],
                "resources": [
                    "leases"
                ],
                "verbs": [
                    "get",
                    "list",
                    "create",
                    "update",
                    "patch"
                ]
            }
        ]
    }))
    .unwrap();
    let role_client: Api<Role> = Api::namespaced(client.clone(), namespace);
    role_client
        .create(&PostParams::default(), &r)
        .await
        .unwrap();

    let rb = serde_json::from_value(serde_json::json!({
        "apiVersion": "rbac.authorization.k8s.io/v1",
        "kind": "RoleBinding",
        "metadata": {
            "name": name
        },
        "roleRef": {
            "apiGroup": "rbac.authorization.k8s.io",
            "kind": "Role",
            "name": format!("{}-role", name)
        },
        "subjects": [
            {
                "kind": "ServiceAccount",
                "name": namespace,
                "namespace": namespace
            }
        ]
    }))
    .unwrap();
    let rolebinding_client: Api<RoleBinding> = Api::namespaced(client.clone(), namespace);
    rolebinding_client
        .create(&PostParams::default(), &rb)
        .await
        .unwrap();

    let l_zero = serde_json::from_value(serde_json::json!({
        "apiVersion": "coordination.k8s.io/v1",
        "kind": "Lease",
        "metadata": {
            "name": format!("{}-0",name),
            "labels": {
                "process": "logdna-agent-startup"
            },
        },
        "spec": {
            "holderIdentity": "agent-default"
        }
    }))
    .unwrap();
    let lease_client: Api<Lease> = Api::namespaced(client.clone(), namespace);
    lease_client
        .create(&PostParams::default(), &l_zero)
        .await
        .unwrap();
    let l_one = serde_json::from_value(serde_json::json!({
        "apiVersion": "coordination.k8s.io/v1",
        "kind": "Lease",
        "metadata": {
            "name": format!("{}-1",name),
            "labels": {
                "process": "logdna-agent-startup"
            },
        },
        "spec": {
            "holderIdentity": "agent-default"
        }
    }))
    .unwrap();
    let lease_client: Api<Lease> = Api::namespaced(client.clone(), namespace);
    lease_client
        .create(&PostParams::default(), &l_one)
        .await
        .unwrap();
    let l_two = serde_json::from_value(serde_json::json!({
        "apiVersion": "coordination.k8s.io/v1",
        "kind": "Lease",
        "metadata": {
            "name": format!("{}-2",name),
            "labels": {
                "process": "logdna-agent-startup"
            },
        },
        "spec": {
            "holderIdentity": "agent-default"
        }
    }))
    .unwrap();
    let lease_client: Api<Lease> = Api::namespaced(client.clone(), namespace);
    lease_client
        .create(&PostParams::default(), &l_two)
        .await
        .unwrap();
}

async fn create_agent_feature_lease(
    client: Client,
    namespace: &str,
    lease_name: &str,
    process: &str,
) {
    let r = serde_json::from_value(serde_json::json!({
        "apiVersion": "rbac.authorization.k8s.io/v1",
        "kind": "Role",
        "metadata": {
            "namespace": namespace,
            "name": lease_name
        },
        "rules": [
            {
                "apiGroups": [
                    "coordination.k8s.io"
                ],
                "resources": [
                    "leases"
                ],
                "verbs": [
                    "get",
                    "list",
                    "create",
                    "update",
                    "patch"
                ]
            }
        ]
    }))
    .unwrap();
    let role_client: Api<Role> = Api::namespaced(client.clone(), namespace);
    role_client
        .create(&PostParams::default(), &r)
        .await
        .unwrap();

    let rb = serde_json::from_value(serde_json::json!({
        "apiVersion": "rbac.authorization.k8s.io/v1",
        "kind": "RoleBinding",
        "metadata": {
            "name": lease_name,
            "namespace": namespace
        },
        "roleRef": {
            "apiGroup": "rbac.authorization.k8s.io",
            "kind": "Role",
            "name": lease_name
        },
        "subjects": [
            {
                "kind": "ServiceAccount",
                "name": namespace,
                "namespace": namespace
            }
        ]
    }))
    .unwrap();
    let rolebinding_client: Api<RoleBinding> = Api::namespaced(client.clone(), namespace);
    rolebinding_client
        .create(&PostParams::default(), &rb)
        .await
        .unwrap();

    let l_zero = serde_json::from_value(serde_json::json!({
        "apiVersion": "coordination.k8s.io/v1",
        "kind": "Lease",
        "metadata": {
            "name": lease_name,
            "labels": {
                "process": process
            },
        },
        "spec": {
            "leaseDurationSeconds": 5i32
        }
    }))
    .unwrap();
    let lease_client: Api<Lease> = Api::namespaced(client.clone(), namespace);
    lease_client
        .create(&PostParams::default(), &l_zero)
        .await
        .unwrap();
}

#[test(tokio::test)]
#[cfg_attr(not(feature = "k8s_tests"), ignore)]
async fn test_k8s_connection() {
    let client = Client::try_default().await.unwrap();
    let pods: Api<Pod> = Api::all(client);
    let lp = ListParams::default(); // for this app only
    let pod_list = pods.list(&lp).await.unwrap();
    assert!(pod_list.iter().count() > 0);
}

#[test(tokio::test)]
#[cfg_attr(not(feature = "k8s_tests"), ignore)]
async fn test_k8s_enrichment() {
    let (server, received, shutdown_handle, ingester_addr) = common::start_http_ingester();

    tokio::time::sleep(tokio::time::Duration::from_millis(3000)).await;

    let client = Client::try_default().await.unwrap();

    let socat_pod_name = "socat-listener";
    let default_namespace = "default";
    let socat_pod_node_addr =
        start_line_proxy_pod(client.clone(), socat_pod_name, default_namespace, 30001).await;

    assert!(
        wait_for_pod_ready(
            client.clone(),
            socat_pod_name,
            default_namespace,
            tokio::time::Duration::from_millis(10_000),
        )
        .await
    );
    info!("pod {} in {} is ready", socat_pod_name, default_namespace);

    let agent_name = "k8s-enrichment";
    let agent_namespace = "k8s-enrichment";

    let (server_result, _) = tokio::join!(server, async {
        let ns = serde_json::from_value(serde_json::json!({
            "apiVersion": "v1",
            "kind": "Namespace",
            "metadata": {
                "name": agent_namespace
            }
        }))
        .unwrap();
        let nss: Api<Namespace> = Api::all(client.clone());
        nss.create(&PostParams::default(), &ns).await.unwrap();

        let mock_ingester_socket_addr_str = create_mock_ingester_service(
            client.clone(),
            ingester_public_addr(ingester_addr),
            "ingest-service",
            agent_namespace,
            80,
        )
        .await;

        create_agent_ds(
            client.clone(),
            agent_name,
            agent_namespace,
            &mock_ingester_socket_addr_str,
            "never",
            "always",
            "logdna_agent::_main=debug,info",
            "never",
            "never",
            Some(vec![
                // to make sure all is still functional with delay enabled,
                // in normal case this delay is not going be triggered as
                // all pod metadata shall be always available in this setup (good case)
                (config::env_vars::METADATA_RETRY_DELAY, "5"),
                (config::env_vars::FLUSH_DURATION, "250"),
            ]),
        )
        .await;

        assert!(
            wait_for_ds_ready(
                client.clone(),
                agent_name,
                agent_namespace,
                tokio::time::Duration::from_millis(10_000),
            )
            .await
        );
        info!("daemonset {} in {} is ready", agent_name, agent_namespace);

        print_pod_logs(
            client.clone(),
            agent_namespace,
            &format!("app={}", &agent_name),
        )
        .await;

        let messages = [
            "Hello, World! 0\n",
            "Hello, World! 2\n",
            "Hello, World! 3\n",
            "Hello, World! 1\n",
            "Hello, World! 4\n",
        ];

        let mut logger_stream = TcpStream::connect(socat_pod_node_addr).await.unwrap();

        for msg in messages.iter() {
            // Write some data.
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
            logger_stream.write_all(msg.as_bytes()).await.unwrap();
        }

        // Wait for the data to be received by the mock ingester
        tokio::time::sleep(tokio::time::Duration::from_millis(3_000)).await;

        let map = received.lock().await;

        let result = map.iter().find(|(k, _)| k.contains(socat_pod_name));
        assert!(result.is_some());

        let (_, pod_file_info) = result.unwrap();
        let label = pod_file_info.label.as_ref();
        assert!(label.is_some());
        assert_eq!(label.unwrap()["app.kubernetes.io/name"], socat_pod_name);
        assert_eq!(
            label.unwrap()["app.kubernetes.io/instance"],
            format!("{}-instance", socat_pod_name)
        );
        let values = &pod_file_info.values;
        assert!(values.len() == 5, "{:#?}", values);
        for (left, right) in values.iter().zip(messages.iter()) {
            assert!(left.ends_with(right));
        }

        let result = map.iter().find(|(k, _)| k.contains("sample-pod"));
        assert!(result.is_some());

        let (_, pod_file_info) = result.unwrap();
        let label = pod_file_info.label.as_ref();
        let meta = pod_file_info.meta.as_ref();

        assert_eq!(meta.unwrap()["Image Name"].as_str().unwrap(), "busybox");
        assert!(label.is_some());
        assert_eq!(label.unwrap()["app.kubernetes.io/name"], "sample-pod");
        assert_eq!(
            label.unwrap()["app.kubernetes.io/instance"],
            "sample-pod-instance"
        );

        let result = map.iter().find(|(k, _)| k.contains("sample-job"));
        assert!(result.is_some());
        let (_, job_file_info) = result.unwrap();
        let label = job_file_info.label.as_ref();
        assert!(label.is_some());
        assert_eq!(label.unwrap()["job-name"], "sample-job");

        // Ensure k8s exclusion filter working
        let result = map.iter().find(|(k, _)| k.contains("filter-pod"));
        assert!(result.is_none());

        // Ensure k8s inclusion is filter out non default namespaces
        let result = map.iter().find(|(k, _)| k.contains("kube-system"));
        assert!(result.is_none());

        shutdown_handle();
    });

    // cleanup
    delete_daemonset(client.clone(), agent_name, agent_namespace).await;
    delete_service(client.clone(), "ingest-service", agent_namespace).await;
    delete_namespace(client.clone(), agent_namespace).await;
    delete_pod(client.clone(), socat_pod_name, default_namespace).await;
    delete_service(client.clone(), socat_pod_name, default_namespace).await;

    server_result.unwrap();
}

#[derive(serde::Deserialize)]
struct EventLineMeta {
    #[serde(rename = "type")]
    tag: String,
}
#[derive(serde::Deserialize)]
struct EventLine {
    kube: EventLineMeta,
    // TODO: force a specific event and test for it's contents
    // message: String
}

#[test(tokio::test)]
#[cfg_attr(not(feature = "k8s_tests"), ignore)]
async fn test_k8s_events_logged() {
    let (server, received, shutdown_handle, ingester_addr) = common::start_http_ingester();

    let client = Client::try_default().await.unwrap();

    let agent_name = "k8s-events-logged";
    let agent_namespace = "k8s-events-logged";

    let (server_result, _) = tokio::join!(server, async {
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        let nss: Api<Namespace> = Api::all(client.clone());
        let ns = serde_json::from_value(serde_json::json!({
            "apiVersion": "v1",
            "kind": "Namespace",
            "metadata": {
                "name": agent_namespace
            }
        }))
        .unwrap();
        nss.create(&PostParams::default(), &ns).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        create_agent_feature_lease(
            client.clone(),
            agent_namespace,
            "logdna-agent-k8-events-lease",
            "logdna-agent-k8-events",
        )
        .await;

        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        let mock_ingester_socket_addr_str = create_mock_ingester_service(
            client.clone(),
            ingester_public_addr(ingester_addr),
            "ingest-service",
            agent_namespace,
            80,
        )
        .await;
        tokio::time::sleep(tokio::time::Duration::from_millis(10_000)).await;

        create_agent_ds(
            client.clone(),
            agent_name,
            agent_namespace,
            &mock_ingester_socket_addr_str,
            "always",
            "always",
            "warn",
            "never",
            "never",
            None,
        )
        .await;

        // Wait for the data to be received by the mock ingester
        tokio::time::sleep(tokio::time::Duration::from_millis(10_000)).await;
        let map = received.lock().await;

        let unknown_log_lines = map.get(" unknown").unwrap();

        let mut found_event = false;
        for event in &unknown_log_lines.values {
            if let Ok(event) = serde_json::from_str::<EventLine>(event) {
                if event.kube.tag == "event" {
                    found_event = true;
                    break;
                }
            }
        }
        assert!(found_event);

        let has_dups = (1..unknown_log_lines.values.len())
            .any(|i| unknown_log_lines.values[i..].contains(&unknown_log_lines.values[i - 1]));

        assert!(!has_dups);
        shutdown_handle();
    });

    // cleanup
    delete_daemonset(client.clone(), agent_name, agent_namespace).await;
    delete_service(client.clone(), "ingest-service", agent_namespace).await;
    delete_namespace(client.clone(), agent_namespace).await;

    server_result.unwrap();
}

#[test(tokio::test)]
#[cfg_attr(not(feature = "k8s_tests"), ignore)]
async fn test_k8s_startup_lease_functions() {
    let lease_name = "agent-startup-lease";
    let namespace = "default";
    let pod_name = "agent-pod-name".to_string();
    let lease_label = "process=logdna-agent-startup";
    let mut claimed_lease_name: Option<String> = None;
    let client = Client::try_default().await.unwrap();
    let lease_client: Api<Lease> = Api::namespaced(client.clone(), namespace);
    let lp = ListParams::default().labels(lease_label);

    create_agent_startup_lease_list(client, lease_name, namespace).await;
    let lease_list = lease_client.list(&lp).await;
    assert!(lease_list.as_ref().unwrap().iter().count() > 0);

    k8s::lease::release_lease("agent-startup-lease-1", &lease_client).await;
    let available_lease = k8s::lease::get_available_lease(lease_label, &lease_client).await;
    assert_eq!(available_lease.as_ref().unwrap(), "agent-startup-lease-1");
    k8s::lease::claim_lease(
        available_lease.unwrap(),
        pod_name,
        &lease_client,
        &mut claimed_lease_name,
    )
    .await;
    let available_lease = k8s::lease::get_available_lease(lease_label, &lease_client).await;
    assert_eq!(available_lease.as_ref(), None);
    k8s::lease::release_lease(&claimed_lease_name.unwrap(), &lease_client).await;
    let available_lease = k8s::lease::get_available_lease(lease_label, &lease_client).await;
    assert_eq!(available_lease.as_ref().unwrap(), "agent-startup-lease-1");
}

#[test(tokio::test)]
#[cfg_attr(not(feature = "k8s_tests"), ignore)]
async fn test_k8s_startup_leases_always_start() {
    let (server, received, shutdown_handle, ingester_addr) = common::start_http_ingester();
    tokio::time::sleep(tokio::time::Duration::from_millis(5000)).await;

    let client = Client::try_default().await.unwrap();

    let pod_name = "always-lease-listener";
    let pod_node_addr = start_line_proxy_pod(client.clone(), pod_name, "default", 30002).await;
    tokio::time::sleep(tokio::time::Duration::from_millis(5000)).await;

    let agent_name = "k8s-agent-lease";
    let agent_namespace = "k8s-agent-lease";

    let (server_result, _) = tokio::join!(server, async {
        tokio::time::sleep(tokio::time::Duration::from_millis(5000)).await;

        let agent_lease_name = "agent-startup-lease";
        let agent_lease_label = "process=logdna-agent-startup";

        // Create Agent
        let nss: Api<Namespace> = Api::all(client.clone());
        let ns = serde_json::from_value(serde_json::json!({
            "apiVersion": "v1",
            "kind": "Namespace",
            "metadata": {
                "name": agent_namespace
            }
        }))
        .unwrap();
        nss.create(&PostParams::default(), &ns).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        // Crate Startup Leases
        let agent_lease_api: Api<Lease> = Api::namespaced(client.clone(), agent_namespace);
        let lp = ListParams::default().labels(agent_lease_label);
        create_agent_startup_lease_list(client.clone(), agent_lease_name, agent_namespace).await;
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        // Assert leases were created
        let agent_lease_list = agent_lease_api.list(&lp).await;
        assert!(agent_lease_list.as_ref().unwrap().iter().count() > 0);

        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        let mock_ingester_socket_addr_str = create_mock_ingester_service(
            client.clone(),
            ingester_public_addr(ingester_addr),
            "ingest-service",
            agent_namespace,
            80,
        )
        .await;
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        create_agent_ds(
            client.clone(),
            agent_name,
            agent_namespace,
            &mock_ingester_socket_addr_str,
            "always",
            "always",
            "info",
            "always",
            "never",
            None,
        )
        .await;
        tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;

        print_pod_logs(
            client.clone(),
            agent_namespace,
            &format!("app={}", &agent_name),
        )
        .await;

        let messages = [
            "Agent data! 0\n",
            "Agent data! 1\n",
            "Agent data! 2\n",
            "Agent data! 3\n",
            "Agent data! 4\n",
        ];

        let mut pre_logger_stream = TcpStream::connect(pod_node_addr).await.unwrap();

        // Write some data.
        for msg in messages.iter() {
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
            pre_logger_stream.write_all(msg.as_bytes()).await.unwrap();
        }

        // Wait for the data to be received by the mock ingester
        tokio::time::sleep(tokio::time::Duration::from_millis(10_000)).await;

        let mut map = received.lock().await;
        let mut result = map.iter().find(|(k, _)| k.contains(pod_name));
        assert!(result.is_none());
        drop(map);

        tokio::time::sleep(tokio::time::Duration::from_millis(5000)).await;
        info!("RELEASE AGENT STARTUP LEASE...");
        k8s::lease::release_lease("agent-startup-lease-1", &agent_lease_api).await;
        let available_lease =
            k8s::lease::get_available_lease(agent_lease_label, &agent_lease_api).await;
        assert_eq!(available_lease.as_ref().unwrap(), "agent-startup-lease-1");
        tokio::time::sleep(tokio::time::Duration::from_millis(5000)).await;

        let mut post_logger_stream = TcpStream::connect(pod_node_addr).await.unwrap();
        // Write more data.
        for msg in messages.iter() {
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
            post_logger_stream.write_all(msg.as_bytes()).await.unwrap();
        }

        // Wait for the data to be received by the mock ingester
        tokio::time::sleep(tokio::time::Duration::from_millis(10_000)).await;

        map = received.lock().await;
        result = map.iter().find(|(k, _)| k.contains(pod_name));
        assert!(result.is_some());

        shutdown_handle();
    });

    // cleanup
    delete_daemonset(client.clone(), agent_name, agent_namespace).await;
    delete_service(client.clone(), "ingest-service", agent_namespace).await;
    delete_namespace(client.clone(), agent_namespace).await;

    server_result.unwrap();
}

#[test(tokio::test)]
#[cfg_attr(not(feature = "k8s_tests"), ignore)]
async fn test_k8s_startup_leases_never_start() {
    let (server, received, shutdown_handle, ingester_addr) = common::start_http_ingester();
    tokio::time::sleep(tokio::time::Duration::from_millis(5000)).await;

    let client = Client::try_default().await.unwrap();

    let pod_name = "off-lease-listener";
    let pod_node_addr = start_line_proxy_pod(client.clone(), pod_name, "default", 30003).await;

    tokio::time::sleep(tokio::time::Duration::from_millis(5000)).await;

    let agent_name = "k8s-agent-lease-off";
    let agent_namespace = "k8s-agent-lease-off";

    let (server_result, _) = tokio::join!(server, async {
        tokio::time::sleep(tokio::time::Duration::from_millis(5000)).await;

        // Create Agent
        let nss: Api<Namespace> = Api::all(client.clone());
        let ns = serde_json::from_value(serde_json::json!({
            "apiVersion": "v1",
            "kind": "Namespace",
            "metadata": {
                "name": agent_namespace
            }
        }))
        .unwrap();
        nss.create(&PostParams::default(), &ns).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        let mock_ingester_socket_addr_str = create_mock_ingester_service(
            client.clone(),
            ingester_public_addr(ingester_addr),
            "ingest-service",
            agent_namespace,
            80,
        )
        .await;
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        create_agent_ds(
            client.clone(),
            agent_name,
            agent_namespace,
            &mock_ingester_socket_addr_str,
            "always",
            "always",
            "info",
            "never",
            "never",
            None,
        )
        .await;
        tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;

        print_pod_logs(
            client.clone(),
            agent_namespace,
            &format!("app={}", &agent_name),
        )
        .await;

        let messages = [
            "Agent data! 0\n",
            "Agent data! 1\n",
            "Agent data! 2\n",
            "Agent data! 3\n",
            "Agent data! 4\n",
        ];

        let mut logger_stream = TcpStream::connect(pod_node_addr).await.unwrap();

        // Write some data.
        for msg in messages.iter() {
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
            logger_stream.write_all(msg.as_bytes()).await.unwrap();
        }

        // Wait for the data to be received by the mock ingester
        tokio::time::sleep(tokio::time::Duration::from_millis(10_000)).await;

        let map = received.lock().await;
        let result = map.iter().find(|(k, _)| k.contains(pod_name));
        assert!(result.is_some());

        shutdown_handle();
    });

    // cleanup
    delete_daemonset(client.clone(), agent_name, agent_namespace).await;
    delete_service(client.clone(), "ingest-service", agent_namespace).await;
    delete_namespace(client.clone(), agent_namespace).await;

    server_result.unwrap();
}

#[test(tokio::test)]
#[cfg_attr(not(feature = "k8s_tests"), ignore)]
async fn test_metric_stats_aggregator_enabled() {
    let (server, received, shutdown_handle, ingester_addr) = common::start_http_ingester();
    tokio::time::sleep(tokio::time::Duration::from_millis(5000)).await;

    let client = Client::try_default().await.unwrap();
    tokio::time::sleep(tokio::time::Duration::from_millis(5000)).await;

    let agent_name = "metric-aggregator";
    let agent_namespace = "metric-aggregator";

    let (server_result, _) = tokio::join!(server, async {
        tokio::time::sleep(tokio::time::Duration::from_millis(5000)).await;

        // Create Agent
        let nss: Api<Namespace> = Api::all(client.clone());
        let ns = serde_json::from_value(serde_json::json!({
            "apiVersion": "v1",
            "kind": "Namespace",
            "metadata": {
                "name": agent_namespace
            }
        }))
        .unwrap();
        nss.create(&PostParams::default(), &ns).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        create_agent_feature_lease(
            client.clone(),
            agent_namespace,
            "logdna-agent-reporter-lease",
            "logdna-agent-reporter",
        )
        .await;

        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        let mock_ingester_socket_addr_str = create_mock_ingester_service(
            client.clone(),
            ingester_public_addr(ingester_addr),
            "ingest-service",
            agent_namespace,
            80,
        )
        .await;
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        create_agent_ds(
            client.clone(),
            agent_name,
            agent_namespace,
            &mock_ingester_socket_addr_str,
            "never",
            "never",
            "info",
            "never",
            "always",
            None,
        )
        .await;
        tokio::time::sleep(tokio::time::Duration::from_millis(60000)).await;

        let map = received.lock().await;

        let mut found_pod_log = false;
        let mut found_node_log = false;
        let mut found_cluster_log = false;
        for (key, value) in map.iter() {
            if !key.eq("logdna-reporter") {
                continue;
            }
            for entry in &value.values {
                if entry.contains("{\"resource\":\"cluster\"") {
                    found_cluster_log = true;
                } else if entry.contains("{\"resource\":\"node\"") {
                    found_node_log = true;
                } else if entry.contains("{\"resource\":\"container\"") {
                    found_pod_log = true;
                }
            }
        }
        assert!(found_cluster_log);
        assert!(found_node_log);
        assert!(found_pod_log);

        shutdown_handle();
    });

    // cleanup
    delete_daemonset(client.clone(), agent_name, agent_namespace).await;
    delete_service(client.clone(), "ingest-service", agent_namespace).await;
    delete_namespace(client.clone(), agent_namespace).await;

    server_result.unwrap();
}

#[test(tokio::test)]
#[cfg_attr(not(feature = "k8s_tests"), ignore)]
async fn test_metric_stats_aggregator_disabled() {
    let (server, received, shutdown_handle, ingester_addr) = common::start_http_ingester();
    tokio::time::sleep(tokio::time::Duration::from_millis(5000)).await;

    let client = Client::try_default().await.unwrap();

    tokio::time::sleep(tokio::time::Duration::from_millis(5000)).await;

    let agent_name = "metric-aggregator-disabled";
    let agent_namespace = "metric-aggregator-disabled";

    let (server_result, _) = tokio::join!(server, async {
        tokio::time::sleep(tokio::time::Duration::from_millis(5000)).await;

        // Create Agent
        let nss: Api<Namespace> = Api::all(client.clone());
        let ns = serde_json::from_value(serde_json::json!({
            "apiVersion": "v1",
            "kind": "Namespace",
            "metadata": {
                "name": agent_namespace
            }
        }))
        .unwrap();
        nss.create(&PostParams::default(), &ns).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        create_agent_feature_lease(
            client.clone(),
            agent_namespace,
            "logdna-agent-reporter-lease",
            "logdna-agent-reporter",
        )
        .await;

        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        let mock_ingester_socket_addr_str = create_mock_ingester_service(
            client.clone(),
            ingester_public_addr(ingester_addr),
            "ingest-service",
            agent_namespace,
            80,
        )
        .await;
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        create_agent_ds(
            client.clone(),
            agent_name,
            agent_namespace,
            &mock_ingester_socket_addr_str,
            "always",
            "always",
            "info",
            "never",
            "never",
            None,
        )
        .await;
        tokio::time::sleep(tokio::time::Duration::from_millis(45000)).await;

        let map = received.lock().await;

        let mut found_pod_log = false;
        let mut found_node_log = false;
        let mut found_cluster_log = false;
        for (key, value) in map.iter() {
            if !key.eq("logdna-reporter") {
                continue;
            }
            for entry in &value.values {
                if entry.contains("{\"resource\":\"cluster\"") {
                    found_cluster_log = true;
                } else if entry.contains("{\"resource\":\"node\"") {
                    found_node_log = true;
                } else if entry.contains("{\"resource\":\"container\"") {
                    found_pod_log = true;
                }
            }
        }
        assert!(!found_cluster_log);
        assert!(!found_node_log);
        assert!(!found_pod_log);
        shutdown_handle();
    });

    // cleanup
    delete_daemonset(client.clone(), agent_name, agent_namespace).await;
    delete_service(client.clone(), "ingest-service", agent_namespace).await;
    delete_namespace(client.clone(), agent_namespace).await;

    server_result.unwrap();
}

#[test(tokio::test)]
#[cfg_attr(not(feature = "k8s_tests"), ignore)]
async fn test_feature_leader_grabbing_lease() {
    let (server, _received, shutdown_handle, _ingester_addr) = common::start_http_ingester();
    tokio::time::sleep(tokio::time::Duration::from_millis(5000)).await;

    let client = Client::try_default().await.unwrap();

    tokio::time::sleep(tokio::time::Duration::from_millis(5000)).await;

    let agent_name = "feature-leader";
    let agent_namespace = "feature-leader";

    let (server_result, _) = tokio::join!(server, async {
        tokio::time::sleep(tokio::time::Duration::from_millis(5000)).await;

        // Create Agent
        let nss: Api<Namespace> = Api::all(client.clone());
        let ns = serde_json::from_value(serde_json::json!({
            "apiVersion": "v1",
            "kind": "Namespace",
            "metadata": {
                "name": agent_namespace
            }
        }))
        .unwrap();
        nss.create(&PostParams::default(), &ns).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        create_agent_feature_lease(
            client.clone(),
            agent_namespace,
            "logdna-agent-reporter-lease",
            "logdna-agent-reporter",
        )
        .await;

        let lease_name = "logdna-agent-reporter-lease".to_string();
        let lease_api = k8s::lease::get_k8s_lease_api(agent_namespace, client.clone()).await;

        let feature_leader = FeatureLeader::new(
            agent_namespace.to_string(),
            lease_name.clone(),
            agent_name.to_string(),
            lease_api,
        );

        let is_claim_success = feature_leader.try_claim_feature_leader().await;
        assert!(is_claim_success);
        let is_renewed = feature_leader.renew_feature_leader().await;
        assert!(is_renewed);
        let mut taken_over = feature_leader.try_claim_feature_leader().await;
        assert!(!taken_over);
        tokio::time::sleep(tokio::time::Duration::from_millis(10000)).await;
        taken_over = feature_leader.try_claim_feature_leader().await;
        assert!(taken_over);

        shutdown_handle();
    });

    // cleanup
    delete_daemonset(client.clone(), agent_name, agent_namespace).await;
    delete_service(client.clone(), "ingest-service", agent_namespace).await;
    delete_namespace(client.clone(), agent_namespace).await;

    server_result.unwrap();
}

#[test(tokio::test)]
#[cfg_attr(not(feature = "k8s_tests"), ignore)]
async fn test_retry_line_with_missing_pod_metadata() {
    let (server, received, shutdown_handle, ingester_addr) = common::start_http_ingester();

    tokio::time::sleep(tokio::time::Duration::from_millis(3000)).await;

    let client = Client::try_default().await.unwrap();

    let socat_pod_name = "test-retry-line-socat-listener";
    let default_namespace = "default";

    let socat_pod_node_addr =
        start_line_proxy_pod(client.clone(), socat_pod_name, default_namespace, 30004).await;

    assert!(
        wait_for_pod_ready(
            client.clone(),
            socat_pod_name,
            default_namespace,
            tokio::time::Duration::from_millis(10_000),
        )
        .await
    );
    info!(
        "socat pod {} in {} is ready",
        socat_pod_name, default_namespace
    );

    let agent_name = "test-retry-line-agent";
    let agent_namespace = "test-retry-line";

    let (server_result, _) = tokio::join!(server, async {
        let ns = serde_json::from_value(serde_json::json!({
            "apiVersion": "v1",
            "kind": "Namespace",
            "metadata": {
                "name": agent_namespace
            }
        }))
        .unwrap();
        let nss: Api<Namespace> = Api::all(client.clone());
        nss.create(&PostParams::default(), &ns).await.unwrap();

        let mock_ingester_socket_addr_str = create_mock_ingester_service(
            client.clone(),
            ingester_public_addr(ingester_addr),
            "ingest-service",
            agent_namespace,
            80,
        )
        .await;

        create_agent_ds(
            client.clone(),
            agent_name,
            agent_namespace,
            &mock_ingester_socket_addr_str,
            "never",
            "always",
            "info",
            "never",
            "never",
            Some(vec![
                (config::env_vars::METADATA_RETRY_DELAY, "5"),
                (config::env_vars::MOCK_NO_PODS, "true"), // k8s log lines will not have metadata
            ]),
        )
        .await;

        info!("daemonset {} in {} is ready", agent_name, agent_namespace);

        let messages = [
            "Hello, World! 0\n",
            "Hello, World! 1\n",
            "Hello, World! 2\n",
            "Hello, World! 3\n",
            "Hello, World! 4\n",
        ];

        let mut logger_stream = TcpStream::connect(socat_pod_node_addr).await.unwrap();

        for msg in messages.iter() {
            // Write log lines
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
            logger_stream.write_all(msg.as_bytes()).await.unwrap();
        }

        // Wait for the data to be received by the mock ingester
        // and delayed lines (all) delay time to expire
        tokio::time::sleep(tokio::time::Duration::from_millis(10_000)).await;

        // get socat pod log stats from ingester
        let map = received.lock().await;
        let result = map.iter().find(|(k, _)| k.contains(socat_pod_name));
        assert!(result.is_some());

        // assert we got all lines
        let (_, pod_file_info) = result.unwrap();
        assert!(pod_file_info.lines == 5);

        // assert processing steps triggered
        let log_lines = vec![
            "Enabling filesystem",
            "Pod metadata is missing for line (retries=1)",
            "Pod metadata is missing for line (retries=0)",
        ];
        info!("asserting log lines: {:?}", log_lines);
        assert_log_lines(
            client.clone(),
            agent_namespace,
            &format!("app={}", &agent_name),
            log_lines,
            None,
            false,
        )
        .await;

        info!("success");

        shutdown_handle();
    });

    // cleanup
    delete_daemonset(client.clone(), agent_name, agent_namespace).await;
    delete_namespace(client.clone(), agent_namespace).await;
    delete_pod(client.clone(), socat_pod_name, default_namespace).await;
    delete_service(client.clone(), socat_pod_name, default_namespace).await;

    server_result.unwrap();
}

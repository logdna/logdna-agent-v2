use pnet::datalink;
use std::net::{SocketAddr, ToSocketAddrs};

use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

use futures::{StreamExt, TryStreamExt};

use k8s_openapi::api::apps::v1::DaemonSet;
use k8s_openapi::api::core::v1::{Endpoints, Namespace, Pod, Service, ServiceAccount};
use k8s_openapi::api::rbac::v1::{ClusterRole, ClusterRoleBinding, Role, RoleBinding};
use kube::api::{Api, ListParams, LogParams, PostParams, WatchEvent};
use kube::{Client, ResourceExt};

mod common;

// workaround for unused functions in different features: https://github.com/rust-lang/rust/issues/46379
pub use common::*;

async fn print_pod_logs(client: Client, namespace: &str, label: &str) {
    let pods: Api<Pod> = Api::namespaced(client, namespace);
    let lp = ListParams::default().labels(label);
    pods.list(&lp).await.unwrap().into_iter().for_each(|p| {
        let pods = pods.clone();
        tokio::spawn({
            async move {
                let mut logs = pods
                    .log_stream(
                        &p.name(),
                        &LogParams {
                            follow: true,
                            tail_lines: None,
                            ..LogParams::default()
                        },
                    )
                    .await
                    .unwrap()
                    .boxed();

                log::debug!("Logging agent pod {}", p.name());
                while let Some(line) = logs.next().await {
                    log::debug!("LOG {:?}", String::from_utf8_lossy(&line.unwrap()));
                }
            }
        });
    })
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
    let lp = ListParams::default()
        .fields(&format!("metadata.name={}", pod_name))
        .timeout(60);
    let mut stream = pods.watch(&lp, "0").await.unwrap().boxed();

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
    let default_ip = datalink::interfaces()
        .iter()
        .find(|e| e.is_up() && !e.is_loopback() && !e.ips.is_empty())
        .expect("container should have an interface")
        .ips
        .get(0)
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

async fn create_agent_ds(
    client: Client,
    agent_name: &str,
    agent_namespace: &str,
    ingester_addr: &str,
    log_k8s_events: &str,
    enrich_logs_with_k8s: &str,
    agent_log_level: &str,
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
        "logdna-agent-kind:building",
        ingester_addr,
        "false",
        agent_name,
        log_k8s_events,
        enrich_logs_with_k8s,
        agent_log_level,
    );
    //
    let dss: Api<DaemonSet> = Api::namespaced(client.clone(), agent_namespace);

    dss.create(&PostParams::default(), &ds).await.unwrap();
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    let agent_pods: Api<Pod> = Api::namespaced(client.clone(), agent_namespace);

    let lp = ListParams::default()
        .labels(&format!("app={}", agent_name))
        .timeout(60);
    let mut stream = agent_pods.watch(&lp, "0").await.unwrap().boxed();

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

fn get_agent_ds_yaml(
    image_name: &str,
    ingester_addr: &str,
    use_ssl: &str,
    agent_name: &str,
    log_k8s_events: &str,
    enrich_logs_with_k8s: &str,
    log_level: &str,
) -> DaemonSet {
    serde_json::from_value(serde_json::json!({
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
                                    "name": "LOGDNA_LOG_K8S_EVENTS",
                                    "value": log_k8s_events
                                },
                                {
                                    "name": "LOGDNA_USE_K8S_LOG_ENRICHMENT",
                                    "value": enrich_logs_with_k8s,
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
                                    "memory": "500Mi"
                                },
                                "requests": {
                                    "cpu": "20m"
                                }
                            },
                            "securityContext": {
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
    ))
    .expect("failed to serialize DS manifest")
}

#[tokio::test]
#[cfg_attr(not(feature = "k8s_tests"), ignore)]
async fn test_k8s_connection() {
    let client = Client::try_default().await.unwrap();
    let pods: Api<Pod> = Api::all(client);
    let lp = ListParams::default(); // for this app only
    let pod_list = pods.list(&lp).await.unwrap();
    assert!(pod_list.iter().count() > 0);
}

#[tokio::test]
#[cfg_attr(not(feature = "k8s_tests"), ignore)]
async fn test_k8s_enrichment() {
    let _ = env_logger::Builder::from_default_env().try_init();
    let (server, received, shutdown_handle, ingester_addr) = common::start_http_ingester();

    tokio::time::sleep(tokio::time::Duration::from_millis(3000)).await;

    let client = Client::try_default().await.unwrap();

    let pod_name = "socat-listener";
    let pod_node_addr = start_line_proxy_pod(client.clone(), pod_name, "default", 30001).await;

    tokio::time::sleep(tokio::time::Duration::from_millis(5000)).await;

    let (server_result, _) = tokio::join!(server, async {
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        // Create Agent
        let agent_name = "k8s-enrichment";
        let agent_namespace = "k8s-enrichment";

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
            "warn",
        )
        .await;

        tokio::time::sleep(tokio::time::Duration::from_millis(10_000)).await;

        print_pod_logs(
            client.clone(),
            agent_namespace,
            &format!("app={}", &agent_name),
        )
        .await;

        let messages = vec![
            "Hello, World! 0\n",
            "Hello, World! 2\n",
            "Hello, World! 3\n",
            "Hello, World! 1\n",
            "Hello, World! 4\n",
        ];

        let mut logger_stream = TcpStream::connect(pod_node_addr).await.unwrap();

        for msg in messages.iter() {
            // Write some data.
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
            logger_stream.write_all(msg.as_bytes()).await.unwrap();
        }

        // Wait for the data to be received by the mock ingester
        tokio::time::sleep(tokio::time::Duration::from_millis(10_000)).await;

        let map = received.lock().await;

        let result = map.iter().find(|(k, _)| k.contains(pod_name));
        assert!(result.is_some());

        let (_, pod_file_info) = result.unwrap();
        let label = pod_file_info.label.as_ref();
        assert!(label.is_some());
        assert_eq!(label.unwrap()["app.kubernetes.io/name"], pod_name);
        assert_eq!(
            label.unwrap()["app.kubernetes.io/instance"],
            format!("{}-instance", pod_name)
        );
        let values = &pod_file_info.values;
        assert!(values.len() == 5);
        for (left, right) in values.iter().zip(messages.iter()) {
            assert!(left.ends_with(right));
        }

        let result = map.iter().find(|(k, _)| k.contains("sample-pod"));
        assert!(result.is_some());
        let (_, pod_file_info) = result.unwrap();
        let label = pod_file_info.label.as_ref();
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

        shutdown_handle();
    });

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

#[tokio::test]
#[cfg_attr(not(feature = "k8s_tests"), ignore)]
async fn test_k8s_events_logged() {
    let _ = env_logger::Builder::from_default_env().try_init();

    let (server, received, shutdown_handle, ingester_addr) = common::start_http_ingester();

    let client = Client::try_default().await.unwrap();

    let (server_result, _) = tokio::join!(server, async {
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        // Create Agent
        let agent_name = "k8s-events-logged";
        let agent_namespace = "k8s-events-logged";

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

    server_result.unwrap();
}

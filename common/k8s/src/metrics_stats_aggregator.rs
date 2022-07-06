use anyhow::Result;
use core::result::Result::Ok;

use futures::{stream, Stream, StreamExt};
use http::types::body::LineBuilder;

use k8s_openapi::api::core::v1::{Container, ContainerStatus, Node, Pod};
use kube::{
    api::{Api, DynamicObject, GroupVersionKind, ListParams, ObjectList},
    discovery, Client,
};
use serde_json::Value;

use std::collections::HashMap;
use std::time::Duration;
use tokio::time::sleep;

use crate::kube_stats::{
    cluster_stats::ClusterStats,
    container_stats::ContainerStats,
    controller_stats::ControllerStats,
    extended_pod_stats::ExtendedPodStats,
    node_stats::{NodeContainerStats, NodePodStats, NodeStats},
    pod_stats::PodStats,
};

pub struct MetricsStatsAggregator {
    pub client: Client,
}

impl MetricsStatsAggregator {
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    pub fn start_metrics_call_task(self) -> impl Stream<Item = LineBuilder> {
        stream::unfold(self.client, |client| async {
            sleep(Duration::from_millis(30000)).await;
            Some((
                match self::process_reporter_info(client.clone()).await {
                    Ok((pods_strings, node_strings, cluster_stats_string)) => Some(
                        stream::iter(
                            pods_strings
                                .into_iter()
                                .chain(node_strings.into_iter())
                                .chain(Some(cluster_stats_string).into_iter()),
                        )
                        .map(|line| {
                            LineBuilder::new()
                                .line(line)
                                .file("logdna-reporter".to_string())
                        }),
                    ),
                    Err(e) => {
                        error!("Failed To Gather Metrics Server Info {}", e);
                        None
                    }
                },
                client,
            ))
        })
        .filter_map(|x| async { x })
        .flatten()
    }
}

async fn process_reporter_info(
    client: Client,
) -> anyhow::Result<(Vec<String>, Vec<String>, String)> {
    let pods = self::get_all_pods(client.clone()).await?;
    let nodes = self::get_all_nodes(client.clone()).await?;
    let pod_metrics = self::call_metric_api("PodMetrics", client.clone()).await?;
    let node_metrics = self::call_metric_api("NodeMetrics", client.clone()).await?;

    let mut controller_map: HashMap<String, ControllerStats> = HashMap::new();
    let mut node_pod_counts_map: HashMap<String, NodePodStats> = HashMap::new();
    let mut node_container_counts_map: HashMap<String, NodeContainerStats> = HashMap::new();
    let mut pod_usage_map: HashMap<String, Value> = HashMap::new();
    let mut node_usage_map: HashMap<String, Value> = HashMap::new();

    let mut extended_pod_stats: Vec<ExtendedPodStats> = Vec::new();
    let mut node_stats: Vec<NodeStats> = Vec::new();

    build_pod_metric_map(pod_metrics, &mut pod_usage_map);
    process_pods(
        pods,
        &mut controller_map,
        pod_usage_map,
        &mut extended_pod_stats,
        &mut node_pod_counts_map,
        &mut node_container_counts_map,
    );
    let pods_strings = format_pod_str(extended_pod_stats, controller_map);

    build_node_metric_map(node_metrics, &mut node_usage_map);
    process_nodes(
        nodes,
        node_usage_map,
        &mut node_stats,
        &mut node_pod_counts_map,
        &mut node_container_counts_map,
    );
    let node_strings = format_node_str(&node_stats);

    let cluster_stats = build_cluster_stats(&node_stats);
    let cluster_stats_string = format_cluster_str(&cluster_stats);

    Ok((pods_strings, node_strings, cluster_stats_string))
}

fn build_pod_metric_map(
    pod_metrics: ObjectList<DynamicObject>,
    pod_usage_map: &mut HashMap<String, Value>,
) {
    for pod_metric in pod_metrics {
        let containers = pod_metric.data["containers"].as_array();

        if let Some(..) = containers {
            for container in containers.unwrap() {
                let container_name = container["name"].as_str();

                if container_name.is_none() {
                    continue;
                }

                pod_usage_map.insert(
                    container_name.unwrap().to_string(),
                    container["usage"].clone(),
                );
            }
        }
    }
}

fn build_node_metric_map(
    node_metrics: ObjectList<DynamicObject>,
    node_usage_map: &mut HashMap<String, Value>,
) {
    for node_metric in node_metrics {
        let node_name = node_metric
            .metadata
            .name
            .unwrap_or_else(|| "NONE".to_string());
        let usage = &node_metric.data["usage"];

        node_usage_map.insert(node_name, usage.clone());
    }
}

fn build_cluster_stats(node_stats: &Vec<NodeStats>) -> ClusterStats {
    let mut cluster_stats = ClusterStats::new();

    for node_stat in node_stats {
        cluster_stats.containers_init += node_stat.containers_init;
        cluster_stats.containers_ready += node_stat.containers_ready;
        cluster_stats.containers_running += node_stat.containers_running;
        cluster_stats.containers_terminated += node_stat.containers_terminated;
        cluster_stats.containers_total += node_stat.containers_total;
        cluster_stats.containers_waiting += node_stat.containers_waiting;
        cluster_stats.cpu_allocatable += node_stat.cpu_allocatable.unwrap_or(0);
        cluster_stats.cpu_capacity += node_stat.cpu_capacity.unwrap_or(0);
        cluster_stats.cpu_usage += node_stat.cpu_usage;
        cluster_stats.memory_allocatable += node_stat.memory_allocatable.unwrap_or(0);
        cluster_stats.memory_capacity += node_stat.memory_capacity.unwrap_or(0);
        cluster_stats.memory_usage += node_stat.memory_usage;
        cluster_stats.pods_allocatable += node_stat.pods_allocatable.unwrap_or(0);
        cluster_stats.pods_capacity += node_stat.pods_capacity.unwrap_or(0);
        cluster_stats.pods_failed += node_stat.pods_failed;
        cluster_stats.pods_pending += node_stat.pods_pending;
        cluster_stats.pods_running += node_stat.pods_running;
        cluster_stats.pods_succeeded += node_stat.pods_succeeded;
        cluster_stats.pods_total += node_stat.pods_total;
        cluster_stats.pods_unknown += node_stat.pods_unknown;
        cluster_stats.nodes_total += 1;

        if node_stat.ready.unwrap_or(false) {
            cluster_stats.nodes_ready += 1;
        } else {
            cluster_stats.nodes_notready += 1;
        }

        if node_stat.unschedulable.unwrap_or(false) {
            cluster_stats.nodes_unschedulable += 1;
        }
    }

    cluster_stats
}

fn format_pod_str(
    extended_pod_stats: Vec<ExtendedPodStats>,
    controller_map: HashMap<String, ControllerStats>,
) -> Vec<String> {
    let mut pod_strings: Vec<String> = Vec::new();
    for mut translated_pod_container in extended_pod_stats {
        let controller_key = format!(
            "{}.{}.{}",
            translated_pod_container.pod_stats.namespace.clone(),
            translated_pod_container.pod_stats.controller_type.clone(),
            translated_pod_container.pod_stats.controller.clone()
        );

        let controller_stats = controller_map.get(&controller_key);

        if let Some(..) = controller_stats {
            translated_pod_container
                .controller_stats
                .copy_stats(controller_stats.unwrap());
        }

        let display_str = format!(
            r#"{{"kube":{}}}"#,
            serde_json::to_string(&translated_pod_container).unwrap_or_else(|_| String::from(""))
        );

        pod_strings.push(display_str)
    }
    pod_strings
}

fn format_node_str(nodes: &Vec<NodeStats>) -> Vec<String> {
    let mut node_strings: Vec<String> = Vec::new();
    for node in nodes {
        let node_str = format!(
            r#"{{"kube":{}}}"#,
            serde_json::to_string(&node).unwrap_or_else(|_| String::from(""))
        );
        node_strings.push(node_str);
    }
    node_strings
}

fn format_cluster_str(cluster_stats: &ClusterStats) -> String {
    let cluster_string = format!(
        r#"{{"kube":{}}}"#,
        serde_json::to_string(&cluster_stats).unwrap_or_else(|_| String::from(""))
    );
    cluster_string
}

fn process_pods(
    pods: ObjectList<Pod>,
    controller_map: &mut HashMap<String, ControllerStats>,
    pod_usage_map: HashMap<String, Value>,
    extended_pod_stats: &mut Vec<ExtendedPodStats>,
    node_pod_counts_map: &mut HashMap<String, NodePodStats>,
    node_container_counts_map: &mut HashMap<String, NodeContainerStats>,
) {
    for pod in pods {
        if pod.spec.is_none() || pod.status.is_none() {
            continue;
        }

        let status = pod.status.as_ref().unwrap();
        let spec = pod.spec.as_ref().unwrap();

        if status.conditions.is_none() || status.container_statuses.is_none() {
            continue;
        }

        let translated_pod = PodStats::builder(&pod).build();

        let node = translated_pod.node.clone();
        let phase = translated_pod.phase.clone();

        let node_pod_stat = node_pod_counts_map
            .entry(node.clone())
            .or_insert_with(NodePodStats::new);
        node_pod_stat.inc(&phase);

        let controller_key = format!(
            "{}.{}.{}",
            translated_pod.namespace.clone(),
            translated_pod.controller_type.clone(),
            translated_pod.controller.clone()
        );

        let controller = controller_map
            .entry(controller_key.clone())
            .or_insert_with(ControllerStats::new);

        let conditions = status.conditions.as_ref().unwrap();
        if conditions
            .iter()
            .any(|c| c.status.to_lowercase() == "true" && c.type_.to_lowercase() == "ready")
        {
            controller.inc_pods_ready();
        }

        controller.inc_pods_total();

        let mut container_status_map = HashMap::new();

        let default_status_vec = Vec::new();
        for status in status
            .container_statuses
            .as_ref()
            .unwrap_or(&default_status_vec)
            .iter()
            .chain(
                status
                    .init_container_statuses
                    .as_ref()
                    .unwrap_or(&default_status_vec)
                    .iter(),
            )
        {
            container_status_map.insert(status.name.clone(), status.clone());

            let controller = controller_map
                .entry(controller_key.clone())
                .or_insert_with(ControllerStats::new);

            controller.inc_containers_total();

            if status.ready {
                controller.inc_containers_ready();
            }
        }

        for container in spec.containers.iter() {
            if container.name.is_empty()
                || container.image.is_none()
                || container.resources.is_none()
            {
                continue;
            }

            let container_status = container_status_map.get(&container.name);

            if container_status.is_none() {
                continue;
            }

            let extended_pod_stat = build_extended_pod_stat(
                &pod_usage_map,
                container,
                container_status,
                &translated_pod,
            );

            if let Some(..) = extended_pod_stat {
                let extended_pod_stat = extended_pod_stat.unwrap();

                let node_container_stat = node_container_counts_map
                    .entry(node.to_string())
                    .or_insert_with(NodeContainerStats::new);

                node_container_stat.inc(
                    &extended_pod_stat.container_stats.state,
                    extended_pod_stat.container_stats.ready,
                    false,
                );

                extended_pod_stats.push(extended_pod_stat);
            }
        }

        let default_container_vec: Vec<Container> = Vec::new();
        for init_container in spec
            .init_containers
            .as_ref()
            .unwrap_or(&default_container_vec)
        {
            if init_container.name.is_empty()
                || init_container.image.is_none()
                || init_container.resources.is_none()
            {
                continue;
            }

            let container_status = container_status_map.get(&init_container.name);

            if container_status.is_none() {
                continue;
            }

            let extended_pod_stat = build_extended_pod_stat(
                &pod_usage_map,
                init_container,
                container_status,
                &translated_pod,
            );

            if let Some(..) = extended_pod_stat {
                let extended_pod_stat = extended_pod_stat.unwrap();

                let node_container_stat = node_container_counts_map
                    .entry(node.to_string())
                    .or_insert_with(NodeContainerStats::new);

                node_container_stat.inc(
                    &extended_pod_stat.container_stats.state,
                    extended_pod_stat.container_stats.ready,
                    true,
                );

                extended_pod_stats.push(extended_pod_stat);
            }
        }
    }
}

fn build_extended_pod_stat(
    pod_usage_map: &HashMap<String, Value>,
    container: &Container,
    container_status: Option<&ContainerStatus>,
    translated_pod: &PodStats,
) -> Option<ExtendedPodStats> {
    let usage = pod_usage_map.get(&container.name);
    if let Some(..) = usage {
        let translated_container = ContainerStats::builder(
            container,
            container_status.as_ref().unwrap(),
            container_status.unwrap().state.as_ref().unwrap(),
            usage.unwrap()["cpu"].as_str().unwrap_or(""),
            usage.unwrap()["memory"].as_str().unwrap_or(""),
        )
        .build();

        return Some(ExtendedPodStats::new(
            translated_pod.clone(),
            translated_container,
        ));
    }

    None
}

fn process_nodes(
    nodes: ObjectList<Node>,
    node_usage_map: HashMap<String, Value>,
    output_node_vec: &mut Vec<NodeStats>,
    node_pod_counts_map: &mut HashMap<String, NodePodStats>,
    node_container_counts_map: &mut HashMap<String, NodeContainerStats>,
) {
    for node in nodes {
        if node.spec.is_none() || node.status.is_none() || node.metadata.name.is_none() {
            continue;
        }

        let name = node.metadata.name.as_ref().unwrap();

        let default_node_container_stats = NodeContainerStats::new();
        let default_pod_container_stats = NodePodStats::new();

        let node_container_stats = node_container_counts_map
            .get(name)
            .unwrap_or(&default_node_container_stats);
        let node_pod_stats = node_pod_counts_map
            .get(name)
            .unwrap_or(&default_pod_container_stats);

        let usage = node_usage_map.get(name);
        if let Some(..) = usage {
            let translated_node = NodeStats::builder(
                &node,
                node_pod_stats,
                node_container_stats,
                usage.unwrap()["cpu"].as_str().unwrap_or(""),
                usage.unwrap()["memory"].as_str().unwrap_or(""),
            )
            .build();

            output_node_vec.push(translated_node);
        }
    }
}

async fn call_metric_api(
    kind: &str,
    client: Client,
) -> Result<ObjectList<DynamicObject>, kube::Error> {
    let gvk = GroupVersionKind::gvk("metrics.k8s.io", "v1beta1", kind);
    let (ar, _caps) = discovery::pinned_kind(&client, &gvk).await?;
    let api = Api::<DynamicObject>::all_with(client, &ar);

    let items = api.list(&ListParams::default()).await;

    items
}

async fn get_all_nodes(client: Client) -> Result<ObjectList<Node>, kube::Error> {
    let api: Api<Node> = Api::all(client);
    let nodes = api.list(&ListParams::default()).await;

    nodes
}

async fn get_all_pods(client: Client) -> Result<ObjectList<Pod>, kube::Error> {
    let api: Api<Pod> = Api::all(client);
    let pods = api.list(&ListParams::default()).await;

    pods
}

#[cfg(test)]
mod tests {

    use std::collections::HashMap;

    use k8s_openapi::api::core::v1::{Node, Pod};
    use kube::api::{ListMeta, ObjectList};
    use serde_json::Value;

    use crate::{
        kube_stats::{
            controller_stats::ControllerStats,
            extended_pod_stats::ExtendedPodStats,
            node_stats::{NodeContainerStats, NodePodStats, NodeStats},
        },
        metrics_stats_aggregator::{process_nodes, process_pods},
    };

    use super::build_cluster_stats;

    #[tokio::test]
    async fn test_build_cluster_stats() {
        let mut node_stats: Vec<NodeStats> = Vec::new();

        let stats1 = generate_node();
        let stats2 = generate_node();
        let stats3 = generate_node();
        let stats4 = generate_node();

        node_stats.push(stats1);
        node_stats.push(stats2);
        node_stats.push(stats3);
        node_stats.push(stats4);

        let result = build_cluster_stats(&node_stats);

        assert_eq!(result.containers_init, 4);
        assert_eq!(result.containers_ready, 4);
        assert_eq!(result.containers_running, 4);
        assert_eq!(result.containers_terminated, 4);
        assert_eq!(result.containers_total, 4);
        assert_eq!(result.containers_waiting, 4);
        assert_eq!(result.pods_allocatable, 4);
        assert_eq!(result.pods_capacity, 4);
        assert_eq!(result.pods_failed, 4);
        assert_eq!(result.pods_pending, 4);
        assert_eq!(result.pods_running, 4);
        assert_eq!(result.pods_succeeded, 4);
        assert_eq!(result.pods_total, 4);
        assert_eq!(result.pods_unknown, 4);
        assert_eq!(result.nodes_notready, 0);
        assert_eq!(result.nodes_ready, 4);
        assert_eq!(result.nodes_total, 4);
        assert_eq!(result.nodes_unschedulable, 0);
    }

    #[tokio::test]
    async fn test_process_pods_does_not_panic() {
        let pods = ObjectList::<Pod> {
            metadata: ListMeta {
                continue_: None,
                remaining_item_count: None,
                resource_version: None,
                self_link: None,
            },
            items: Vec::new(),
        };

        let mut controller_map: HashMap<String, ControllerStats> = HashMap::new();
        let mut node_pod_counts_map: HashMap<String, NodePodStats> = HashMap::new();
        let mut node_container_counts_map: HashMap<String, NodeContainerStats> = HashMap::new();
        let pod_usage_map: HashMap<String, Value> = HashMap::new();

        let mut extended_pod_stats: Vec<ExtendedPodStats> = Vec::new();

        process_pods(
            pods,
            &mut controller_map,
            pod_usage_map,
            &mut extended_pod_stats,
            &mut node_pod_counts_map,
            &mut node_container_counts_map,
        );
    }

    #[tokio::test]
    async fn test_process_nodes_does_not_panic() {
        let nodes = ObjectList::<Node> {
            metadata: ListMeta {
                continue_: None,
                remaining_item_count: None,
                resource_version: None,
                self_link: None,
            },
            items: Vec::new(),
        };

        let mut node_pod_counts_map: HashMap<String, NodePodStats> = HashMap::new();
        let mut node_container_counts_map: HashMap<String, NodeContainerStats> = HashMap::new();
        let node_usage_map: HashMap<String, Value> = HashMap::new();

        let mut node_stats: Vec<NodeStats> = Vec::new();

        process_nodes(
            nodes,
            node_usage_map,
            &mut node_stats,
            &mut node_pod_counts_map,
            &mut node_container_counts_map,
        );
    }

    fn generate_node() -> NodeStats {
        NodeStats {
            age: 0,
            container_runtime_version: "".to_string(),
            containers_init: 1,
            containers_ready: 1,
            containers_running: 1,
            containers_terminated: 1,
            containers_total: 1,
            containers_waiting: 1,
            cpu_allocatable: Some(1),
            cpu_capacity: Some(1),
            cpu_usage: 0,
            created: 0,
            ip_external: "".to_string(),
            ip: "".to_string(),
            kernel_version: "".to_string(),
            kubelet_version: "".to_string(),
            memory_allocatable: Some(1),
            memory_capacity: Some(1),
            memory_usage: 0,
            node: "".to_string(),
            os_image: "".to_string(),
            pods_failed: 1,
            pods_pending: 1,
            pods_running: 1,
            pods_succeeded: 1,
            pods_total: 1,
            pods_unknown: 1,
            ready_heartbeat_age: 0,
            ready_heartbeat_time: 0,
            ready_message: "".to_string(),
            ready_status: "".to_string(),
            ready_transition_age: 0,
            ready_transition_time: 0,
            ready: Some(true),
            unschedulable: None,
            pods_allocatable: Some(1),
            pods_capacity: Some(1),
            resource: "node".to_string(),
            r#type: "metric".to_string(),
        }
    }
}

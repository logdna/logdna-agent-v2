use chrono::Utc;
use k8s_openapi::api::core::v1::Node;
use serde::{Deserialize, Serialize};

use super::helpers::{convert_cpu_usage_to_milli, convert_memory_usage_to_bytes};

#[derive(Serialize, Deserialize)]
pub struct NodeStats {
    pub resource: String,
    pub r#type: String,
    pub age: i64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub container_runtime_version: String,
    pub containers_init: u32,
    pub containers_ready: u32,
    pub containers_running: u32,
    pub containers_terminated: u32,
    pub containers_total: u32,
    pub containers_waiting: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(with = "::serde_with::rust::unwrap_or_skip")]
    pub cpu_allocatable: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(with = "::serde_with::rust::unwrap_or_skip")]
    pub cpu_capacity: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(with = "::serde_with::rust::unwrap_or_skip")]
    pub cpu_usage: Option<u32>,
    pub created: i64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub ip_external: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub ip: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub kernel_version: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub kubelet_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(with = "::serde_with::rust::unwrap_or_skip")]
    pub memory_allocatable: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(with = "::serde_with::rust::unwrap_or_skip")]
    pub memory_capacity: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(with = "::serde_with::rust::unwrap_or_skip")]
    pub pods_allocatable: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(with = "::serde_with::rust::unwrap_or_skip")]
    pub pods_capacity: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(with = "::serde_with::rust::unwrap_or_skip")]
    pub memory_usage: Option<u64>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub node: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub os_image: String,
    pub pods_failed: u32,
    pub pods_pending: u32,
    pub pods_running: u32,
    pub pods_succeeded: u32,
    pub pods_total: u32,
    pub pods_unknown: u32,
    pub ready_heartbeat_age: i64,
    pub ready_heartbeat_time: i64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub ready_message: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub ready_status: String,
    pub ready_transition_age: i64,
    pub ready_transition_time: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(with = "::serde_with::rust::unwrap_or_skip")]
    pub ready: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(with = "::serde_with::rust::unwrap_or_skip")]
    pub unschedulable: Option<bool>,
}

impl NodeStats {
    pub fn builder<'a>(
        n: &'a Node,
        n_pods: &'a NodePodStats,
        n_containers: &'a NodeContainerStats,
        raw_cpu_usage: &'a str,
        raw_memory_usage: &'a str,
    ) -> NodeStatsBuilder<'a> {
        NodeStatsBuilder {
            n,
            n_pods,
            n_containers,
            raw_cpu_usage,
            raw_memory_usage,
        }
    }
}

pub struct NodeStatsBuilder<'a> {
    n: &'a Node,
    n_pods: &'a NodePodStats,
    n_containers: &'a NodeContainerStats,
    raw_cpu_usage: &'a str,
    raw_memory_usage: &'a str,
}

impl NodeStatsBuilder<'_> {
    pub fn new<'a>(
        n: &'a Node,
        n_pods: &'a NodePodStats,
        n_containers: &'a NodeContainerStats,
        raw_cpu_usage: &'a str,
        raw_memory_usage: &'a str,
    ) -> NodeStatsBuilder<'a> {
        NodeStatsBuilder {
            n,
            n_pods,
            n_containers,
            raw_cpu_usage,
            raw_memory_usage,
        }
    }

    pub fn build(self) -> NodeStats {
        let mut age = 0;

        let memory_usage = convert_memory_usage_to_bytes(self.raw_memory_usage);
        let cpu_usage = convert_cpu_usage_to_milli(self.raw_cpu_usage);

        let mut container_runtime_version = String::new();
        let mut ip = String::new();
        let mut ip_external = String::new();
        let mut kernel_version = String::new();
        let mut kubelet_version = String::new();
        let mut node = String::new();
        let mut os_image = String::new();
        let mut ready_message = String::new();
        let mut ready_status = String::new();

        let mut cpu_allocatable: Option<u32> = None;
        let mut cpu_capacity: Option<u32> = None;
        let mut memory_allocatable: Option<u64> = None;
        let mut memory_capacity: Option<u64> = None;
        let mut pods_allocatable: Option<u64> = None;
        let mut pods_capacity: Option<u64> = None;

        let mut created = 0;
        let mut ready_heartbeat_age = 0;
        let mut ready_heartbeat_time = 0;
        let mut ready_transition_age = 0;
        let mut ready_transition_time = 0;

        let mut ready: Option<bool> = None;
        let mut unschedulable: Option<bool> = None;

        let status = &self.n.status;
        let spec = &self.n.spec;

        let containers_init = self.n_containers.containers_init;
        let containers_ready = self.n_containers.containers_ready;
        let containers_running = self.n_containers.containers_running;
        let containers_terminated = self.n_containers.containers_terminated;
        let containers_total = self.n_containers.containers_total;
        let containers_waiting = self.n_containers.containers_waiting;

        let pods_failed = self.n_pods.pods_failed;
        let pods_pending = self.n_pods.pods_pending;
        let pods_running = self.n_pods.pods_running;
        let pods_succeeded = self.n_pods.pods_succeeded;
        let pods_total = self.n_pods.pods_total;
        let pods_unknown = self.n_pods.pods_unknown;

        match spec {
            Some(spec) => {
                if spec.unschedulable.is_some() {
                    unschedulable = Some(*spec.unschedulable.as_ref().unwrap());
                }
            }
            None => {}
        }

        match status {
            Some(status) => {
                if status.node_info.is_some() {
                    container_runtime_version
                        .clone_from(&status.node_info.as_ref().unwrap().container_runtime_version);

                    kernel_version.clone_from(&status.node_info.as_ref().unwrap().kernel_version);
                    kubelet_version.clone_from(&status.node_info.as_ref().unwrap().kubelet_version);
                    os_image.clone_from(&status.node_info.as_ref().unwrap().os_image);
                }

                if status.allocatable.is_some() {
                    let allocatable = status.allocatable.as_ref().unwrap();
                    let cpu_quantity = allocatable.get("cpu");
                    let memory_quantity = allocatable.get("memory");
                    let pods_quantity = allocatable.get("pods");

                    cpu_allocatable = cpu_quantity
                        .map(|cpu| convert_cpu_usage_to_milli(cpu.0.as_str()))
                        .unwrap_or(None);

                    memory_allocatable = memory_quantity
                        .map(|memory| convert_memory_usage_to_bytes(memory.0.as_str()))
                        .unwrap_or(None);

                    pods_allocatable =
                        pods_quantity.and_then(|pods_quantity| pods_quantity.0.parse().ok());
                }

                if status.capacity.is_some() {
                    let capacity = status.capacity.as_ref().unwrap();
                    let cpu_quantity = capacity.get("cpu");
                    let memory_quantity = capacity.get("memory");
                    let pods_quantity = capacity.get("pods");

                    cpu_capacity = cpu_quantity
                        .map(|cpu| convert_cpu_usage_to_milli(cpu.0.as_str()))
                        .unwrap_or(None);

                    memory_capacity = memory_quantity
                        .map(|memory| convert_memory_usage_to_bytes(memory.0.as_str()))
                        .unwrap_or(None);

                    pods_capacity =
                        pods_quantity.and_then(|pods_quantity| pods_quantity.0.parse().ok());
                }

                if status.addresses.is_some() {
                    let addresses = status.addresses.as_ref().unwrap();

                    for address in addresses {
                        if address.type_.to_lowercase() == "internalip" {
                            ip.clone_from(&address.address);
                        } else if address.type_.to_lowercase() == "externalip" {
                            ip_external.clone_from(&address.address);
                        }
                    }
                }

                if status.conditions.is_some() {
                    let conditions = status.conditions.as_ref().unwrap();

                    for condition in conditions {
                        if condition.type_.to_lowercase() == "ready" {
                            if condition.last_heartbeat_time.is_some() {
                                let heartbeat = condition.last_heartbeat_time.clone().unwrap();

                                ready_heartbeat_age = Utc::now()
                                    .signed_duration_since(heartbeat.0)
                                    .num_milliseconds();

                                ready_heartbeat_time = heartbeat.0.timestamp_millis();

                                ready_message.clone_from(
                                    condition.message.as_ref().unwrap_or(&"".to_string()),
                                );
                                ready_status.clone_from(&condition.status);
                            }

                            if condition.last_transition_time.is_some() {
                                let transition = condition.last_transition_time.clone().unwrap();

                                ready_transition_age = Utc::now()
                                    .signed_duration_since(transition.0)
                                    .num_milliseconds();

                                ready_transition_time = transition.0.timestamp_millis();
                            }

                            ready = Some(condition.status.to_lowercase() == "true");
                        }
                    }
                }
            }
            None => {}
        }

        if self.n.metadata.creation_timestamp.is_some() {
            let node_created = self.n.metadata.creation_timestamp.clone().unwrap();
            age = Utc::now()
                .signed_duration_since(node_created.0)
                .num_milliseconds();

            created = node_created.0.timestamp_millis();
        }

        if self.n.metadata.name.is_some() {
            node.clone_from(self.n.metadata.name.as_ref().unwrap());
        }

        NodeStats {
            age,
            container_runtime_version,
            containers_init,
            containers_ready,
            containers_running,
            containers_terminated,
            containers_total,
            containers_waiting,
            cpu_allocatable,
            cpu_capacity,
            cpu_usage,
            created,
            ip_external,
            ip,
            kernel_version,
            kubelet_version,
            memory_allocatable,
            memory_capacity,
            memory_usage,
            node,
            os_image,
            pods_failed,
            pods_pending,
            pods_running,
            pods_succeeded,
            pods_total,
            pods_unknown,
            ready_heartbeat_age,
            ready_heartbeat_time,
            ready_message,
            ready_status,
            ready_transition_age,
            ready_transition_time,
            ready,
            unschedulable,
            pods_allocatable,
            pods_capacity,
            resource: "node".to_string(),
            r#type: "metric".to_string(),
        }
    }
}

#[derive(Debug)]
pub struct NodeContainerStats {
    pub containers_waiting: u32,
    pub containers_total: u32,
    pub containers_terminated: u32,
    pub containers_running: u32,
    pub containers_ready: u32,
    pub containers_init: u32,
}

impl Default for NodeContainerStats {
    fn default() -> Self {
        Self::new()
    }
}

impl NodeContainerStats {
    pub fn new() -> Self {
        NodeContainerStats {
            containers_waiting: 0,
            containers_total: 0,
            containers_terminated: 0,
            containers_running: 0,
            containers_ready: 0,
            containers_init: 0,
        }
    }

    pub fn inc(&mut self, state: &str, ready: bool, init: bool) {
        if init {
            self.containers_init += 1;
        }

        match state.to_lowercase().as_str() {
            "waiting" => {
                self.containers_waiting += 1;
                self.containers_total += 1;
            }
            "terminated" => {
                self.containers_terminated += 1;
                self.containers_total += 1;
            }
            "running" => {
                self.containers_running += 1;
                self.containers_total += 1;

                if ready {
                    self.containers_ready += 1;
                }
            }
            _ => {}
        }
    }
}

#[derive(Debug)]
pub struct NodePodStats {
    pub pods_failed: u32,
    pub pods_pending: u32,
    pub pods_running: u32,
    pub pods_succeeded: u32,
    pub pods_unknown: u32,
    pub pods_total: u32,
}

impl Default for NodePodStats {
    fn default() -> Self {
        Self::new()
    }
}

impl NodePodStats {
    pub fn new() -> Self {
        NodePodStats {
            pods_failed: 0,
            pods_pending: 0,
            pods_running: 0,
            pods_succeeded: 0,
            pods_unknown: 0,
            pods_total: 0,
        }
    }

    pub fn inc(&mut self, phase: &str) {
        self.pods_total += 1;

        match phase.to_lowercase().as_str() {
            "failed" => {
                self.pods_failed += 1;
            }
            "pending" => {
                self.pods_pending += 1;
            }
            "running" => {
                self.pods_running += 1;
            }
            "succeeded" => {
                self.pods_succeeded += 1;
            }
            "unknown" => {
                self.pods_unknown += 1;
            }
            _ => {
                self.pods_unknown += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use chrono::Utc;
    use k8s_openapi::{
        api::core::v1::{Node, NodeAddress, NodeCondition, NodeSpec, NodeStatus, NodeSystemInfo},
        apimachinery::pkg::{api::resource::Quantity, apis::meta::v1::Time},
    };
    use kube::api::ObjectMeta;

    use super::{NodeContainerStats, NodePodStats, NodeStatsBuilder};

    #[tokio::test]
    async fn test_build_node_stats() {
        let allocatable = create_allocatable_default();
        let capacity = create_capacity_default();

        let node_pod_stats = NodePodStats::new();
        let node_container_stats = NodeContainerStats::new();

        let status = create_status(Some(capacity), Some(allocatable), true, true, true);
        let node = create_node(status);
        let builder =
            NodeStatsBuilder::new(&node, &node_pod_stats, &node_container_stats, "1", "1");

        let result = builder.build();

        assert_eq!(result.node, "name".to_string());
        assert_eq!(result.container_runtime_version, "version".to_string());
        assert_eq!(result.containers_init, 0);
        assert_eq!(result.containers_ready, 0);
        assert_eq!(result.containers_running, 0);
        assert_eq!(result.containers_terminated, 0);
        assert_eq!(result.containers_total, 0);
        assert_eq!(result.containers_waiting, 0);
        assert_eq!(result.cpu_allocatable.unwrap(), 123000);
        assert_eq!(result.cpu_capacity.unwrap(), 123000);
        assert_eq!(result.cpu_usage.unwrap(), 1000);
        assert_eq!(result.memory_usage.unwrap(), 1);
        assert_eq!(result.ip, "a2".to_string());
        assert_eq!(result.ip_external, "a1".to_string());
        assert_eq!(result.kernel_version, "kernel".to_string());
        assert_eq!(result.kubelet_version, "kubelet".to_string());
        assert_eq!(result.memory_allocatable.unwrap(), 123);
        assert_eq!(result.memory_capacity.unwrap(), 123);
        assert_eq!(result.os_image, "os_image".to_string());
        assert_eq!(result.pods_allocatable.unwrap(), 123);
        assert_eq!(result.pods_capacity.unwrap(), 123);
        assert_eq!(result.pods_failed, 0);
        assert_eq!(result.pods_pending, 0);
        assert_eq!(result.pods_running, 0);
        assert_eq!(result.pods_succeeded, 0);
        assert_eq!(result.pods_total, 0);
        assert_eq!(result.pods_unknown, 0);
        assert_eq!(result.ready, Some(true));
        assert_eq!(result.ready_status, "true".to_string());
        assert_eq!(result.ready_message, "message".to_string());
    }

    #[tokio::test]
    async fn test_no_addresses() {
        let allocatable = create_allocatable_default();
        let capacity = create_capacity_default();

        let node_pod_stats = NodePodStats::new();
        let node_container_stats = NodeContainerStats::new();

        let status = create_status(Some(capacity), Some(allocatable), false, true, true);
        let node = create_node(status);
        let builder =
            NodeStatsBuilder::new(&node, &node_pod_stats, &node_container_stats, "1", "1");

        let result = builder.build();

        assert_eq!(result.node, "name".to_string());
        assert_eq!(result.ip, "".to_string());
        assert_eq!(result.ip_external, "".to_string());
    }

    #[tokio::test]
    async fn test_no_conditions() {
        let allocatable = create_allocatable_default();
        let capacity = create_capacity_default();

        let node_pod_stats = NodePodStats::new();
        let node_container_stats = NodeContainerStats::new();

        let status = create_status(Some(capacity), Some(allocatable), true, false, true);
        let node = create_node(status);
        let builder =
            NodeStatsBuilder::new(&node, &node_pod_stats, &node_container_stats, "1", "1");

        let result = builder.build();

        assert_eq!(result.node, "name".to_string());
        assert_eq!(result.ready_status, "".to_string());
    }

    #[tokio::test]
    async fn test_no_node_info() {
        let allocatable = create_allocatable_default();
        let capacity = create_capacity_default();

        let node_pod_stats = NodePodStats::new();
        let node_container_stats = NodeContainerStats::new();

        let status = create_status(Some(capacity), Some(allocatable), true, true, false);
        let node = create_node(status);
        let builder =
            NodeStatsBuilder::new(&node, &node_pod_stats, &node_container_stats, "1", "1");

        let result = builder.build();

        assert_eq!(result.node, "name".to_string());
        assert_eq!(result.os_image, "".to_string());
    }

    #[tokio::test]
    async fn test_bad_capacity() {
        let allocatable = create_allocatable_default();
        let capacity = create_capacity_bad();

        let node_pod_stats = NodePodStats::new();
        let node_container_stats = NodeContainerStats::new();

        let status = create_status(Some(capacity), Some(allocatable), true, true, false);
        let node = create_node(status);
        let builder =
            NodeStatsBuilder::new(&node, &node_pod_stats, &node_container_stats, "1", "1");

        let result = builder.build();

        assert_eq!(result.cpu_capacity, None);
    }

    #[tokio::test]
    async fn test_bad_allocatable() {
        let allocatable = create_allocatable_bad();
        let capacity = create_capacity_default();

        let node_pod_stats = NodePodStats::new();
        let node_container_stats = NodeContainerStats::new();

        let status = create_status(Some(capacity), Some(allocatable), true, true, false);
        let node = create_node(status);
        let builder =
            NodeStatsBuilder::new(&node, &node_pod_stats, &node_container_stats, "1", "1");

        let result = builder.build();

        assert_eq!(result.cpu_allocatable, None);
    }

    fn create_node(status: Option<NodeStatus>) -> Node {
        let spec = create_spec();

        let meta = ObjectMeta {
            annotations: None,
            cluster_name: None,
            creation_timestamp: Some(Time(Utc::now())),
            deletion_grace_period_seconds: None,
            deletion_timestamp: None,
            finalizers: None,
            generate_name: None,
            generation: None,
            labels: None,
            managed_fields: None,
            name: Some("name".to_string()),
            namespace: Some("namespace".to_string()),
            owner_references: None,
            resource_version: None,
            self_link: None,
            uid: None,
        };

        Node {
            metadata: meta,
            spec,
            status,
        }
    }

    fn create_spec() -> Option<NodeSpec> {
        Some(NodeSpec {
            config_source: None,
            external_id: None,
            pod_cidr: None,
            pod_cidrs: None,
            provider_id: None,
            taints: None,
            unschedulable: Some(true),
        })
    }

    fn create_status(
        capacity: Option<BTreeMap<String, Quantity>>,
        allocatable: Option<BTreeMap<String, Quantity>>,
        populate_addresses: bool,
        populate_conditions: bool,
        populate_node_info: bool,
    ) -> Option<NodeStatus> {
        let mut address = None;
        let mut conditions = None;
        let mut node_info = None;

        if populate_addresses {
            let address_vec = vec![
                NodeAddress {
                    address: "a1".to_string(),
                    type_: "externalip".to_string(),
                },
                NodeAddress {
                    address: "a2".to_string(),
                    type_: "internalip".to_string(),
                },
            ];
            address = Some(address_vec);
        }

        if populate_conditions {
            let conditions_vec = vec![NodeCondition {
                last_heartbeat_time: Some(Time(Utc::now())),
                last_transition_time: Some(Time(Utc::now())),
                message: Some("message".to_string()),
                reason: Some("reason".to_string()),
                status: "true".to_string(),
                type_: "ready".to_string(),
            }];

            conditions = Some(conditions_vec)
        }

        if populate_node_info {
            node_info = Some(NodeSystemInfo {
                architecture: "arch".to_string(),
                boot_id: "boot".to_string(),
                container_runtime_version: "version".to_string(),
                kernel_version: "kernel".to_string(),
                kube_proxy_version: "proxy".to_string(),
                kubelet_version: "kubelet".to_string(),
                machine_id: "id".to_string(),
                operating_system: "opsys".to_string(),
                os_image: "os_image".to_string(),
                system_uuid: "sysid".to_string(),
            });
        }

        Some(NodeStatus {
            addresses: address,
            allocatable,
            capacity,
            conditions,
            config: None,
            daemon_endpoints: None,
            images: None,
            node_info,
            phase: Some("phase".to_string()),
            volumes_attached: None,
            volumes_in_use: None,
        })
    }

    fn create_allocatable_default() -> BTreeMap<String, Quantity> {
        let mut allocatable: BTreeMap<String, Quantity> = BTreeMap::new();
        allocatable.insert("cpu".to_string(), Quantity("123".to_string()));
        allocatable.insert("memory".to_string(), Quantity("123".to_string()));
        allocatable.insert("pods".to_string(), Quantity("123".to_string()));

        allocatable
    }

    fn create_allocatable_bad() -> BTreeMap<String, Quantity> {
        let mut allocatable: BTreeMap<String, Quantity> = BTreeMap::new();
        allocatable.insert("cpu".to_string(), Quantity("ab".to_string()));
        allocatable.insert("memory".to_string(), Quantity("ab".to_string()));
        allocatable.insert("pods".to_string(), Quantity("ab".to_string()));

        allocatable
    }

    fn create_capacity_default() -> BTreeMap<String, Quantity> {
        let mut capacity: BTreeMap<String, Quantity> = BTreeMap::new();
        capacity.insert("cpu".to_string(), Quantity("123".to_string()));
        capacity.insert("memory".to_string(), Quantity("123".to_string()));
        capacity.insert("pods".to_string(), Quantity("123".to_string()));

        capacity
    }

    fn create_capacity_bad() -> BTreeMap<String, Quantity> {
        let mut capacity: BTreeMap<String, Quantity> = BTreeMap::new();
        capacity.insert("cpu".to_string(), Quantity("ab".to_string()));
        capacity.insert("memory".to_string(), Quantity("ab".to_string()));

        capacity.insert("pods".to_string(), Quantity("ab".to_string()));

        capacity
    }
}

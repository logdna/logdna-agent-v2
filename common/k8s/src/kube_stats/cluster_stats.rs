use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct ClusterStats {
    pub resource: String,
    pub r#type: String,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(with = "::serde_with::rust::unwrap_or_skip")]
    pub memory_allocatable: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(with = "::serde_with::rust::unwrap_or_skip")]
    pub memory_capacity: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(with = "::serde_with::rust::unwrap_or_skip")]
    pub memory_usage: Option<u64>,
    pub nodes_notready: u32,
    pub nodes_ready: u32,
    pub nodes_total: u32,
    pub nodes_unschedulable: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(with = "::serde_with::rust::unwrap_or_skip")]
    pub pods_allocatable: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(with = "::serde_with::rust::unwrap_or_skip")]
    pub pods_capacity: Option<u64>,
    pub pods_failed: u32,
    pub pods_pending: u32,
    pub pods_running: u32,
    pub pods_succeeded: u32,
    pub pods_unknown: u32,
    pub pods_total: u32,
}

impl Default for ClusterStats {
    fn default() -> Self {
        Self::new()
    }
}

impl ClusterStats {
    pub fn new() -> ClusterStats {
        ClusterStats {
            containers_init: 0,
            containers_ready: 0,
            containers_running: 0,
            containers_terminated: 0,
            containers_total: 0,
            containers_waiting: 0,
            cpu_allocatable: None,
            cpu_capacity: None,
            cpu_usage: None,
            memory_allocatable: None,
            memory_capacity: None,
            memory_usage: None,
            nodes_notready: 0,
            nodes_ready: 0,
            nodes_total: 0,
            nodes_unschedulable: 0,
            pods_allocatable: None,
            pods_capacity: None,
            pods_failed: 0,
            pods_pending: 0,
            pods_running: 0,
            pods_succeeded: 0,
            pods_unknown: 0,
            pods_total: 0,
            resource: "cluster".to_string(),
            r#type: "metric".to_string(),
        }
    }
}

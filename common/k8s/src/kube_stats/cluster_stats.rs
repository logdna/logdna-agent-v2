use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct ClusterStats {
    pub resource: String,
    pub r#type: String,
    pub containers_init: i32,
    pub containers_ready: i32,
    pub containers_running: i32,
    pub containers_terminated: i32,
    pub containers_total: i32,
    pub containers_waiting: i32,
    pub cpu_allocatable: i64,
    pub cpu_capacity: i64,
    pub cpu_usage: i32,
    pub memory_allocatable: i64,
    pub memory_capacity: i64,
    pub memory_usage: i64,
    pub nodes_notready: i32,
    pub nodes_ready: i32,
    pub nodes_total: i32,
    pub nodes_unschedulable: i64,
    pub pods_allocatable: i64,
    pub pods_capacity: i64,
    pub pods_failed: i32,
    pub pods_pending: i32,
    pub pods_running: i32,
    pub pods_succeeded: i32,
    pub pods_unknown: i32,
    pub pods_total: i32,
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
            cpu_allocatable: 0,
            cpu_capacity: 0,
            cpu_usage: 0,
            memory_allocatable: 0,
            memory_capacity: 0,
            memory_usage: 0,
            nodes_notready: 0,
            nodes_ready: 0,
            nodes_total: 0,
            nodes_unschedulable: 0,
            pods_allocatable: 0,
            pods_capacity: 0,
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

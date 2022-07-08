use chrono::Utc;
use k8s_openapi::api::core::v1::{Container, ContainerState, ContainerStatus};
use serde::{Deserialize, Serialize};

use super::helpers::{convert_cpu_usage_to_milli, convert_memory_usage_to_bytes};

#[derive(Serialize, Deserialize)]
pub struct ContainerStats {
    pub container_age: i64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub container: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(with = "::serde_with::rust::unwrap_or_skip")]
    pub cpu_limit: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(with = "::serde_with::rust::unwrap_or_skip")]
    pub cpu_request: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(with = "::serde_with::rust::unwrap_or_skip")]
    pub cpu_usage: Option<u32>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub image_tag: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub image: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(with = "::serde_with::rust::unwrap_or_skip")]
    pub last_finished: Option<i64>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub last_reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(with = "::serde_with::rust::unwrap_or_skip")]
    pub last_started: Option<i64>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub last_state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(with = "::serde_with::rust::unwrap_or_skip")]
    pub memory_limit: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(with = "::serde_with::rust::unwrap_or_skip")]
    pub memory_request: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(with = "::serde_with::rust::unwrap_or_skip")]
    pub memory_usage: Option<u64>,
    pub ready: bool,
    pub restarts: i32,
    pub started: i64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub state: String,
}

impl ContainerStats {
    pub fn builder<'a>(
        c: &'a Container,
        c_status: &'a ContainerStatus,
        c_state: &'a ContainerState,
        raw_cpu_usage: &'a str,
        raw_memory_usage: &'a str,
    ) -> ContainerStatsBuilder<'a> {
        ContainerStatsBuilder {
            c,
            c_status,
            c_state,
            raw_cpu_usage,
            raw_memory_usage,
        }
    }
}

pub struct ContainerStatsBuilder<'a> {
    c: &'a Container,
    c_status: &'a ContainerStatus,
    c_state: &'a ContainerState,
    raw_cpu_usage: &'a str,
    raw_memory_usage: &'a str,
}

impl ContainerStatsBuilder<'_> {
    pub fn new<'a>(
        c: &'a Container,
        c_status: &'a ContainerStatus,
        c_state: &'a ContainerState,
        raw_cpu_usage: &'a str,
        raw_memory_usage: &'a str,
    ) -> ContainerStatsBuilder<'a> {
        ContainerStatsBuilder {
            c,
            c_status,
            c_state,
            raw_cpu_usage,
            raw_memory_usage,
        }
    }

    pub fn build(self) -> ContainerStats {
        let container = self.c.name.clone();

        let memory_usage = convert_memory_usage_to_bytes(self.raw_memory_usage);
        let cpu_usage = convert_cpu_usage_to_milli(self.raw_cpu_usage);

        let mut image = String::new();
        let mut image_tag = String::new();
        let state;
        let mut last_state = String::new();
        let mut last_reason = String::new();

        let mut cpu_limit = None;
        let mut cpu_request = None;
        let mut memory_limit = None;
        let mut memory_request = None;
        let mut last_started = None;
        let mut last_finished = None;

        let mut container_age: i64 = 0;
        let mut started: i64 = 0;

        let restarts = self.c_status.restart_count;
        let ready = self.c_status.ready;

        if self.c.image.is_some() {
            let container_image = self.c.image.clone().unwrap();

            if let Some(split) = container_image.split_once(':') {
                image = split.0.to_string();
                image_tag = split.1.to_string();
            } else {
                image = container_image;
            }
        }

        let terminated = self.c_state.terminated.as_ref();

        if let Some(running) = self.c_state.running.as_ref() {
            state = "Running".to_string();

            if let Some(started_at) = running.started_at.as_ref().map(|s| s.0) {
                container_age = Utc::now()
                    .signed_duration_since(started_at)
                    .num_milliseconds();

                started = started_at.timestamp_millis();
            }
        } else if terminated.is_some() {
            state = "Terminated".to_string()
        } else {
            state = "Waiting".to_string()
        }

        if let Some(last_status_state) = self.c_status.last_state.as_ref() {
            let last_running = last_status_state.running.as_ref();
            let last_terminated = last_status_state.terminated.as_ref();
            let last_waiting = last_status_state.waiting.as_ref();

            if last_waiting.is_some() {
                last_state = String::from("Waiting");
            }

            last_running.and_then(|l| {
                last_state = "Running".to_string();
                l.started_at.as_ref().map(|s| {
                    last_started = Some(s.0.timestamp_millis());
                })
            });

            last_terminated.and_then(|l| {
                last_state = "Terminated".to_string();
                if let Some(s) = l.started_at.as_ref() {
                    last_started = Some(s.0.timestamp_millis());
                }
                if let Some(f) = l.finished_at.as_ref() {
                    last_finished = Some(f.0.timestamp_millis());
                }
                l.reason.as_ref().map(|r| last_reason = r.to_string())
            });
        }

        if last_state.eq(&state) || last_state.eq("") {
            last_state = String::from("");
            last_reason = String::from("");
            last_finished = None;
            last_started = None;
        }
        if let Some(resources) = self.c.resources.as_ref() {
            if let Some(limits) = resources.limits.as_ref() {
                let cpu = limits.get("cpu");
                let memory = limits.get("memory");

                cpu_limit = cpu
                    .map(|cpu| convert_cpu_usage_to_milli(cpu.0.as_str()))
                    .unwrap_or(None);

                memory_limit = memory
                    .map(|memory| convert_memory_usage_to_bytes(memory.0.as_str()))
                    .unwrap_or(None);
            }

            if let Some(requests) = resources.requests.as_ref() {
                let cpu = requests.get("cpu");
                let memory = requests.get("memory");

                cpu_request = cpu
                    .map(|cpu| convert_cpu_usage_to_milli(cpu.0.as_str()))
                    .unwrap_or(None);

                memory_request = memory
                    .map(|memory| convert_memory_usage_to_bytes(memory.0.as_str()))
                    .unwrap_or(None);
            }
        }

        ContainerStats {
            container_age,
            container,
            cpu_limit,
            cpu_request,
            cpu_usage,
            image_tag,
            image,
            last_finished,
            last_reason,
            last_started,
            last_state,
            memory_limit,
            memory_request,
            memory_usage,
            ready,
            restarts,
            started,
            state,
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use k8s_openapi::{
        api::core::v1::{
            ContainerStateRunning, ContainerStateTerminated, ContainerStateWaiting,
            ResourceRequirements,
        },
        apimachinery::pkg::{api::resource::Quantity, apis::meta::v1::Time},
    };
    use std::collections::BTreeMap;

    use super::*;

    #[tokio::test]
    async fn test_create_running_container_stats() {
        let resource = create_resource_default();
        let state = create_state("running".to_string());
        let status = create_status(None);
        let container = create_container(resource);
        let container_builder = ContainerStatsBuilder::new(&container, &status, &state, "1", "1");

        let result = container_builder.build();

        assert_eq!(result.image, "test-image".to_string());
        assert_eq!(result.image_tag, "1234:1234".to_string());
        assert_eq!(result.memory_usage.unwrap(), 1);
        assert_eq!(result.cpu_usage.unwrap(), 1000);
        assert_eq!(result.state, "Running".to_string());
        assert_eq!(result.cpu_limit.unwrap(), 123000);
        assert_eq!(result.cpu_request.unwrap(), 123000);
        assert_eq!(result.memory_limit.unwrap(), 123);
        assert_eq!(result.memory_request.unwrap(), 123);
        assert_eq!(result.restarts, 0);
        assert!(result.ready);
        assert_eq!(result.last_finished, None);
        assert_eq!(result.last_started, None);
        assert_eq!(result.last_reason, String::from(""));
        assert_eq!(result.last_state, String::from(""));
        assert_eq!(result.state, "Running".to_string());
    }

    #[tokio::test]
    async fn test_create_running_prev_waiting_container_stats() {
        let resource = create_resource_default();
        let state = create_state("running".to_string());
        let prev_state = create_state("waiting".to_string());
        let status = create_status(Some(prev_state));
        let container = create_container(resource);
        let container_builder = ContainerStatsBuilder::new(&container, &status, &state, "1", "1");

        let result = container_builder.build();

        assert_eq!(result.state, "Running".to_string());
        assert_eq!(result.last_state, "Waiting".to_string());
        assert_eq!(result.last_finished, None);
    }

    #[tokio::test]
    async fn test_create_running_prev_terminated_container_stats() {
        let resource = create_resource_default();
        let state = create_state("running".to_string());
        let prev_state = create_state("terminated".to_string());
        let status = create_status(Some(prev_state));
        let container = create_container(resource);
        let container_builder = ContainerStatsBuilder::new(&container, &status, &state, "1", "1");

        let result = container_builder.build();

        assert_eq!(result.state, "Running".to_string());
        assert_eq!(result.last_state, "Terminated".to_string());
        assert!(result.last_finished.is_some());
        assert!(result.last_started.is_some());
    }

    #[tokio::test]
    async fn test_bad_limits_bad_requests_container_stats() {
        let resource = create_resource_bad();
        let state = create_state("running".to_string());
        let prev_state = create_state("terminated".to_string());
        let status = create_status(Some(prev_state));
        let container = create_container(resource);
        let container_builder = ContainerStatsBuilder::new(&container, &status, &state, "1", "1");

        let result = container_builder.build();

        assert_eq!(result.cpu_limit, None);
        assert_eq!(result.cpu_request, None);
        assert_eq!(result.memory_limit, None);
        assert_eq!(result.memory_request, None);
    }

    fn create_state(state: String) -> ContainerState {
        let mut running_state = None;
        let mut terminated_state = None;
        let mut waiting_state = None;

        if state.eq(&"running".to_string()) {
            running_state = Some(ContainerStateRunning {
                started_at: Some(Time(Utc::now())),
            })
        } else if state.eq(&"terminated".to_string()) {
            terminated_state = Some(ContainerStateTerminated {
                container_id: None,
                exit_code: 0,
                finished_at: Some(Time(Utc::now())),
                message: Some("message".to_string()),
                reason: Some("reason".to_string()),
                signal: None,
                started_at: Some(Time(Utc::now())),
            })
        } else if state.eq(&"waiting".to_string()) {
            waiting_state = Some(ContainerStateWaiting {
                message: Some("reason".to_string()),
                reason: None,
            })
        }

        ContainerState {
            running: running_state,
            terminated: terminated_state,
            waiting: waiting_state,
        }
    }

    fn create_status(prev_state: Option<ContainerState>) -> ContainerStatus {
        ContainerStatus {
            container_id: Some("container".to_string()),
            image: "image".to_string(),
            image_id: "image".to_string(),
            last_state: prev_state,
            name: "container_name".to_string(),
            ready: true,
            restart_count: 0,
            started: None,
            state: None,
        }
    }

    fn create_container(resource: ResourceRequirements) -> Container {
        Container {
            args: None,
            command: None,
            env: None,
            env_from: None,
            image: Some("test-image:1234:1234".to_string()),
            image_pull_policy: Some("test-sometimes:1234:1234".to_string()),
            lifecycle: None,
            liveness_probe: None,
            name: "container-name".to_string(),
            ports: None,
            readiness_probe: None,
            resources: Some(resource),
            security_context: None,
            startup_probe: None,
            stdin: None,
            stdin_once: None,
            termination_message_path: None,
            termination_message_policy: None,
            tty: None,
            volume_devices: None,
            volume_mounts: None,
            working_dir: None,
        }
    }

    fn create_resource_default() -> ResourceRequirements {
        let mut b_tree_limits: BTreeMap<String, Quantity> = BTreeMap::new();
        b_tree_limits.insert("cpu".to_string(), Quantity("123".to_string()));
        b_tree_limits.insert("memory".to_string(), Quantity("123".to_string()));

        let mut b_tree_requests = BTreeMap::new();
        b_tree_requests.insert("cpu".to_string(), Quantity("123".to_string()));

        b_tree_requests.insert("memory".to_string(), Quantity("123".to_string()));

        ResourceRequirements {
            limits: Some(b_tree_limits),
            requests: Some(b_tree_requests),
        }
    }

    fn create_resource_bad() -> ResourceRequirements {
        let mut b_tree_limits: BTreeMap<String, Quantity> = BTreeMap::new();
        b_tree_limits.insert("cpu".to_string(), Quantity("not a limit".to_string()));
        b_tree_limits.insert("memory".to_string(), Quantity("not a limit".to_string()));

        let mut b_tree_requests = BTreeMap::new();
        b_tree_requests.insert("cpu".to_string(), Quantity("not a limit".to_string()));
        b_tree_requests.insert("memory".to_string(), Quantity("not a limit".to_string()));

        ResourceRequirements {
            limits: Some(b_tree_limits),
            requests: Some(b_tree_requests),
        }
    }
}

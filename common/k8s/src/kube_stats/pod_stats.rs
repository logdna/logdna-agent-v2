use chrono::Utc;
use k8s_openapi::{api::core::v1::Pod, apimachinery::pkg::apis::meta::v1::OwnerReference};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone)]
pub struct PodStats {
    pub resource: String,
    pub r#type: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub controller: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub controller_type: String,
    pub created: i64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub ip: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub namespace: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub node: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub phase: String,
    pub pod_age: i64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub pod: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub priority_class: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(with = "::serde_with::rust::unwrap_or_skip")]
    pub priority: Option<i32>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub qos_class: String,
}

impl PodStats {
    pub fn builder(p: &Pod) -> PodStatsBuilder {
        PodStatsBuilder { p }
    }
}

pub struct PodStatsBuilder<'a> {
    p: &'a Pod,
}

impl PodStatsBuilder<'_> {
    pub fn new(p: &Pod) -> PodStatsBuilder {
        PodStatsBuilder { p }
    }

    pub fn build(self) -> PodStats {
        let details = get_controller_details(&self.p.metadata.owner_references);
        let controller = details.0;
        let controller_type = details.1;

        let spec = &self.p.spec;
        let status = &self.p.status;

        let mut priority_class = String::new();
        let mut node = String::new();
        let mut ip = String::new();
        let mut phase = String::new();
        let mut qos_class = String::new();

        let mut pod_age = 0;
        let mut created: i64 = 0;

        let mut priority = None;

        let namespace = self.p.metadata.namespace.clone().unwrap_or_default();

        let pod = self.p.metadata.name.clone().unwrap_or_default();

        if let Some(spec) = spec {
            if spec.priority.is_some() {
                priority = Some(spec.priority.unwrap());
            }

            if spec.priority_class_name.is_some() {
                priority_class = spec.priority_class_name.clone().unwrap();
            }

            if spec.node_name.is_some() {
                node = spec.node_name.clone().unwrap();
            }
        }

        if let Some(status) = status {
            if status.start_time.is_some() {
                let pod_created = status.start_time.clone().unwrap();

                pod_age = Utc::now()
                    .signed_duration_since(pod_created.0)
                    .num_milliseconds();

                created = pod_created.0.timestamp_millis();
            }

            if status.pod_ip.is_some() {
                ip = status.pod_ip.clone().unwrap();
            }

            if status.phase.is_some() {
                phase = status.phase.clone().unwrap();
            }

            if status.qos_class.is_some() {
                qos_class = status.qos_class.clone().unwrap();
            }
        }

        PodStats {
            controller,
            controller_type,
            created,
            ip,
            namespace,
            node,
            phase,
            pod_age,
            pod,
            priority_class,
            priority,
            qos_class,
            resource: "container".to_string(),
            r#type: "metric".to_string(),
        }
    }
}

fn get_controller_details(owners: &Option<Vec<OwnerReference>>) -> (String, String) {
    if owners.is_some() {
        for owner in owners.as_ref().unwrap() {
            if owner.controller == Some(true) {
                return (owner.name.clone(), owner.kind.clone());
            }
        }
    }

    ("".to_string(), "".to_string())
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use k8s_openapi::{
        api::core::v1::{Pod, PodSpec, PodStatus},
        apimachinery::pkg::apis::meta::v1::Time,
    };
    use kube::api::ObjectMeta;

    use super::PodStatsBuilder;

    #[tokio::test]
    async fn test_create_pod() {
        let spec = create_spec();
        let status = create_status();
        let pod = create_pod(Some(spec), Some(status));

        let pod_builder = PodStatsBuilder::new(&pod);
        let result = pod_builder.build();

        assert_eq!(result.ip, "ip".to_string());
        assert_eq!(result.phase, "phase".to_string());
        assert_eq!(result.priority_class, "p_class".to_string());
        assert_eq!(result.node, "node_name".to_string());
        assert_eq!(result.qos_class, "class".to_string());
        assert_eq!(result.namespace, "namespace".to_string());
        assert_eq!(result.pod, "name".to_string());
        assert_eq!(result.priority.unwrap(), 222);
    }

    #[tokio::test]
    async fn test_create_no_spec() {
        let status = create_status();
        let pod = create_pod(None, Some(status));

        let pod_builder = PodStatsBuilder::new(&pod);
        let result = pod_builder.build();

        assert_eq!(result.node, "".to_string());
    }

    #[tokio::test]
    async fn test_create_no_status() {
        let spec = create_spec();
        let pod = create_pod(Some(spec), None);

        let pod_builder = PodStatsBuilder::new(&pod);
        let result = pod_builder.build();

        assert_eq!(result.phase, "".to_string());
    }

    fn create_pod(spec: Option<PodSpec>, status: Option<PodStatus>) -> Pod {
        let meta = ObjectMeta {
            annotations: None,
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

        Pod {
            metadata: meta,
            spec,
            status,
        }
    }

    fn create_spec() -> PodSpec {
        PodSpec {
            active_deadline_seconds: None,
            affinity: None,
            automount_service_account_token: None,
            containers: Vec::new(),
            dns_config: None,
            dns_policy: None,
            enable_service_links: None,
            ephemeral_containers: None,
            host_aliases: None,
            host_ipc: None,
            host_network: None,
            host_pid: None,
            host_users: None,
            hostname: None,
            image_pull_secrets: None,
            init_containers: None,
            node_name: Some("node_name".to_string()),
            node_selector: None,
            overhead: None,
            os: None,
            preemption_policy: None,
            priority: Some(222),
            priority_class_name: Some("p_class".to_string()),
            readiness_gates: None,
            resource_claims: None,
            restart_policy: None,
            runtime_class_name: None,
            scheduler_name: None,
            scheduling_gates: None,
            security_context: None,
            service_account: None,
            service_account_name: None,
            share_process_namespace: None,
            subdomain: None,
            termination_grace_period_seconds: None,
            tolerations: None,
            topology_spread_constraints: None,
            volumes: None,
            set_hostname_as_fqdn: None,
        }
    }

    fn create_status() -> PodStatus {
        PodStatus {
            conditions: None,
            container_statuses: None,
            ephemeral_container_statuses: None,
            host_ip: None,
            host_ips: None,
            init_container_statuses: None,
            message: None,
            nominated_node_name: None,
            phase: Some("phase".to_string()),
            pod_ip: Some("ip".to_string()),
            pod_ips: None,
            qos_class: Some("class".to_string()),
            reason: None,
            resize: None,
            resource_claim_statuses: None,
            start_time: Some(Time(Utc::now())),
        }
    }
}

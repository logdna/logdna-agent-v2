use chrono::Utc;
use k8s_openapi::api::coordination::v1::{Lease, LeaseSpec};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::MicroTime;
use kube::api::{Api, ListParams, ObjectMeta, Patch, PatchParams, PostParams};
use kube::core::ObjectList;
use kube::{Client, Error};
use serde::{Deserialize, Serialize};
use tracing::{error, info};

pub const K8S_STARTUP_LEASE_LABEL: &str = "process=logdna-agent-startup";
pub const K8S_STARTUP_LEASE_RETRY_ATTEMPTS: i32 = 3;

#[derive(Debug, Serialize, Deserialize)]
struct LeasePatchSpec {
    spec: LeasePatchValue,
}

// Non snake case for JSON parsing reasons
#[allow(non_snake_case)]
#[derive(Debug, Serialize, Deserialize)]
struct LeasePatchValue {
    #[serde(rename = "holderIdentity")]
    holder_identity: Option<String>,
    #[serde(rename = "aquireTime")]
    acquire_time: MicroTime,
}

pub async fn get_available_lease(lease_label: &str, lease_client: &Api<Lease>) -> Option<String> {
    let lease_info = get_lease_list(lease_label, lease_client).await;
    for lease in lease_info.unwrap().into_iter() {
        let lease_name = lease.metadata.name.unwrap();
        match lease.spec.unwrap().holder_identity {
            Some(lease_owner) => {
                info!("Lease {} is OWNED by {:?}.", lease_name, lease_owner)
            }
            None => {
                info!("Lease {} NOT OWNED...", lease_name);
                return Some(lease_name);
            }
        }
    }
    None
}

pub async fn claim_lease(
    lease_name: String,
    pod_name: String,
    lease_client: &Api<Lease>,
    return_ref: &mut Option<String>,
) {
    let patch_value = LeasePatchValue {
        holder_identity: Some(pod_name),
        acquire_time: MicroTime(Utc::now()),
    };

    let patch_spec = LeasePatchSpec { spec: patch_value };
    let patch = Patch::Merge(patch_spec);
    let pp = PatchParams::apply(&lease_name);
    let patch_lease = lease_client.patch(&lease_name, &pp, &patch).await;
    match patch_lease {
        Ok(ref patch) => {
            info!(
                "Lease {} now owned by {:?}",
                &lease_name,
                &patch
                    .spec
                    .as_ref()
                    .unwrap()
                    .holder_identity
                    .as_ref()
                    .unwrap(),
            );
        }
        Err(e) => {
            error!("Issue patching lease: {:?}", e)
        }
    }
    *return_ref = Some(lease_name);
}

pub async fn renew_lease(
    pod_name: String,
    lease_client: &Api<Lease>,
    namespace: String,
    lease: Lease,
) -> bool {
    if let Some(spec) = lease.spec {
        let lease_name = lease.metadata.name.unwrap_or_default();
        let original_duration = spec.lease_duration_seconds.unwrap_or(0i32);

        let metadata = create_lease_metadata(lease_name.clone(), namespace.clone());
        let lease_spec = create_lease_spec_patch(pod_name, original_duration);

        let lease = Lease {
            metadata,
            spec: Some(lease_spec),
        };

        let patch = Patch::Merge(lease);
        let pp = PatchParams::apply(&lease_name);
        let patch_lease = lease_client.patch(&lease_name, &pp, &patch).await;
        match patch_lease {
            Ok(_) => true,
            Err(e) => {
                info!("Error renewing lease{}", e);
                false
            }
        }
    } else {
        false
    }
}

pub async fn release_lease(lease_name: &str, lease_client: &Api<Lease>) {
    let patch_value = LeasePatchValue {
        holder_identity: None,
        acquire_time: MicroTime(Utc::now()),
    };
    let patch_spec = LeasePatchSpec { spec: patch_value };
    let patch = Patch::Merge(patch_spec);
    let pp = PatchParams::apply(lease_name);
    let patch_lease = lease_client.patch(lease_name, &pp, &patch).await;
    match patch_lease {
        Ok(ref patch) => {
            info!(
                "Lease {} has now been released to {:?}",
                &lease_name,
                &patch.spec.as_ref().unwrap().holder_identity
            );
        }
        Err(e) => {
            error!("Issue releasing lease: {:?}", e)
        }
    }
}

pub async fn replace_lease(
    lease_name: String,
    lease: &Lease,
    lease_client: &Api<Lease>,
    new_holder: String,
) -> Result<Lease, Error> {
    let pp = PostParams::default();

    let mut new_lease = lease.clone();

    let mut duration = 0;

    if let Some(spec) = &lease.spec {
        duration = spec.lease_duration_seconds.unwrap_or(0i32);
    }

    new_lease.spec = Some(create_lease_spec(new_holder, duration));
    lease_client.replace(&lease_name, &pp, &new_lease).await
}

pub async fn get_k8s_lease_api(namespace: &str, client: Client) -> Api<Lease> {
    let lease_api: Api<Lease> = Api::namespaced(client, namespace);
    lease_api
}

pub async fn get_lease(name: &str, lease_client: &Api<Lease>) -> Result<Lease, kube::Error> {
    lease_client.get(name).await
}

async fn get_lease_list(
    lease_label: &str,
    lease_client: &Api<Lease>,
) -> Result<ObjectList<Lease>, kube::Error> {
    let lp = ListParams::default().labels(lease_label);
    lease_client.list(&lp).await
}

fn create_lease_metadata(lease_name: String, namespace: String) -> ObjectMeta {
    ObjectMeta {
        name: Some(lease_name),
        namespace: Some(namespace),
        ..Default::default()
    }
}

fn create_lease_spec(pod_name: String, duration: i32) -> LeaseSpec {
    LeaseSpec {
        holder_identity: Some(pod_name),
        lease_duration_seconds: Some(duration),
        acquire_time: Some(MicroTime(Utc::now())),
        renew_time: Some(MicroTime(Utc::now())),
        ..Default::default()
    }
}

fn create_lease_spec_patch(pod_name: String, duration: i32) -> LeaseSpec {
    LeaseSpec {
        holder_identity: Some(pod_name),
        lease_duration_seconds: Some(duration),
        renew_time: Some(MicroTime(Utc::now())),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    #[tokio::test]
    async fn test_leasepatchspec_object_serialize() {
        let test_agent = String::from("test-agent");
        let test_expected_agent = String::from("test-agent");
        let test_date = Utc.with_ymd_and_hms(2020, 3, 28, 15, 30, 5).unwrap();

        let test_leasepatchvalue = LeasePatchValue {
            holder_identity: Some(test_agent),
            acquire_time: MicroTime(test_date),
        };

        let test_leasepatchspec = LeasePatchSpec {
            spec: test_leasepatchvalue,
        };

        assert_eq!(
            test_leasepatchspec.spec.holder_identity.unwrap(),
            test_expected_agent
        );
        assert_eq!(test_leasepatchspec.spec.acquire_time, MicroTime(test_date));
    }
}

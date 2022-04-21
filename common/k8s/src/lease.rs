use chrono::Utc;
use k8s_openapi::api::coordination::v1::Lease;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::MicroTime;
use kube::api::{Api, ListParams, Patch, PatchParams};
use kube::core::ObjectList;
use kube::Client;
use serde::{Deserialize, Serialize};

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
                "Lease {} had now been released to {:?}",
                &lease_name,
                &patch.spec.as_ref().unwrap().holder_identity
            );
        }
        Err(e) => {
            error!("Issue releasing lease: {:?}", e)
        }
    }
}

// TODO: This may not be needed.
pub async fn get_k8s_lease_api(namespace: &str, client: Client) -> Api<Lease> {
    let lease_api: Api<Lease> = Api::namespaced(client, namespace);
    lease_api
}

async fn get_lease_list(
    lease_label: &str,
    lease_client: &Api<Lease>,
) -> Result<ObjectList<Lease>, kube::Error> {
    let lp = ListParams::default().labels(lease_label);
    let lease_info = lease_client.list(&lp).await;
    lease_info
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    #[tokio::test]
    async fn test_leasepatchspec_object_serialize() {
        let test_agent = String::from("test-agent");
        let test_expected_agent = String::from("test-agent");
        let test_date = Utc.ymd(2020, 3, 28).and_hms(15, 30, 5);

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

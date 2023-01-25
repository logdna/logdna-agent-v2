use chrono::{Duration, Utc};
use k8s_openapi::api::coordination::v1::{Lease, LeaseSpec};
use kube::{Api, Error};
use tracing::{debug, info};

use crate::lease::{self, get_lease};

pub struct FeatureLeaderMeta {
    pub interval: i32,
    pub leader: FeatureLeader,
    pub is_starting_leader: bool,
}

#[derive(Clone)]
pub struct FeatureLeader {
    pub namespace: String,
    pub feature: String,
    pub pod_name: String,
    pub lease_api: Api<Lease>,
    pub tries: i32,
}

impl FeatureLeader {
    pub fn new(
        namespace: String,
        feature: String,
        pod_name: String,
        lease_api: Api<Lease>,
    ) -> Self {
        Self {
            namespace,
            feature,
            pod_name,
            lease_api,
            tries: 0,
        }
    }

    pub async fn renew_feature_leader(&self) -> bool {
        let lease = lease::get_lease(&self.feature, &self.lease_api).await;

        match lease {
            Ok(lease) => {
                let mut leader_pod_name = String::new();
                if let Some(spec) = &lease.spec {
                    leader_pod_name = spec
                        .holder_identity
                        .as_ref()
                        .unwrap_or(&String::new())
                        .to_string();
                }

                let is_same = leader_pod_name.eq(&self.pod_name);

                if !is_same {
                    info!(
                        "Can't renew lease because this pod is not the owner {} lease {}",
                        self.pod_name, self.feature
                    );
                    return false;
                }

                let result = lease::renew_lease(
                    self.pod_name.clone(),
                    &self.lease_api,
                    self.namespace.clone(),
                    lease,
                )
                .await;

                match result {
                    true => {
                        debug!("Successfully renewed feature lease for {}", self.feature);
                        true
                    }
                    false => {
                        info!("Failed to renew feature lease for {}", self.feature);
                        false
                    }
                }
            }
            Err(e) => {
                info!("Failed to renew {} lease {}", self.feature, e);
                false
            }
        }
    }

    pub async fn try_claim_feature_leader(&self) -> bool {
        let lease = get_lease(&self.feature, &self.lease_api).await;
        if let Ok(lease) = lease {
            if let Some(spec) = lease.spec.as_ref() {
                if spec.holder_identity.is_none() || is_lease_expired(spec) {
                    // if not null, don't try and claim on start up - if is null even if we lose a race we'll have an out of date version on replace
                    let result = self.take_feature_leader_lease().await;

                    match result {
                        Ok(_) => {
                            info!(
                                "Acquired feature lease for {} on pod {}",
                                self.feature, self.pod_name
                            );
                            return true;
                        }
                        Err(e) => {
                            info!("Failed to get lease{}", e);
                            return false;
                        }
                    }
                }
            } else {
                info!("{} lease is not properly configured", self.feature)
            }
        } else {
            info!("There is no {} lease in this namespace", self.feature)
        }

        false
    }

    pub async fn get_feature_leader_lease(&self) -> Result<Lease, Error> {
        get_lease(&self.feature, &self.lease_api).await
    }

    async fn take_feature_leader_lease(&self) -> Result<Lease, Error> {
        let lease = get_lease(&self.feature, &self.lease_api).await;

        if let Ok(lease) = lease {
            lease::replace_lease(
                self.feature.to_string(),
                &lease,
                &self.lease_api,
                self.pod_name.clone(),
            )
            .await
        } else {
            lease
        }
    }
}

fn is_lease_expired(spec: &LeaseSpec) -> bool {
    if let Some(renew_time) = &spec.renew_time {
        let expire_time =
            renew_time.0 + Duration::seconds(spec.lease_duration_seconds.unwrap_or(0) as i64);
        return Utc::now().gt(&expire_time);
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::MicroTime;

    #[tokio::test]
    async fn test_lease_expiration_no_duration() {
        let lease_spec = LeaseSpec {
            acquire_time: Some(MicroTime(Utc::now() - Duration::seconds(500))),
            holder_identity: None,
            lease_duration_seconds: Some(0i32),
            lease_transitions: Some(0),
            renew_time: Some(MicroTime(Utc::now() - Duration::seconds(500))),
        };

        let result = is_lease_expired(&lease_spec);
        assert!(result);
    }

    #[tokio::test]
    async fn test_lease_expiration_duration() {
        let lease_spec = LeaseSpec {
            acquire_time: Some(MicroTime(Utc::now() - Duration::seconds(500))),
            holder_identity: None,
            lease_duration_seconds: Some(300i32),
            lease_transitions: Some(0),
            renew_time: Some(MicroTime(Utc::now() - Duration::seconds(500))),
        };

        let result = is_lease_expired(&lease_spec);
        assert!(result);
    }

    #[tokio::test]
    async fn test_lease_expiration_lease_not_expired() {
        let lease_spec = LeaseSpec {
            acquire_time: Some(MicroTime(Utc::now() - Duration::seconds(500))),
            holder_identity: None,
            lease_duration_seconds: Some(600i32),
            lease_transitions: Some(0),
            renew_time: Some(MicroTime(Utc::now() - Duration::seconds(500))),
        };

        let result = is_lease_expired(&lease_spec);
        assert!(!result);
    }

    #[tokio::test]
    async fn test_lease_expiration_no_renew() {
        let lease_spec = LeaseSpec {
            acquire_time: Some(MicroTime(Utc::now() - Duration::seconds(500))),
            holder_identity: None,
            lease_duration_seconds: Some(600i32),
            lease_transitions: Some(0),
            renew_time: None,
        };

        let result = is_lease_expired(&lease_spec);
        assert!(!result);
    }
}

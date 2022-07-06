use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct ControllerStats {
    pub controller_containers_ready: i32,
    pub controller_containers_total: i32,
    pub controller_pods_ready: i32,
    pub controller_pods_total: i32,
}

impl Default for ControllerStats {
    fn default() -> Self {
        Self::new()
    }
}

impl ControllerStats {
    pub fn new() -> Self {
        Self {
            controller_containers_ready: 0,
            controller_containers_total: 0,
            controller_pods_ready: 0,
            controller_pods_total: 0,
        }
    }

    pub fn inc_containers_ready(&mut self) {
        self.controller_containers_ready += 1;
    }

    pub fn inc_containers_total(&mut self) {
        self.controller_containers_total += 1;
    }

    pub fn inc_pods_ready(&mut self) {
        self.controller_pods_ready += 1;
    }

    pub fn inc_pods_total(&mut self) {
        self.controller_pods_total += 1;
    }

    pub fn copy_stats(&mut self, value: &ControllerStats) {
        self.controller_containers_ready = value.controller_containers_ready;
        self.controller_containers_total = value.controller_containers_total;
        self.controller_pods_ready = value.controller_pods_ready;
        self.controller_pods_total = value.controller_pods_total;
    }
}

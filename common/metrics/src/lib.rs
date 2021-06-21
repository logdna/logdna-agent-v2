#[cfg(unix)]
use jemalloc_ctl::stats::{active, active_mib, allocated, allocated_mib, resident, resident_mib};
#[cfg(unix)]
use jemalloc_ctl::{epoch, epoch_mib};

use json::object;
use lazy_static::lazy_static;
use log::info;
use prometheus::{
    exponential_buckets, register_histogram, register_histogram_vec, register_int_counter,
    register_int_counter_vec, register_int_gauge, Histogram, HistogramVec, IntCounter,
    IntCounterVec, IntGauge,
};
use std::time::{Duration, Instant};
use tokio::time::sleep;

lazy_static! {
    static ref METRICS: Metrics = Metrics::new();
    static ref FS_EVENTS: IntCounterVec = register_int_counter_vec!(
        "logdna_agent_fs_events",
        "Filesystem events received",
        &["event_type"]
    )
    .unwrap();
    static ref FS_LINES: IntCounter =
        register_int_counter!("logdna_agent_fs_lines", "Number of lines parsed by the Filesystem module").unwrap();
    static ref FS_FILES: IntGauge =
        register_int_gauge!("logdna_agent_fs_files", "Number of open files").unwrap();
    static ref FS_BYTES: IntCounter =
        register_int_counter!("logdna_agent_fs_bytes", "Number of bytes read by the Filesystem module").unwrap();
    static ref FS_PARTIAL_READS: IntCounter =
        register_int_counter!("logdna_agent_fs_partial_reads", "Filesystem partial reads").unwrap();
    static ref INGEST_RETRIES: IntCounter = register_int_counter!(
        "logdna_agent_ingest_retries",
        "Retry attempts made to the http ingestion service"
    )
    .unwrap();
    static ref INGEST_RATE_LIMIT_HITS: IntCounter = register_int_counter!(
        "logdna_agent_ingest_rate_limit_hits",
        "Number of times the http request was delayed due to the rate limiter"
    )
    .unwrap();
    static ref INGEST_REQUEST_SIZE: Histogram = register_histogram!(
        "logdna_agent_ingest_request_size",
        "Size in bytes of the requests made to http ingestion service",
        // Buckets ranging from 500 bytes to 2Mb
        exponential_buckets(500.0, 2.0, 13).unwrap()
    )
    .unwrap();
    static ref INGEST_REQUEST_DURATION: HistogramVec = register_histogram_vec!(
        "logdna_agent_ingest_request_duration_millis",
        "Latency of the requests made to http ingestion service",
        &["outcome"],
        // Buckets ranging from 0.1ms to 26s
        exponential_buckets(0.1, 4.0, 10).unwrap()
    )
    .unwrap();
    static ref K8S_EVENTS: IntCounterVec = register_int_counter_vec!(
        "logdna_agent_k8s_events",
        "Kubernetes events received",
        &["event_type"]
    )
    .unwrap();
    static ref K8S_LINES: IntCounter =
        register_int_counter!("logdna_agent_k8s_lines", "Kubernetes event lines read").unwrap();
    static ref JOURNAL_RECORDS: Histogram = register_histogram!(
        "logdna_agent_journald_records",
        "Size of the Journald log entries read"
    )
    .unwrap();
}

mod labels {
    pub const CREATE: &str = "create";
    pub const DELETE: &str = "delete";
    pub const WRITE: &str = "write";
    pub const SUCCESS: &str = "success";
    pub const FAILURE: &str = "failure";
    pub const TIMEOUT: &str = "timeout";
}

pub struct Metrics {
    fs: Fs,
    #[cfg(unix)]
    memory: Memory,
    http: Http,
    k8s: K8s,
    journald: Journald,
}

impl Metrics {
    fn new() -> Self {
        Self {
            fs: Fs::new(),
            #[cfg(unix)]
            memory: Memory::new(),
            http: Http::new(),
            k8s: K8s::new(),
            journald: Journald::new(),
        }
    }

    pub async fn log_periodically() {
        loop {
            sleep(Duration::from_secs(60)).await;
            info!("{}", Metrics::print());
        }
    }

    pub fn fs() -> &'static Fs {
        &METRICS.fs
    }

    #[cfg(unix)]
    pub fn memory() -> &'static Memory {
        &METRICS.memory
    }

    pub fn http() -> &'static Http {
        &METRICS.http
    }

    pub fn k8s() -> &'static K8s {
        &METRICS.k8s
    }

    pub fn journald() -> &'static Journald {
        &METRICS.journald
    }

    pub fn print() -> String {

        let fs_create = FS_EVENTS.with_label_values(&[labels::CREATE]).get();
        let fs_delete = FS_EVENTS.with_label_values(&[labels::DELETE]).get();
        let fs_write = FS_EVENTS.with_label_values(&[labels::WRITE]).get();
        let k8s_create = K8S_EVENTS.with_label_values(&[labels::CREATE]).get();
        let k8s_delete = K8S_EVENTS.with_label_values(&[labels::DELETE]).get();
        let latency_success = INGEST_REQUEST_DURATION.with_label_values(&[labels::SUCCESS]);
        let latency_failure = INGEST_REQUEST_DURATION.with_label_values(&[labels::FAILURE]);
        let latency_timeout = INGEST_REQUEST_DURATION.with_label_values(&[labels::TIMEOUT]);

        let object = object! {
            "fs" => object!{
                "events" => fs_create + fs_delete + fs_write,
                "creates" => fs_create,
                "deletes" => fs_delete,
                "writes" => fs_write,
                "lines" => FS_LINES.get(),
                "bytes" => FS_BYTES.get(),
                "files_tracked" => FS_FILES.get(),
                "partial_reads" => FS_PARTIAL_READS.get(),
            },
            // CPU and memory metrics are exported to Prometheus by default only on linux.
            // We still rely on jemalloc stats for this periodic printing the memory metrics
            // as it supports more platforms
            "memory" => {
                #[cfg(unix)]
                {
                    let memory = Metrics::memory();
                    object!{
                        "active" => memory.read_active(),
                        "allocated" => memory.read_allocated(),
                        "resident" => memory.read_resident(),
                    }
                }
                #[cfg(not(unix))]
                object!{}
            },
            "ingest" => object!{
                "requests" => INGEST_REQUEST_SIZE.get_sample_count(),
                "requests_size" => INGEST_REQUEST_SIZE.get_sample_sum(),
                "rate_limits" => INGEST_RATE_LIMIT_HITS.get(),
                "retries" => INGEST_RETRIES.get(),
                // The request duration is exported as a histogram in Prometheus,
                // in this output is a simple sum
                "requests_duration" => latency_success.get_sample_sum() + latency_failure.get_sample_sum() + latency_timeout.get_sample_sum(),
                "requests_timed_out" => latency_timeout.get_sample_count(),
                "requests_failed" => latency_failure.get_sample_count(),
                "requests_succeeded" => latency_success.get_sample_count(),
            },
            "k8s" => object!{
                "lines" => K8S_LINES.get(),
                "creates" => k8s_create,
                "deletes" => k8s_delete,
                "events" => k8s_create + k8s_delete,
            },
            "journald" => object!{
                "lines" => JOURNAL_RECORDS.get_sample_count(),
                "bytes" => JOURNAL_RECORDS.get_sample_sum(),
            },
        };

        object.to_string()
    }
}

#[derive(Default)]
pub struct Fs {}

impl Fs {
    pub fn new() -> Self {
        Self {}
    }

    pub fn increment_creates(&self) {
        FS_EVENTS.with_label_values(&[labels::CREATE]).inc();
    }

    pub fn increment_deletes(&self) {
        FS_EVENTS.with_label_values(&[labels::DELETE]).inc();
    }

    pub fn increment_writes(&self) {
        FS_EVENTS.with_label_values(&[labels::WRITE]).inc();
    }

    pub fn increment_lines(&self) {
        FS_LINES.inc();
    }

    pub fn increment_tracked_files(&self) {
        FS_FILES.inc();
    }

    pub fn decrement_tracked_files(&self) {
        FS_FILES.dec();
    }

    pub fn add_bytes(&self, num: u64) {
        FS_BYTES.inc_by(num);
    }

    pub fn increment_partial_reads(&self) {
        FS_PARTIAL_READS.inc();
    }
}

#[cfg(unix)]
pub struct Memory {
    epoch_mib: epoch_mib,
    active_mib: active_mib,
    allocated_mib: allocated_mib,
    resident_mib: resident_mib,
}

#[cfg(unix)]
impl Memory {
    pub fn new() -> Self {
        Self {
            epoch_mib: epoch::mib().unwrap(),
            active_mib: active::mib().unwrap(),
            allocated_mib: allocated::mib().unwrap(),
            resident_mib: resident::mib().unwrap(),
        }
    }

    pub fn read_active(&self) -> u64 {
        self.epoch_mib.advance().unwrap();
        self.active_mib.read().unwrap() as u64
    }

    pub fn read_allocated(&self) -> u64 {
        self.epoch_mib.advance().unwrap();
        self.allocated_mib.read().unwrap() as u64
    }

    pub fn read_resident(&self) -> u64 {
        self.epoch_mib.advance().unwrap();
        self.resident_mib.read().unwrap() as u64
    }
}

#[cfg(unix)]
impl Default for Memory {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Default)]
pub struct Http {}

impl Http {
    pub fn new() -> Self {
        Self {}
    }

    pub fn increment_limit_hits(&self) {
        INGEST_RATE_LIMIT_HITS.inc();
    }

    pub fn add_request_size(&self, num: u64) {
        INGEST_REQUEST_SIZE.observe(num as f64);
    }

    pub fn add_request_success(&self, start: Instant) {
        INGEST_REQUEST_DURATION
            .with_label_values(&[labels::SUCCESS])
            .observe(elapsed(start))
    }

    pub fn add_request_failure(&self, start: Instant) {
        INGEST_REQUEST_DURATION
            .with_label_values(&[labels::FAILURE])
            .observe(elapsed(start))
    }

    pub fn add_request_timeout(&self, start: Instant) {
        INGEST_REQUEST_DURATION
            .with_label_values(&[labels::TIMEOUT])
            .observe(elapsed(start))
    }

    pub fn increment_retries(&self) {
        INGEST_RETRIES.inc();
    }
}

#[derive(Default)]
pub struct K8s {}

impl K8s {
    pub fn new() -> Self {
        Self {}
    }

    pub fn increment_lines(&self) {
        K8S_LINES.inc();
    }

    pub fn increment_creates(&self) {
        K8S_EVENTS.with_label_values(&[labels::CREATE]).inc();
    }

    pub fn increment_deletes(&self) {
        K8S_EVENTS.with_label_values(&[labels::DELETE]).inc();
    }
}

#[derive(Default)]
pub struct Journald {}

impl Journald {
    pub fn new() -> Self {
        Self {}
    }

    pub fn add_bytes(&self, num: usize) {
        JOURNAL_RECORDS.observe(num as f64);
    }
}

fn elapsed(start: Instant) -> f64 {
    start.elapsed().as_micros() as f64 / 1_000.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ops::Sub;

    /// Verifies that increments/marks and printing does not panic
    #[test]
    fn print_should_return_json() {
        METRICS.fs.increment_creates();
        METRICS.fs.increment_deletes();
        METRICS.fs.increment_writes();
        METRICS.fs.increment_lines();
        METRICS.fs.increment_partial_reads();
        METRICS.fs.add_bytes(123);
        METRICS.http.add_request_size(12);
        METRICS
            .http
            .add_request_success(Instant::now().sub(Duration::from_micros(8137)));
        METRICS
            .http
            .add_request_failure(Instant::now().sub(Duration::from_micros(1137)));
        METRICS
            .http
            .add_request_timeout(Instant::now().sub(Duration::from_micros(20137)));
        METRICS.http.increment_limit_hits();
        METRICS.http.increment_retries();
        METRICS.journald.add_bytes(32);
        METRICS.k8s.increment_lines();
        METRICS.k8s.increment_deletes();
        METRICS.k8s.increment_creates();
        let result = Metrics::print();
        assert!(result.starts_with('{') && result.ends_with('}'));
    }
}

use common::AgentSettings;
pub use common::*;
use hyper::StatusCode;
use prometheus_parse::{Sample, Scrape, Value};
use rand::Rng;
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tempfile::tempdir;

mod common;

#[tokio::test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
async fn test_retry_location() {
    let _ = env_logger::Builder::from_default_env().try_init();
    let timeout = 200;
    let base_delay_ms = 300;
    let step_delay_ms = 100;
    let metrics_port = 9881;

    let log_dir = tempdir().unwrap().into_path();
    let log_file_path = log_dir.join("test.log");
    let mut log_file = File::create(&log_file_path).expect("Couldn't create temp log file...");

    // Simulate a slow ingest API
    let (server, _, shutdown_ingest, address) = start_ingester(Box::new(|_| None), {
        Box::new(move |body| {
            if body
                .lines
                .iter()
                .any(|l| l.file.as_deref().unwrap().contains("test.log"))
            {
                return Some(Box::pin(tokio::time::sleep(Duration::from_millis(
                    timeout + 20,
                ))));
            }
            None
        })
    });

    // Agent Process
    let retry_dir = tempdir().unwrap().into_path();

    let mut settings = AgentSettings::with_mock_ingester(log_dir.to_str().unwrap(), &address);
    let config_file_path = get_config_file(
        timeout,
        base_delay_ms,
        step_delay_ms,
        Some(retry_dir.clone()),
    );
    settings.config_file = config_file_path.to_str();
    settings.metrics_port = Some(metrics_port);

    let mut agent_handle = common::spawn_agent(settings);
    let agent_stderr = agent_handle.stderr.take().unwrap();
    common::consume_output(agent_stderr);

    let (server_result, _) = tokio::join!(server, async move {
        // Wait for the agent to bootstrap and then start generating some log data
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        gen_log_data(&mut log_file).await;

        // Check that a retry file was created in the retry dir
        let matches = std::fs::read_dir(retry_dir).unwrap().filter(|result| {
            if let Ok(path) = result {
                path.file_name().to_string_lossy().ends_with("\\.retry")
            } else {
                false
            }
        });
        assert!(matches.count() > 0);

        shutdown_ingest();
    });

    server_result.unwrap();
    agent_handle.kill().unwrap();
}

#[tokio::test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
async fn test_retry_after_timeout() {
    let _ = env_logger::Builder::from_default_env().try_init();
    let timeout = 200;
    let base_delay_ms = 300;
    let step_delay_ms = 100;
    let attempts = 10;
    let config_file_path = get_config_file(timeout, base_delay_ms, step_delay_ms, None);

    let dir = tempdir().unwrap().into_path();
    let file_path = dir.join("test.log");
    let mut file = File::create(&file_path).expect("Couldn't create temp log file...");

    let attempts_counter = Arc::new(AtomicI64::new(0));
    let counter = attempts_counter.clone();
    let (server, received, shutdown_handle, address) = start_ingester(
        Box::new(|_| None),
        Box::new(move |body| {
            if body
                .lines
                .iter()
                .any(|l| l.file.as_deref().unwrap().contains("test.log"))
            {
                counter.fetch_add(1, Ordering::SeqCst);
                if counter.load(Ordering::SeqCst) < attempts {
                    // Sleep enough time to mark the request as timed out by the client
                    return Some(Box::pin(tokio::time::sleep(Duration::from_millis(
                        timeout + 20,
                    ))));
                }
            }
            None
        }),
    );

    let mut settings = AgentSettings::with_mock_ingester(dir.to_str().unwrap(), &address);
    settings.config_file = config_file_path.to_str();
    let mut agent_handle = common::spawn_agent(settings);
    let agent_stderr = agent_handle.stderr.take().unwrap();
    common::consume_output(agent_stderr);

    let (server_result, _) = tokio::join!(server, async move {
        // Wait for the server to catch up
        tokio::time::sleep(tokio::time::Duration::from_millis(400)).await;

        let file_lines = &["hello\n", " world\n", " world2\n", " world3\n", " world4\n"];
        for item in file_lines {
            write!(file, "{}", item).unwrap();
        }

        // Wait for the data to be received by the mock ingester
        tokio::time::sleep(tokio::time::Duration::from_millis(
            base_delay_ms + (timeout + step_delay_ms) * (attempts as u64),
        ))
        .await;

        // Sleep for long enough for retry to tick
        tokio::time::sleep(tokio::time::Duration::from_millis(6000)).await;

        let map = received.lock().await;
        let file_info = map.get(file_path.to_str().unwrap()).unwrap();
        // Received it multiple times
        assert!(file_info.values.len() >= file_lines.len());

        let attempts_made = attempts_counter.load(Ordering::SeqCst);

        // It retried multiple times
        assert!(
            i64::abs(attempts_made - attempts) <= 2,
            "{} attempts received, expected {}",
            attempts_made,
            attempts
        );
        shutdown_handle();
    });

    server_result.unwrap();
    agent_handle.kill().unwrap();
}

#[tokio::test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
async fn test_retry_is_not_made_before_retry_base_delay_ms() {
    let _ = env_logger::Builder::from_default_env().try_init();
    // Use a large base delay
    let base_delay_ms = 300_000;
    let timeout = 200;
    let config_file_path = get_config_file(timeout, base_delay_ms, 100, None);

    let dir = tempdir().unwrap().into_path();
    let file_path = dir.join("test.log");
    let mut file = File::create(&file_path).expect("Couldn't create temp log file...");

    let attempts_counter = Arc::new(AtomicUsize::new(0));
    let counter = attempts_counter.clone();
    let (server, _, shutdown_handle, address) = start_ingester(
        Box::new(|_| None),
        Box::new(move |body| {
            if body
                .lines
                .iter()
                .any(|l| l.file.as_deref().unwrap().contains("test.log"))
            {
                counter.fetch_add(1, Ordering::SeqCst);
                // Sleep enough time to mark the request as timed out by the client
                return Some(Box::pin(tokio::time::sleep(Duration::from_millis(
                    timeout + 20,
                ))));
            }
            None
        }),
    );

    let mut settings = AgentSettings::with_mock_ingester(dir.to_str().unwrap(), &address);
    settings.config_file = config_file_path.to_str();
    let mut agent_handle = common::spawn_agent(settings);
    let agent_stderr = agent_handle.stderr.take().unwrap();
    common::consume_output(agent_stderr);

    let (server_result, _) = tokio::join!(server, async move {
        // Wait for the server to catch up
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

        write!(file, "hello\nworld\n").unwrap();

        // Wait for the data to be received by the mock ingester / retried
        tokio::time::sleep(tokio::time::Duration::from_secs(4)).await;
        let attempts_made = attempts_counter.load(Ordering::SeqCst);
        // It was not retried
        assert_eq!(attempts_made, 1);
        shutdown_handle();
    });

    server_result.unwrap();
    agent_handle.kill().unwrap();
}

async fn gen_log_data(file: &mut File) {
    let mut rng = rand::thread_rng();
    let total_lines = rng.gen_range(10..50);
    for _ in 0..total_lines {
        let line_len = rng.gen_range(5..80);
        let line: String = rand::thread_rng()
            .sample_iter(&rand::distributions::Alphanumeric)
            .take(line_len)
            .map(char::from)
            .collect();

        writeln!(file, "{}", line).unwrap();
        file.flush().unwrap();
    }
    tokio::time::sleep(tokio::time::Duration::from_millis(250)).await;
}

#[derive(Debug)]
struct MetricRec {
    timestamp_ms: i64,
    raw_value: f64,
}

impl MetricRec {
    fn new(timestamp_ms: i64, raw_value: f64) -> Self {
        Self {
            timestamp_ms,
            raw_value,
        }
    }
}

#[derive(Debug)]
struct TestMetrics {
    min_timestamp_ms: AtomicI64,
    max_timestamp_ms: AtomicI64,
    retry_pending: Vec<MetricRec>,
    retry_storage_used: Vec<MetricRec>,
    retry_success: Vec<MetricRec>,
    retry_failure: Vec<MetricRec>,
}

impl TestMetrics {
    fn new() -> Self {
        TestMetrics {
            min_timestamp_ms: AtomicI64::new(i64::MAX),
            max_timestamp_ms: AtomicI64::new(i64::MIN),
            retry_pending: Vec::new(),
            retry_storage_used: Vec::new(),
            retry_success: Vec::new(),
            retry_failure: Vec::new(),
        }
    }

    fn record(&mut self, sample: &Sample) {
        let ts = sample.timestamp.timestamp_millis();
        self.min_timestamp_ms.fetch_min(ts, Ordering::Relaxed);
        self.max_timestamp_ms.fetch_max(ts, Ordering::Relaxed);

        match sample.metric.as_str() {
            "logdna_agent_retry_pending" => {
                if let Value::Gauge(v) = sample.value {
                    self.retry_pending.push(MetricRec::new(ts, v));
                }
            }
            "logdna_agent_retry_storage_used" => {
                if let Value::Gauge(v) = sample.value {
                    self.retry_storage_used.push(MetricRec::new(ts, v));
                }
            }
            "logdna_agent_ingest_retries_success" => {
                if let Value::Counter(c) = sample.value {
                    self.retry_success.push(MetricRec::new(ts, c));
                }
            }
            "logdna_agent_ingest_retries_failure" => {
                if let Value::Counter(c) = sample.value {
                    self.retry_failure.push(MetricRec::new(ts, c));
                }
            }
            _ => {}
        }
    }
}

#[tokio::test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
async fn test_retry_metrics_emitted() {
    let _ = env_logger::Builder::from_default_env().try_init();
    let timeout = 200;
    let base_delay_ms = 300;
    let step_delay_ms = 100;
    let metrics_port = 9881;

    let dir = tempdir().unwrap().into_path();
    let file_path = dir.join("test.log");
    let mut file = File::create(&file_path).expect("Couldn't create temp log file...");

    // Generate a mock ingestion service that can be toggled between normal speed and slow running
    // Slow responses should trigger a retry when the agent is set with small timeout values.
    let simulate_ingest_problems = Arc::new(AtomicBool::new(false));
    let (server, _, shutdown_ingest, address) = start_ingester(Box::new(|_| None), {
        let simulate_ingest_problems = simulate_ingest_problems.clone();
        Box::new(move |body| {
            if body
                .lines
                .iter()
                .any(|l| l.file.as_deref().unwrap().contains("test.log"))
                && simulate_ingest_problems.load(Ordering::Relaxed)
            {
                return Some(Box::pin(tokio::time::sleep(Duration::from_millis(
                    timeout + 20,
                ))));
            }
            None
        })
    });

    // Agent Process
    let mut settings = AgentSettings::with_mock_ingester(dir.to_str().unwrap(), &address);
    let config_file_path = get_config_file(timeout, base_delay_ms, step_delay_ms, None);
    settings.config_file = config_file_path.to_str();
    settings.metrics_port = Some(metrics_port);

    let mut agent_handle = common::spawn_agent(settings);
    let agent_stderr = agent_handle.stderr.take().unwrap();
    common::consume_output(agent_stderr);

    // This creates a new thread that scrapes the metrics from the agent process and
    // stores all the values for the retry metrics under test.
    let scrape_metrics = Arc::new(AtomicBool::new(true));
    let rec_metrics = Arc::new(Mutex::new(TestMetrics::new()));

    let metrics_handle = tokio::spawn({
        let scrape_metrics = scrape_metrics.clone();
        let rec_metrics = rec_metrics.clone();

        async move {
            while scrape_metrics.load(Ordering::Relaxed) {
                if let Ok((StatusCode::OK, Some(data))) =
                    common::fetch_agent_metrics(metrics_port).await
                {
                    let lines = data.lines().map(|l| Ok(l.to_string()));
                    for sample in Scrape::parse(lines).unwrap().samples {
                        rec_metrics.lock().as_mut().unwrap().record(&sample);
                    }
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    });

    let (server_result, _, _) = tokio::join!(server, metrics_handle, async move {
        // Wait for the agent to bootstrap and then start generating some log data
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        gen_log_data(&mut file).await;

        // Signal to the mock ingestor to start doing random rejections on log data.
        simulate_ingest_problems.store(true, Ordering::Relaxed);
        gen_log_data(&mut file).await;
        gen_log_data(&mut file).await;

        // Signal to the mock ingestor to stop doing random rejections
        simulate_ingest_problems.store(false, Ordering::Relaxed);
        tokio::time::sleep(tokio::time::Duration::from_millis(6000)).await;
        gen_log_data(&mut file).await;

        // Shut down the scraper and ingest server
        scrape_metrics.store(false, Ordering::Relaxed);
        shutdown_ingest();
    });

    server_result.unwrap();
    agent_handle.kill().unwrap();

    // Assertions
    //
    // There should be at least 1 metric recorded for each of the retry metrics.
    let rec_metrics = rec_metrics.lock().unwrap();

    assert!(!rec_metrics.retry_pending.is_empty());
    assert!(!rec_metrics.retry_success.is_empty());
    assert!(!rec_metrics.retry_failure.is_empty());
    assert!(!rec_metrics.retry_storage_used.is_empty());

    // The pending count should start and end at 0 since the agent starts normal and
    // then recovers. This metric is a gauge and is allowed to move up and down.
    let pending_first = rec_metrics.retry_pending.get(0).unwrap();
    let pending_last = rec_metrics.retry_pending.iter().last().unwrap();
    assert!(pending_first.raw_value < f64::EPSILON);
    assert!(pending_last.raw_value < f64::EPSILON);

    // The pending count should have some recorded value that greater than 0 if the
    // agent did actually enter a retry loop.
    rec_metrics.retry_pending.iter().any(|r| r.raw_value > 0.0);

    // The retry success/failure counts first metric data point should only occur after
    // the first recorded metric and continue since they are counters and counters only
    // emit on the first increment.
    let success_first = rec_metrics.retry_success.get(0).unwrap();
    let failure_first = rec_metrics.retry_failure.get(0).unwrap();
    assert!(rec_metrics.min_timestamp_ms.load(Ordering::Relaxed) < success_first.timestamp_ms);
    assert!(rec_metrics.min_timestamp_ms.load(Ordering::Relaxed) < failure_first.timestamp_ms);

    let success_last = rec_metrics.retry_success.iter().last().unwrap();
    let failure_last = rec_metrics.retry_failure.iter().last().unwrap();
    assert!(success_last.timestamp_ms <= rec_metrics.max_timestamp_ms.load(Ordering::Relaxed));
    assert!(failure_last.timestamp_ms <= rec_metrics.max_timestamp_ms.load(Ordering::Relaxed));

    // Each of the success counts should be greater than or equal to the prior count.
    assert!(rec_metrics.retry_success.windows(2).all(|w| match w {
        [a, b] => a.raw_value <= b.raw_value,
        _ => false,
    }));

    // Each of the failure counts should be greater than or equal to the prior count.
    assert!(rec_metrics.retry_failure.windows(2).all(|w| match w {
        [a, b] => a.raw_value <= b.raw_value,
        _ => false,
    }));

    // The amount of space used for retries should increase but then eventually decrease to to zero
    // as the agent recovers. Note that zero values won't appear at first since nothing has reported
    // a metric until a retry attempt.
    assert!(rec_metrics
        .retry_storage_used
        .iter()
        .any(|r| r.raw_value > 0.0));
    assert!(
        rec_metrics
            .retry_storage_used
            .iter()
            .last()
            .unwrap()
            .raw_value
            < f64::EPSILON
    );
}

/// Creates a temp config file with required fields and the provided parameters
fn get_config_file(
    timeout: u64,
    retry_base_delay_ms: u64,
    retry_step_delay_ms: u64,
    retry_dir: Option<PathBuf>,
) -> PathBuf {
    let config_dir = tempdir().unwrap().into_path();
    let config_file_path = config_dir.join("config.yaml");
    let mut config_file = File::create(&config_file_path).unwrap();

    let retry_dir_line = retry_dir
        .map(|p| format!("  retry_dir: {}", p.to_string_lossy()))
        .unwrap_or("".to_string());

    write!(
        config_file,
        "
http:
  timeout: {}
  endpoint: /logs/agent
  use_ssl: false
  use_compression: true
  gzip_level: 2
  params:
    hostname: abc
    tags: tag1
    now: 0
  body_size: 2097152
  retry_base_delay_ms: {}
  retry_step_delay_ms: {}
{}
log:
  dirs:
  - /var/log/
  include:
    glob:
      - \"*.log\"
    regex: []
  exclude:
    glob:
      - /var/log/wtmp
      - /var/log/btmp
    regex: []
journald: {{}}
",
        timeout, retry_base_delay_ms, retry_step_delay_ms, retry_dir_line
    )
    .unwrap();

    config_file_path
}

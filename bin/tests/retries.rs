use common::AgentSettings;
pub use common::*;
use logdna_metrics_recorder::*;
use prometheus_parse::Value;
use rand::Rng;
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tempfile::tempdir;

mod common;

#[tokio::test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
async fn test_retry_disk_limit() {
    let _ = env_logger::Builder::from_default_env().try_init();
    let timeout = 200;
    let base_delay_ms = 2000; // set high to cause retries to pile up
    let step_delay_ms = 500;
    let disk_limit = 7 * 1024;

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
                return Some(Box::pin(tokio::time::sleep(Duration::from_millis(timeout))));
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
        Some(disk_limit),
    );
    settings.config_file = config_file_path.to_str();

    let mut agent_handle = common::spawn_agent(settings);
    let agent_stderr = agent_handle.stderr.take().unwrap();
    common::consume_output(agent_stderr);

    let (server_result, _) = tokio::join!(server, async move {
        // Wait for the agent to bootstrap and then start generating some log data
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        for _ in 0..5 {
            gen_log_data(&mut log_file).await;
        }

        // Check that a retry file was created in the retry dir
        let used = std::fs::read_dir(retry_dir).unwrap().filter(
            |r| !matches!(r, Ok(path) if path.file_name().to_string_lossy().ends_with("\\.retry")),
        ).fold(0u64, |acc, result|
            match result {
                Ok(entry) => {
                    acc + entry.metadata().map(|md| md.len()).unwrap_or_default()
                },
                _ => acc,
            });

        assert!(used < disk_limit);
        shutdown_ingest();
    });

    server_result.unwrap();
    agent_handle.kill().unwrap();
}

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
                    timeout * 2,
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
        None,
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
        let matches = std::fs::read_dir(retry_dir).unwrap().filter(
            |r| !matches!(r, Ok(path) if path.file_name().to_string_lossy().ends_with("\\.retry")),
        );
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
    let retry_dir = tempdir().unwrap().into_path();
    let config_file_path =
        get_config_file(timeout, base_delay_ms, step_delay_ms, Some(retry_dir), None);

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
    let retry_dir = tempdir().unwrap().into_path();
    let config_file_path = get_config_file(timeout, base_delay_ms, 100, Some(retry_dir), None);

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

#[tokio::test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
async fn test_retry_metrics_emitted() {
    let _ = env_logger::Builder::from_default_env().try_init();
    let timeout = 200;
    let base_delay_ms = 300;
    let step_delay_ms = 100;
    let metrics_port = 9881;

    let log_dir = tempdir().unwrap().into_path();
    let log_file_path = log_dir.join("test.log");
    let mut log_file = File::create(&log_file_path).expect("Couldn't create temp log file...");

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
    let mut settings = AgentSettings::with_mock_ingester(log_dir.to_str().unwrap(), &address);
    let retry_dir = tempdir().unwrap().into_path();
    let config_file_path =
        get_config_file(timeout, base_delay_ms, step_delay_ms, Some(retry_dir), None);
    settings.config_file = config_file_path.to_str();
    settings.metrics_port = Some(metrics_port);

    let mut agent_handle = common::spawn_agent(settings);
    let agent_stderr = agent_handle.stderr.take().unwrap();
    common::consume_output(agent_stderr);

    // This creates a new thread that scrapes the metrics from the agent process and
    // stores all the values for the retry metrics under test.
    let recorder = MetricsRecorder::start(metrics_port, Some(Duration::from_millis(100)));

    let (ingest_result, metrics_result) = tokio::join!(server, async move {
        // Wait for the agent to bootstrap and then start generating some log data
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        gen_log_data(&mut log_file).await;

        // Signal to the mock ingestor to start doing random rejections on log data.
        simulate_ingest_problems.store(true, Ordering::Relaxed);
        gen_log_data(&mut log_file).await;
        gen_log_data(&mut log_file).await;

        // Signal to the mock ingestor to stop doing random rejections
        simulate_ingest_problems.store(false, Ordering::Relaxed);
        gen_log_data(&mut log_file).await;
        tokio::time::sleep(tokio::time::Duration::from_millis(6000)).await;
        gen_log_data(&mut log_file).await;

        shutdown_ingest();
        recorder.stop().await
    });

    // Shut down processes
    ingest_result.unwrap();
    agent_handle.kill().unwrap();

    // Extract a set of metrics that are revelent for this test
    let retry_pending = metrics_result
        .iter()
        .filter_map(|s| match s.value {
            Value::Gauge(raw) if s.metric.as_str() == "logdna_agent_retry_pending" => {
                Some((s.timestamp.timestamp_millis(), raw))
            }
            _ => None,
        })
        .collect::<Vec<(i64, f64)>>();

    let retries = metrics_result
        .iter()
        .filter_map(|s| match s.value {
            Value::Counter(raw) if s.metric.as_str() == "logdna_agent_ingest_retries" => {
                Some((s.timestamp.timestamp_millis(), raw))
            }
            _ => None,
        })
        .collect::<Vec<(i64, f64)>>();

    let retry_success = metrics_result
        .iter()
        .filter_map(|s| match s.value {
            Value::Counter(raw) if s.metric.as_str() == "logdna_agent_ingest_retries_success" => {
                Some((s.timestamp.timestamp_millis(), raw))
            }
            _ => None,
        })
        .collect::<Vec<(i64, f64)>>();

    let retry_failure = metrics_result
        .iter()
        .filter_map(|s| match s.value {
            Value::Counter(raw) if s.metric.as_str() == "logdna_agent_ingest_retries_failure" => {
                Some((s.timestamp.timestamp_millis(), raw))
            }
            _ => None,
        })
        .collect::<Vec<(i64, f64)>>();

    let retry_storage_used = metrics_result
        .iter()
        .filter_map(|s| match s.value {
            Value::Gauge(raw) if s.metric.as_str() == "logdna_agent_retry_storage_used" => {
                Some((s.timestamp.timestamp_millis(), raw))
            }
            _ => None,
        })
        .collect::<Vec<(i64, f64)>>();

    let min_timestamp_ms = metrics_result
        .iter()
        .map(|s| s.timestamp.timestamp_millis())
        .min()
        .unwrap();

    let max_timestamp_ms = metrics_result
        .iter()
        .map(|s| s.timestamp.timestamp_millis())
        .max()
        .unwrap();

    // Assertions
    //
    // There should be at least 1 metric recorded for each of the retry metrics.
    assert!(!retry_pending.is_empty());
    assert!(!retry_success.is_empty());
    assert!(!retry_failure.is_empty());
    assert!(!retry_storage_used.is_empty());

    // The pending count should start and end at 0 since the agent starts normal and
    // then recovers. This metric is a gauge and is allowed to move up and down.
    let pending_first = retry_pending.get(0).unwrap();
    let pending_last = retry_pending.iter().last().unwrap();
    assert!(pending_first.1 < f64::EPSILON);
    assert!(pending_last.1 < f64::EPSILON);

    // The pending count should have some recorded value that greater than 0 if the
    // agent did actually enter a retry loop.
    retry_pending.iter().any(|r| r.1 > 0.0);

    // The retry success/failure counts first metric data point should only occur after
    // the first recorded metric and continue since they are counters and counters only
    // emit on the first increment.
    let success_first = retry_success.get(0).unwrap();
    let failure_first = retry_failure.get(0).unwrap();
    assert!(min_timestamp_ms < success_first.0);
    assert!(min_timestamp_ms < failure_first.0);

    let success_last = retry_success.iter().last().unwrap();
    let failure_last = retry_failure.iter().last().unwrap();
    assert!(success_last.0 <= max_timestamp_ms);
    assert!(failure_last.0 <= max_timestamp_ms);

    // Each of the success counts should be greater than or equal to the prior count.
    assert!(retry_success.windows(2).all(|w| match w {
        [a, b] => a.1 <= b.1,
        _ => false,
    }));

    // Each of the failure counts should be greater than or equal to the prior count.
    assert!(retry_failure.windows(2).all(|w| match w {
        [a, b] => a.1 <= b.1,
        _ => false,
    }));

    // The total retries should be equal to or greater than the sum of successful and failed retries.
    // Depending on when the metrics are scraped and where in the code the execution is, e.g. retry
    // is currently in process, we may have an attempt without a success or failure. The pending
    // count is really tracking only number of requests on disk.
    //
    // This assertion tries to find a common data range to compare based on timestamp since the
    // metrics may have been running and scaped before the actual test case starts. We also track
    // the starting count values to reset the series of data to ignore any values held prior to
    // scraping.
    let (attempts_start_ts, attempt_count_basis) = retries.get(0).unwrap();
    let (success_start_ts, success_count_basis) = retry_success.get(0).unwrap();
    let (failure_start_ts, failure_count_basis) = retry_failure.get(0).unwrap();
    let compare_window_start = std::cmp::max(
        std::cmp::max(*attempts_start_ts, *success_start_ts),
        *failure_start_ts,
    );

    let attempt_counts = retries
        .iter()
        .filter(|(ts, _)| *ts >= compare_window_start)
        .map(|(_, count)| *count - attempt_count_basis);

    let success_counts = retry_success
        .iter()
        .filter(|(ts, _)| *ts >= compare_window_start)
        .map(|(_, count)| *count - success_count_basis);

    let failure_counts = retry_failure
        .iter()
        .filter(|(ts, _)| *ts >= compare_window_start)
        .map(|(_, count)| *count - failure_count_basis);

    assert!(success_counts
        .zip(failure_counts)
        .zip(attempt_counts)
        .all(|((success, failure), total)| { success + failure <= total }));

    // The amount of space used for retries should increase but then eventually decrease to to zero
    // as the agent recovers. Note that zero values won't appear at first since nothing has reported
    // a metric until a retry attempt.
    assert!(retry_storage_used.iter().any(|r| r.1 > 0.0));
    assert!(retry_storage_used.iter().last().unwrap().1 < f64::EPSILON);
}

/// Creates a temp config file with required fields and the provided parameters
fn get_config_file(
    timeout: u64,
    retry_base_delay_ms: u64,
    retry_step_delay_ms: u64,
    retry_dir: Option<PathBuf>,
    retry_disk_limit: Option<u64>,
) -> PathBuf {
    let config_dir = tempdir().unwrap().into_path();
    let config_file_path = config_dir.join("config.yaml");
    let mut config_file = File::create(&config_file_path).unwrap();

    let retry_dir_line = retry_dir
        .map(|p| format!("  retry_dir: {}", p.to_string_lossy()))
        .unwrap_or_default();

    let disk_limit_line = retry_disk_limit
        .map(|u| format!("  retry_disk_limit: {}", u))
        .unwrap_or_default();

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
startup: {{}}
",
        timeout, retry_base_delay_ms, retry_step_delay_ms, retry_dir_line, disk_limit_line
    )
    .unwrap();

    config_file_path
}

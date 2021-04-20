use common::AgentSettings;
pub use common::*;
use logdna_mock_ingester::{
    http_ingester_with_processors, FileLineCounter, IngestBody, IngestError,
};
use std::fs::File;
use std::future::Future;
use std::io::Write;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use tempfile::tempdir;

mod common;

#[tokio::test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
async fn test_retry_after_timeout() {
    let timeout = 200;
    let base_delay_ms = 300;
    let step_delay_ms = 100;
    let attempts = 10;
    let config_file_path = get_config_file(timeout, base_delay_ms, step_delay_ms);

    let dir = tempdir().unwrap().into_path();
    let file_path = dir.join("test.log");
    let mut file = File::create(&file_path).expect("Couldn't create temp log file...");

    let attempts_counter = Arc::new(Mutex::new(0));
    let counter = attempts_counter.clone();
    let (server, received, shutdown_handle, address) = start_ingester(Box::new(move |body| {
        if body
            .lines
            .iter()
            .any(|l| l.file.as_deref().unwrap().contains("test.log"))
        {
            let mut counter = counter.lock().unwrap();
            *counter += 1;
            if *counter < attempts {
                // Sleep enough time to mark the request as timed out by the client
                thread::sleep(Duration::from_millis(timeout + 20));
            }
        }
    }));

    let mut settings = AgentSettings::with_mock_ingester(&dir.to_str().unwrap(), &address);
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
            base_delay_ms + (timeout + step_delay_ms) * attempts,
        ))
        .await;

        let map = received.lock().await;
        let file_info = map.get(file_path.to_str().unwrap()).unwrap();
        // Received it multiple times
        assert!(file_info.values.len() >= file_lines.len());

        let attempts_made = attempts_counter.lock().unwrap();
        // It retried multiple times
        assert_eq!(*attempts_made, attempts);
        shutdown_handle();
    });

    server_result.unwrap();
    agent_handle.kill().unwrap();
}

#[tokio::test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
async fn test_retry_is_not_made_before_retry_base_delay_ms() {
    // Use a large base delay
    let base_delay_ms = 300_000;
    let timeout = 200;
    let config_file_path = get_config_file(timeout, base_delay_ms, 100);

    let dir = tempdir().unwrap().into_path();
    let file_path = dir.join("test.log");
    let mut file = File::create(&file_path).expect("Couldn't create temp log file...");

    let attempts_counter = Arc::new(Mutex::new(0));
    let counter = attempts_counter.clone();
    let (server, _, shutdown_handle, address) = start_ingester(Box::new(move |body| {
        if body
            .lines
            .iter()
            .any(|l| l.file.as_deref().unwrap().contains("test.log"))
        {
            let mut counter = counter.lock().unwrap();
            *counter += 1;
            // Sleep enough time to mark the request as timed out by the client
            thread::sleep(Duration::from_millis(timeout + 20));
        }
    }));

    let mut settings = AgentSettings::with_mock_ingester(&dir.to_str().unwrap(), &address);
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
        let attempts_made = attempts_counter.lock().unwrap();
        // It was not retried
        assert_eq!(*attempts_made, 1);
        shutdown_handle();
    });

    server_result.unwrap();
    agent_handle.kill().unwrap();
}

/// Creates a temp config file with required fields and the provided parameters
fn get_config_file(timeout: u64, retry_base_delay_ms: u64, retry_step_delay_ms: u64) -> PathBuf {
    let config_dir = tempdir().unwrap().into_path();
    let config_file_path = config_dir.join("config.yaml");
    let mut config_file = File::create(&config_file_path).unwrap();

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
        timeout, retry_base_delay_ms, retry_step_delay_ms
    )
    .unwrap();

    config_file_path
}

fn start_ingester(
    process_fn: Box<dyn Fn(&IngestBody) + Send>,
) -> (
    impl Future<Output = std::result::Result<(), IngestError>>,
    FileLineCounter,
    impl FnOnce(),
    String,
) {
    let port = common::get_available_port().expect("No ports free");
    let address = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), port);

    let (server, received, shutdown_handle) = http_ingester_with_processors(address, process_fn);
    (
        server,
        received,
        shutdown_handle,
        format!("localhost:{}", port),
    )
}

use crate::common::{self, AgentSettings};
pub use common::*;
use std::fs::File;
use std::io::Write;
use std::sync::{Arc, Mutex};
use tempfile::tempdir;

use test_log::test;

#[test(tokio::test)]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
async fn test_http_buffer_size() {
    let dir = tempdir().unwrap().into_path();
    let file_path = dir.join("test.log");
    let mut file = File::create(&file_path).expect("Couldn't create temp log file...");

    let call_counter = Arc::new(Mutex::new(0));
    let counter = call_counter.clone();
    let (mock_server, _, shutdown_handle, address) = common::start_ingester(
        Box::new(|_| None),
        Box::new(move |body| {
            // The agent picks up some of its own log messages and sends them to the mock ingest service
            // but this test only wants to count messages generated by the test case to make an accurate
            // assertion about the behavior of the buffer.
            if body
                .lines
                .iter()
                .any(|line| line.line.as_ref().unwrap().contains("abcdefghijklmnop"))
            {
                let mut counter = counter.lock().unwrap();
                *counter += 1;
            };
            None
        }),
    );

    let mut settings = AgentSettings::with_mock_ingester(dir.to_str().unwrap(), &address);
    settings.ingest_buffer_size = Some("100"); // encourage buffer flushes every 2 test log lines

    let mut agent_handle = common::spawn_agent(settings);
    let mut agent_stderr = std::io::BufReader::new(agent_handle.stderr.take().unwrap());

    let (server_result, _) = tokio::join!(mock_server, async move {
        common::wait_for_event("Enabling filesystem", &mut agent_stderr);
        common::consume_output(agent_stderr.into_inner());
        // generate some log data
        for _ in 0..10 {
            writeln!(file, "abcdefghijklmnop").unwrap();
        }

        // Wait for the data to be received by the mock ingester
        tokio::time::sleep(tokio::time::Duration::from_millis(2500)).await;

        // Ensure that the agent sent each log message individually because each log line
        // filled the configured buffer capacity.
        let calls_made = call_counter.lock().unwrap();
        assert!(*calls_made >= 5, "{:#?}", *calls_made);

        agent_handle.kill().unwrap();
        shutdown_handle();
    });

    server_result.unwrap();
}

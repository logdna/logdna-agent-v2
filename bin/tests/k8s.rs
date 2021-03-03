use common::AgentSettings;
use std::fs::File;
use std::io::BufReader;
use tempfile::tempdir;

mod common;

// workaround for unused functions in different features: https://github.com/rust-lang/rust/issues/46379
pub use common::*;

#[tokio::test]
#[cfg_attr(not(feature = "k8s_tests"), ignore)]
async fn test_k8s_enrichment() {
    let dir = tempdir().expect("Couldn't create temp dir...").into_path();
    let (server, received, shutdown_handle, addr) = common::start_http_ingester();
    let file_path = dir.join("test.log");
    File::create(&file_path).expect("Couldn't create temp log file...");
    let settings = AgentSettings::with_mock_ingester(&dir.to_str().unwrap(), &addr);
    let mut agent_handle = common::spawn_agent(settings);
    let mut stderr_reader = BufReader::new(agent_handle.stderr.as_mut().unwrap());
    common::wait_for_file_event("initialized", &file_path, &mut stderr_reader);

    let (server_result, _) = tokio::join!(server, async {
        common::append_to_file(&file_path, 10, 5).unwrap();
        common::force_client_to_flush(&dir).await;

        // Wait for the data to be received by the mock ingester
        tokio::time::delay_for(tokio::time::Duration::from_millis(2000)).await;

        let map = received.lock().await;

        let result = map.iter().find(|(k, _)| k.contains("sample-pod"));
        assert!(result.is_some());
        let (_, pod_file_info) = result.unwrap();
        let label = pod_file_info.label.as_ref();
        assert!(label.is_some());
        assert_eq!(label.unwrap()["app.kubernetes.io/name"], "sample-pod");
        assert_eq!(
            label.unwrap()["app.kubernetes.io/instance"],
            "sample-pod-instance"
        );

        let result = map.iter().find(|(k, _)| k.contains("sample-job"));
        assert!(result.is_some());
        let (_, job_file_info) = result.unwrap();
        let label = job_file_info.label.as_ref();
        assert!(label.is_some());
        assert_eq!(label.unwrap()["job-name"], "sample-job");

        shutdown_handle();
    });

    server_result.unwrap();
    agent_handle.kill().expect("Could not kill process");
}

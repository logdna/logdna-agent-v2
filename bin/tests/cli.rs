use std::fs;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::MetadataExt;
use std::process::Command;
use std::thread::{self, sleep};
use std::time::Duration;
use wait_timeout::ChildExt;

use crate::common::{consume_output, AgentSettings};

use assert_cmd::prelude::*;
use futures::FutureExt;
use hyper::{Client, StatusCode};
use log::debug;
use predicates::prelude::*;
use proptest::prelude::*;
use systemd::journal;
use tempfile::tempdir;
use test_types::random_line_string_vec;
use tokio::task;

mod common;

#[test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
fn api_key_missing() {
    let mut cmd = Command::cargo_bin("logdna-agent").unwrap();
    cmd.env_clear()
        .env("RUST_LOG", "debug")
        .assert()
        .stderr(predicate::str::contains(
            "config error: http.ingestion_key is missing",
        ))
        .failure();
}

#[test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
fn api_key_present() {
    let _ = env_logger::Builder::from_default_env().try_init();
    let dir = tempdir().expect("Couldn't create temp dir...");

    let dir_path = format!("{}/", dir.path().to_str().unwrap());

    let before_file_path = dir.path().join("before.log");
    let mut file = File::create(&before_file_path).expect("Couldn't create temp log file...");

    let mut handle = common::spawn_agent(AgentSettings {
        exclusion_regex: Some(r"/var\w*"),
        log_dirs: &dir_path,
        lookback: Some("start"),
        ..Default::default()
    });
    // Dump the agent's stdout
    // TODO: assert that it's successfully uploaded

    thread::sleep(std::time::Duration::from_secs(1));

    let log_lines = "This is a test log line\nLook at me, another test log line\nMore log lines....\nAnother log line!";

    writeln!(file, "{}", log_lines).expect("Couldn't write to temp log file...");
    file.sync_all().unwrap();
    thread::sleep(std::time::Duration::from_secs(1));

    let test_file_path = dir.path().join("test.log");
    let mut file = File::create(&test_file_path).expect("Couldn't create temp log file...");

    writeln!(file, "{}", log_lines).expect("Couldn't write to temp log file...");
    file.sync_all().unwrap();
    thread::sleep(std::time::Duration::from_secs(1));

    let test1_file_path = dir.path().join("test1.log");
    let mut file = File::create(&test1_file_path).expect("Couldn't create temp log file...");

    writeln!(file, "{}", log_lines).expect("Couldn't write to temp log file...");
    file.sync_all().unwrap();
    thread::sleep(std::time::Duration::from_secs(1));

    handle.kill().unwrap();
    let mut output = String::new();

    let stderr_ref = handle.stderr.as_mut().unwrap();
    stderr_ref.read_to_string(&mut output).unwrap();

    debug!("{}", output);

    // Check that the agent logs that it has sent lines from each file
    assert!(
        predicate::str::contains(&format!(
            "watching \"{}\"",
            before_file_path.to_str().unwrap()
        ))
        .eval(&output),
        "'watching' not found in output: {}",
        output
    );
    assert!(predicate::str::contains(&format!(
        "tailer sendings lines for [\"{}\"]",
        before_file_path.to_str().unwrap()
    ))
    .eval(&output));

    assert!(predicate::str::contains(&format!(
        "watching \"{}\"",
        test_file_path.to_str().unwrap()
    ))
    .eval(&output));
    assert!(predicate::str::contains(&format!(
        "tailer sendings lines for [\"{}\"]",
        test_file_path.to_str().unwrap()
    ))
    .eval(&output));

    assert!(predicate::str::contains(&format!(
        "watching \"{}\"",
        test1_file_path.to_str().unwrap()
    ))
    .eval(&output));
    assert!(predicate::str::contains(&format!(
        "tailer sendings lines for [\"{}\"]",
        test1_file_path.to_str().unwrap()
    ))
    .eval(&output));

    handle.wait().unwrap();
}

#[test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
fn test_read_file_appended_in_the_background() {
    let dir = tempdir().expect("Could not create temp dir").into_path();
    let mut agent_handle = common::spawn_agent(AgentSettings::new(dir.to_str().unwrap()));

    let context = common::start_append_to_file(&dir, 5);

    let mut stderr_reader = BufReader::new(agent_handle.stderr.as_mut().unwrap());
    let mut line = String::new();
    let mut occurrences = 0;
    let expected_occurrences = 100;

    for _safeguard in 0..100_000 {
        stderr_reader.read_line(&mut line).unwrap();
        if line.contains("sendings lines for") && line.contains("appended.log") {
            occurrences += 1;
        }
        line.clear();

        if occurrences == expected_occurrences {
            break;
        }
    }

    let total_lines_written = (context.stop_handle)();
    assert!(total_lines_written > 0);
    assert_eq!(occurrences, expected_occurrences);
    agent_handle.kill().unwrap();
}

#[test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
fn test_append_and_delete() {
    let dir = tempdir().expect("Could not create temp dir").into_path();
    let file_path = dir.join("file1.log");
    File::create(&file_path).expect("Could not create file");

    let mut agent_handle = common::spawn_agent(AgentSettings::new(dir.to_str().unwrap()));

    let mut stderr_reader = BufReader::new(agent_handle.stderr.take().unwrap());

    common::wait_for_file_event("initialized", &file_path, &mut stderr_reader);
    common::append_to_file(&file_path, 10_000, 50).expect("Could not append");
    fs::remove_file(&file_path).expect("Could not remove file");

    // Immediately, start appending in a new file
    common::append_to_file(&file_path, 5, 5).expect("Could not append");

    common::wait_for_file_event("unwatching", &file_path, &mut stderr_reader);
    common::wait_for_file_event("added", &file_path, &mut stderr_reader);

    common::assert_agent_running(&mut agent_handle);

    agent_handle.kill().expect("Could not kill process");
}

#[test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
fn test_file_added_after_initialization() {
    let dir = tempdir().expect("Could not create temp dir").into_path();

    let mut agent_handle = common::spawn_agent(AgentSettings::new(dir.to_str().unwrap()));

    let mut reader = BufReader::new(agent_handle.stderr.as_mut().unwrap());

    thread::sleep(std::time::Duration::from_millis(2000));

    let file_path = dir.join("file1.log");
    File::create(&file_path).expect("Could not create file");
    common::wait_for_file_event("added", &file_path, &mut reader);
    common::append_to_file(&file_path, 1000, 50).expect("Could not append");
    common::wait_for_file_event("tailer sendings lines for", &file_path, &mut reader);

    common::assert_agent_running(&mut agent_handle);

    agent_handle.kill().expect("Could not kill process");
}

#[test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
fn test_delete_does_not_leave_file_descriptor() {
    let dir = tempdir().expect("Could not create temp dir").into_path();
    let file_path = dir.join("file1.log");
    File::create(&file_path).expect("Could not create file");

    let mut agent_handle = common::spawn_agent(AgentSettings::new(dir.to_str().unwrap()));

    let process_id = agent_handle.id();
    let mut stderr_reader = BufReader::new(agent_handle.stderr.as_mut().unwrap());

    common::wait_for_file_event("initialized", &file_path, &mut stderr_reader);
    common::append_to_file(&file_path, 100, 50).expect("Could not append");

    // Verify that the file is shown in the open files of the process
    assert!(common::open_files_include(process_id, &file_path).is_some());

    // Remove it
    fs::remove_file(&file_path).expect("Could not remove file");
    common::wait_for_file_event("unwatching", &file_path, &mut stderr_reader);

    common::assert_agent_running(&mut agent_handle);

    // Wait for the file descriptor to be released
    thread::sleep(Duration::from_millis(200));

    // Verify that it doesn't appear any more, otherwise include it in the assert panic message
    assert_eq!(
        common::open_files_include(process_id, &file_path),
        None,
        "the file should not appear in the output"
    );

    agent_handle.kill().expect("Could not kill process");
}

#[test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
#[cfg(unix)]
fn test_send_sigterm_does_not_leave_file_descriptor() {
    // k8s uses SIGTERM
    test_signals(nix::sys::signal::Signal::SIGTERM);
}

#[test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
#[cfg(unix)]
fn test_send_sigint_does_not_leave_file_descriptor() {
    // k8s uses SIGTERM
    test_signals(nix::sys::signal::Signal::SIGINT);
}

#[cfg(unix)]
fn test_signals(signal: nix::sys::signal::Signal) {
    let dir = tempdir().expect("Could not create temp dir").into_path();
    let file_path = dir.join("file1.log");
    File::create(&file_path).expect("Could not create file");

    let mut agent_handle = common::spawn_agent(AgentSettings::new(dir.to_str().unwrap()));

    let process_id = agent_handle.id();
    let mut stderr_reader = BufReader::new(agent_handle.stderr.as_mut().unwrap());

    common::wait_for_file_event("initialized", &file_path, &mut stderr_reader);
    common::append_to_file(&file_path, 100, 50).expect("Could not append");

    // Verify that the file is shown in the open files
    assert!(common::is_file_open(&file_path));

    nix::sys::signal::kill(nix::unistd::Pid::from_raw(process_id as i32), signal).unwrap();

    // Verify that it should exit with 0
    assert_eq!(
        agent_handle
            .wait_timeout(Duration::from_secs(1))
            .unwrap()
            .map(|e| e.success()),
        Some(true)
    );

    //Verify that file descriptor doesn't appear any more
    assert!(!common::is_file_open(&file_path));
}

#[test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
fn test_append_and_move() {
    let dir = tempdir().expect("Could not create temp dir").into_path();
    let file1_path = dir.join("file1.log");
    let file2_path = dir.join("file2.log");
    File::create(&file1_path).expect("Could not create file");

    let mut agent_handle = common::spawn_agent(AgentSettings::new(dir.to_str().unwrap()));
    let mut stderr_reader = BufReader::new(agent_handle.stderr.as_mut().unwrap());

    common::wait_for_file_event("initialized", &file1_path, &mut stderr_reader);
    common::append_to_file(&file1_path, 10_000, 50).expect("Could not append");
    fs::rename(&file1_path, &file2_path).expect("Could not move file");
    fs::remove_file(&file2_path).expect("Could not remove file");

    // Immediately, start appending in a new file
    common::append_to_file(&file1_path, 5, 5).expect("Could not append");

    // Should be added back
    common::wait_for_file_event("added", &file1_path, &mut stderr_reader);

    common::assert_agent_running(&mut agent_handle);

    agent_handle.kill().expect("Could not kill process");
}

#[test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
fn test_truncate_file() {
    // K8s uses copytruncate, see https://github.com/kubernetes/kubernetes/issues/38495
    let dir = tempdir().expect("Could not create temp dir").into_path();
    let file_path = dir.join("file1.log");
    common::append_to_file(&file_path, 100, 50).expect("Could not append");

    let mut agent_handle = common::spawn_agent(AgentSettings::new(dir.to_str().unwrap()));

    let mut stderr_reader = BufReader::new(agent_handle.stderr.as_mut().unwrap());

    common::wait_for_file_event("initialized", &file_path, &mut stderr_reader);
    common::append_to_file(&file_path, 10_000, 20).expect("Could not append");
    common::truncate_file(&file_path).expect("Could not truncate file");

    // Immediately, start appending to the truncated file
    common::append_to_file(&file_path, 5, 5).expect("Could not append");
    common::wait_for_file_event("truncated", &file_path, &mut stderr_reader);

    // Continue appending
    common::append_to_file(&file_path, 100, 5).expect("Could not append");
    common::wait_for_file_event("tailer sendings lines", &file_path, &mut stderr_reader);

    common::assert_agent_running(&mut agent_handle);

    agent_handle.kill().expect("Could not kill process");
}

#[test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
fn test_exclusion_rules() {
    let dir = tempdir().expect("Could not create temp dir").into_path();
    let included_file = dir.join("file1.log");
    let excluded_file = dir.join("file2.log");
    common::append_to_file(&included_file, 100, 50).expect("Could not append");
    common::append_to_file(&excluded_file, 100, 50).expect("Could not append");

    let mut agent_handle = common::spawn_agent(AgentSettings {
        log_dirs: dir.to_str().unwrap(),
        exclusion_regex: Some(r"\w+ile2\.\w{3}"),
        ..Default::default()
    });

    let mut stderr_reader = BufReader::new(agent_handle.stderr.as_mut().unwrap());

    let lines = common::wait_for_file_event("initialized", &included_file, &mut stderr_reader);

    let matches_excluded_file = predicate::str::is_match(r"initialized [^\n]*file2\.log").unwrap();
    assert!(
        !matches_excluded_file.eval(&lines),
        "file2.log should have been excluded"
    );

    // Continue appending
    common::append_to_file(&included_file, 100, 5).expect("Could not append");
    let lines =
        common::wait_for_file_event("tailer sendings lines", &included_file, &mut stderr_reader);
    assert!(
        !matches_excluded_file.eval(&lines),
        "file2.log should have been excluded"
    );

    common::assert_agent_running(&mut agent_handle);

    agent_handle.kill().expect("Could not kill process");
}

#[tokio::test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
async fn test_metrics_endpoint() {
    let dir = tempdir().expect("Could not create temp dir").into_path();
    let included_file = dir.join("file1.log");
    let port = 9881;

    let mut agent_handle = common::spawn_agent(AgentSettings {
        log_dirs: dir.to_str().unwrap(),
        metrics_port: Some(port),
        ..Default::default()
    });

    let mut stderr_reader = BufReader::new(agent_handle.stderr.take().unwrap());
    common::wait_for_event("Enabling prometheus endpoint", &mut stderr_reader);

    // Append to a file and wait for the notify event
    common::append_to_file(&included_file, 100, 5).unwrap();
    common::wait_for_file_event("tailer sendings lines", &included_file, &mut stderr_reader);
    let mut body_str = String::new();

    // Wait for all the metrics to be tracked
    for _ in 0..20 {
        let client = Client::new();
        let url = format!("http://127.0.0.1:{}/metrics", port)
            .parse()
            .unwrap();
        if let Ok(response) = client.get(url).await {
            assert_eq!(response.status(), StatusCode::OK);

            let buf = hyper::body::to_bytes(response).await.unwrap();
            body_str = std::str::from_utf8(&buf).unwrap().to_string();

            // Request duration metrics are the last ones to appear
            if body_str.contains("logdna_agent_ingest_request_duration") {
                break;
            }
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    assert!(body_str.contains("# TYPE logdna_agent_fs_bytes counter"));
    assert!(body_str.contains("# TYPE logdna_agent_fs_lines counter"));
    assert!(body_str.contains("# TYPE logdna_agent_ingest_request_size histogram"));
    assert!(body_str.contains("# TYPE logdna_agent_ingest_request_duration_millis histogram"));
    assert!(body_str.contains("# TYPE logdna_agent_fs_events counter"));
    // One created file
    assert!(body_str.contains("logdna_agent_fs_events{event_type=\"create\"} 1"));

    common::assert_agent_running(&mut agent_handle);
    agent_handle.kill().expect("Could not kill process");
}

#[test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
fn test_include_only_rules() {
    let dir = tempdir().unwrap().into_path();
    let included = dir.join("my_app.log");
    let excluded1 = dir.join("other_file.log");
    let excluded2 = dir.join("another_file.log");
    common::append_to_file(&included, 100, 50).expect("Could not append");
    common::append_to_file(&excluded1, 100, 50).expect("Could not append");
    common::append_to_file(&excluded2, 100, 50).expect("Could not append");

    let mut agent_handle = common::spawn_agent(AgentSettings {
        log_dirs: dir.to_str().unwrap(),
        exclusion: Some("!(*my_app*)"),
        ..Default::default()
    });

    let mut stderr_reader = BufReader::new(agent_handle.stderr.as_mut().unwrap());
    let lines = common::wait_for_file_event("initialized", &included, &mut stderr_reader);

    let matches_excluded1 = predicate::str::is_match(format!(
        r"initialized [^\n]*{}",
        excluded1.file_stem().unwrap().to_str().unwrap()
    ))
    .unwrap();
    let matches_excluded2 = predicate::str::is_match(format!(
        r"initialized [^\n]*{}",
        excluded2.file_stem().unwrap().to_str().unwrap()
    ))
    .unwrap();
    assert!(!matches_excluded1.eval(&lines));
    assert!(!matches_excluded2.eval(&lines));

    // Continue appending
    common::append_to_file(&included, 100, 5).expect("Could not append");
    common::append_to_file(&excluded1, 100, 5).expect("Could not append");
    let lines = common::wait_for_file_event("tailer sendings lines", &included, &mut stderr_reader);
    assert!(!matches_excluded1.eval(&lines));
    assert!(!matches_excluded2.eval(&lines));

    common::assert_agent_running(&mut agent_handle);
    agent_handle.kill().expect("Could not kill process");
}

#[test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
fn test_files_other_than_dot_log_should_be_not_included_by_default() {
    let dir = tempdir().expect("Could not create temp dir").into_path();
    let included_file = dir.join("file1.log");
    let not_included_files = vec![
        "file2.tar.gz",
        "file3.tar",
        "file4.0",
        "file5.1",
        ".file6",
        "file7",
    ];
    common::append_to_file(&included_file, 100, 50).expect("Could not append");

    for file_name in &not_included_files {
        common::append_to_file(&dir.join(file_name), 100, 50).expect("Could not append");
    }

    let mut agent_handle = common::spawn_agent(AgentSettings::new(dir.to_str().unwrap()));

    let mut reader = BufReader::new(agent_handle.stderr.as_mut().unwrap());
    let lines = common::wait_for_file_event("initialized", &included_file, &mut reader);

    for file_name in &not_included_files {
        let file_parts: Vec<&str> = file_name.split('.').collect();
        let regex;
        if file_parts.len() == 2 {
            regex = format!(
                "{}{}\\.{}",
                r"initialized [^\n]*", file_parts[0], file_parts[1]
            );
        } else {
            regex = format!("{}{}", r"initialized [^\n]*", file_name);
        }
        let matches_excluded_file = predicate::str::is_match(regex).unwrap();
        assert!(
            !matches_excluded_file.eval(&lines),
            "{} should not been included",
            file_name
        );
    }

    common::assert_agent_running(&mut agent_handle);
    agent_handle.kill().expect("Could not kill process");
}

#[test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
#[cfg_attr(not(target_os = "linux"), ignore)]
fn test_dangling_symlinks() {
    let log_dir = tempdir().expect("Could not create temp dir").into_path();
    let data_dir = tempdir().expect("Could not create temp dir").into_path();
    let file_path = data_dir.join("file1.log");
    let symlink_path = log_dir.join("file1.log");
    common::append_to_file(&file_path, 100, 50).expect("Could not append");

    let mut agent_handle = common::spawn_agent(AgentSettings::new(log_dir.to_str().unwrap()));

    let mut stderr_reader = BufReader::new(agent_handle.stderr.as_mut().unwrap());

    std::os::unix::fs::symlink(&file_path, &symlink_path).unwrap();
    common::wait_for_file_event("initialized", &file_path, &mut stderr_reader);
    common::append_to_file(&file_path, 100, 20).expect("Could not append");

    // Remove original file first
    fs::remove_file(&file_path).expect("Could not remove file");

    common::wait_for_file_event("unwatching", &file_path, &mut stderr_reader);

    // Remove the dangling symlink also
    fs::remove_file(&symlink_path).expect("Could not remove symlink");
    common::wait_for_file_event("unwatching", &file_path, &mut stderr_reader);

    common::assert_agent_running(&mut agent_handle);
    agent_handle.kill().expect("Could not kill process");
}

#[test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
#[cfg_attr(not(target_os = "linux"), ignore)]
fn test_append_after_symlinks_delete() {
    let log_dir = tempdir().expect("Could not create temp dir").into_path();
    let data_dir = tempdir().expect("Could not create temp dir").into_path();
    let file_path = data_dir.join("file1.log");
    let symlink_path = log_dir.join("file1.log");
    common::append_to_file(&file_path, 100, 50).expect("Could not append");

    let mut agent_handle = common::spawn_agent(AgentSettings::new(log_dir.to_str().unwrap()));
    let mut stderr_reader = BufReader::new(agent_handle.stderr.as_mut().unwrap());

    std::os::unix::fs::symlink(&file_path, &symlink_path).unwrap();
    common::wait_for_file_event("initialized", &file_path, &mut stderr_reader);
    common::append_to_file(&file_path, 100, 20).expect("Could not append");

    // Remove symlink first
    fs::remove_file(&symlink_path).expect("Could not remove symlink");

    // Append to the original file
    common::append_to_file(&file_path, 1_000, 20).expect("Could not append");

    // Delete the original file
    fs::remove_file(&file_path).expect("Could not remove file");

    common::assert_agent_running(&mut agent_handle);
    agent_handle.kill().expect("Could not kill process");
}

#[test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
#[cfg_attr(not(target_os = "linux"), ignore)]
fn test_directory_symlinks_delete() {
    let _ = env_logger::Builder::from_default_env().try_init();
    let log_dir = tempdir().expect("Could not create temp dir").into_path();
    let data_dir = tempdir().expect("Could not create temp dir").into_path();

    let dir_1_path = data_dir.join("dir_1");
    let dir_1_1_path = dir_1_path.join("dir_1_1");
    let dir_1_2_path = dir_1_path.join("dir_1_2");
    let dir_1_2_1_path = dir_1_2_path.join("dir_1_2_1");
    let file1_path = dir_1_1_path.join("file1.log");
    let file2_path = dir_1_2_1_path.join("file2.log");
    let file3_path = dir_1_2_1_path.join("file3.log");
    let symlink_path = log_dir.join("dir_1_link");

    common::create_dirs(&[&dir_1_path, &dir_1_1_path, &dir_1_2_path, &dir_1_2_1_path]);

    common::append_to_file(&file1_path, 100, 50).expect("Could not append");
    common::append_to_file(&file2_path, 100, 50).expect("Could not append");
    common::append_to_file(&file3_path, 100, 50).expect("Could not append");

    let mut agent_handle = common::spawn_agent(AgentSettings::new(log_dir.to_str().unwrap()));

    let mut stderr_reader = BufReader::new(agent_handle.stderr.as_mut().unwrap());

    std::os::unix::fs::symlink(&dir_1_path, &symlink_path).unwrap();

    debug!("waiting for initialized");
    common::wait_for_file_event("watching", &symlink_path, &mut stderr_reader);

    common::append_to_file(&file1_path, 1_000, 50).expect("Could not append");
    common::append_to_file(&file2_path, 1_000, 50).expect("Could not append");
    common::append_to_file(&file3_path, 1_000, 50).expect("Could not append");

    fs::remove_file(&symlink_path).expect("Could not remove symlink");

    debug!("waiting for unwatching");
    common::wait_for_file_event("unwatching", &file3_path, &mut stderr_reader);

    common::assert_agent_running(&mut agent_handle);
    agent_handle.kill().expect("Could not kill process");
}

#[tokio::test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
#[cfg_attr(not(target_os = "linux"), ignore)]
async fn test_journald_support() {
    assert_eq!(journal::print(6, "Sample info"), 0);
    sleep(Duration::from_millis(1000));
    let dir = "/var/log/journal";
    let (server, received, shutdown_handle, addr) = common::start_http_ingester();
    let mut settings = AgentSettings::with_mock_ingester("/var/log/journal", &addr);
    settings.journald_dirs = Some(dir);
    settings.exclusion_regex = Some(r"^(?!/var/log/journal).*$");
    let mut agent_handle = common::spawn_agent(settings);
    let mut stderr_reader = BufReader::new(agent_handle.stderr.as_mut().unwrap());

    common::wait_for_event("monitoring journald path", &mut stderr_reader);

    let (server_result, _) = tokio::join!(server, async {
        for _ in 0..10 {
            journal::print(1, "Sample alert");
            journal::print(6, "Sample info");
        }

        // Wait for the data to be received by the mock ingester
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        let map = received.lock().await;
        let file_info = map.values().next().unwrap();

        let predicate_fn = predicate::in_iter(file_info.values.iter().map(|s| s.trim_end()));
        assert!(predicate_fn.eval(&"Sample alert"));
        assert!(predicate_fn.eval(&"Sample info"));

        shutdown_handle();
    });

    server_result.unwrap();
    common::assert_agent_running(&mut agent_handle);
    agent_handle.kill().expect("Could not kill process");
}

#[tokio::test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
#[cfg_attr(not(target_os = "linux"), ignore)]
async fn test_journalctl_support() {
    assert_eq!(journal::print(6, "Sample info"), 0);
    sleep(Duration::from_millis(1000));
    let (server, received, shutdown_handle, addr) = common::start_http_ingester();
    let mut settings = AgentSettings::with_mock_ingester("/var/log/journal", &addr);
    settings.journald_dirs = None;
    settings.exclusion_regex = Some(r"^(?!/var/log/journal).*$");
    let mut agent_handle = common::spawn_agent(settings);
    let mut stderr_reader = BufReader::new(agent_handle.stderr.as_mut().unwrap());

    common::wait_for_event("Listening to journalctl", &mut stderr_reader);

    let (server_result, _) = tokio::join!(server, async {
        for _ in 0..10 {
            journal::print(1, "Sample alert");
            journal::print(6, "Sample info");
        }

        // Wait for the data to be received by the mock ingester
        tokio::time::sleep(tokio::time::Duration::from_millis(5000)).await;

        let map = received.lock().await;
        let file_info = map.values().next().unwrap();

        let predicate_fn = predicate::in_iter(file_info.values.iter().map(|s| s.trim_end()));
        assert!(predicate_fn.eval(&"Sample alert"));
        assert!(predicate_fn.eval(&"Sample info"));

        shutdown_handle();
    });

    server_result.unwrap();
    common::assert_agent_running(&mut agent_handle);
    agent_handle.kill().expect("Could not kill process");
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1))]
    #[test]
    #[cfg_attr(not(feature = "integration_tests"), ignore)]
    fn lookback_start_lines_are_delivered(log_lines in random_line_string_vec(150, 2000)) {
        let _ = env_logger::Builder::from_default_env().try_init();
        let dir = tempdir().expect("Couldn't create temp dir...");

        let dir_path = format!("{}/", dir.path().to_str().unwrap());
        let (server, received, shutdown_handle, cert_file, addr) = common::self_signed_https_ingester();

        let file_path = dir.path().join("test.log");
        let mut file = File::create(&file_path).expect("Couldn't create temp log file...");

        // Enough bytes to get past the lookback threshold
        let lines_write_count: usize = log_lines.iter()
            .scan(0, |acc, line| {*acc += line.len(); Some(*acc)})
            .enumerate()
            .take_while(|(_, cnt)| *cnt < 8192).last().unwrap().0;

        debug!("{}", lines_write_count);
        log_lines[0..lines_write_count].iter()
            .for_each(|log_line| writeln!(file, "{}", log_line).expect("Couldn't write to temp log file..."));
        file.sync_all().expect("Failed to sync file");

        // Dump the agent's stdout
        // TODO: assert that it's successfully uploaded

        thread::sleep(std::time::Duration::from_secs(1));

        tokio_test::block_on(async {
            let ((line_count, lines), _, server) = tokio::join!(
                async {
                    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
                    let mut handle = common::spawn_agent(AgentSettings {
                        log_dirs: &dir_path,
                        exclusion_regex: Some(r"/var\w*"),
                        ssl_cert_file: Some(cert_file.path()),
                        lookback: Some("start"),
                        host: Some(&addr),
                        ..Default::default()
                    });

                    let stderr_reader = std::io::BufReader::new(handle.stderr.take().unwrap());
                    std::thread::spawn(move || {
                        stderr_reader.lines().for_each(|line| debug!("{:?}", line))
                    });

                    tokio::time::sleep(tokio::time::Duration::from_millis(5000)).await;

                    handle.kill().unwrap();

                    debug!("getting lines from {}", file_path.to_str().unwrap());
                    let file_info = received.lock().await;
                    let file_info = file_info
                        .get(file_path.to_str().unwrap())
                        .unwrap();
                    let line_count = file_info.lines;
                    let lines = file_info.values.clone();
                    shutdown_handle();

                    handle.wait().unwrap();
                    (line_count, lines)
                },
                async {
                    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                    log_lines[lines_write_count..lines_write_count + 5].iter().for_each(|log_line| {
                        writeln!(file, "{}", log_line).expect("Couldn't write to temp log file...");
                        file.sync_all().expect("Failed to sync file");
                    });
                    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                    file.sync_all().expect("Failed to sync file");
                },
                server
            );
            server.unwrap();
            assert_eq!(line_count, lines_write_count + 5);

            lines[..].iter().map(|s| s.trim_end()).zip(
                log_lines[0..lines_write_count + 5].iter().map(|s| s.trim_end())).for_each(|(a, b)| debug!("received: {}, sent: {}", a, b));
            itertools::assert_equal(
                lines[..].iter().map(|s| s.trim_end()),
                log_lines[0..lines_write_count + 5].iter().map(|s| s.trim_end())
            )
        });
    }
}

#[test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
fn lookback_none_lines_are_delivered() {
    let _ = env_logger::Builder::from_default_env().try_init();

    let dir = tempdir().expect("Couldn't create temp dir...");
    let dir_path = format!("{}/", dir.path().to_str().unwrap());

    let (server, received, shutdown_handle, cert_file, addr) = common::self_signed_https_ingester();
    let log_lines = "This is a test log line";

    let file_path = dir.path().join("test.log");
    let mut file = File::create(&file_path).expect("Couldn't create temp log file...");

    debug!("test log: {}", file_path.to_str().unwrap());
    // Enough bytes to get past the lookback threshold
    let line_write_count = (8192 / (log_lines.as_bytes().len() + 1)) + 1;
    (0..line_write_count)
        .for_each(|_| writeln!(file, "{}", log_lines).expect("Couldn't write to temp log file..."));

    debug!(
        "wrote {} lines to {} with size {}",
        line_write_count,
        file_path.to_str().unwrap(),
        (log_lines.as_bytes().len() + 1) * line_write_count
    );
    file.sync_all().expect("Failed to sync file");

    // Dump the agent's stdout
    // TODO: assert that it's successfully uploaded

    tokio_test::block_on(async {
        let (line_count, _, server) = tokio::join!(
            async {
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                let mut handle = common::spawn_agent(AgentSettings {
                    log_dirs: &dir_path,
                    exclusion_regex: Some(r"^/var.*"),
                    ssl_cert_file: Some(cert_file.path()),
                    lookback: Some("none"),
                    host: Some(&addr),
                    ..Default::default()
                });
                debug!("spawned agent");

                let stderr_reader = std::io::BufReader::new(handle.stderr.take().unwrap());
                std::thread::spawn(move || {
                    stderr_reader.lines().for_each(|line| debug!("{:?}", line))
                });

                tokio::time::sleep(tokio::time::Duration::from_millis(3000)).await;

                handle.kill().unwrap();

                debug!("getting lines from {}", file_path.to_str().unwrap());
                handle.wait().unwrap();
                let line_count = received
                    .lock()
                    .await
                    .get(file_path.to_str().unwrap())
                    .unwrap()
                    .lines;
                shutdown_handle();
                line_count
            },
            async move {
                tokio::time::sleep(tokio::time::Duration::from_millis(2000)).await;
                (0..5).for_each(|_| {
                    writeln!(file, "{}", log_lines).expect("Couldn't write to temp log file...");
                    file.sync_all().expect("Failed to sync file");
                });
                file.sync_all().expect("Failed to sync file");
                debug!("wrote 5 lines");
            },
            server
        );
        server.unwrap();
        assert_eq!(line_count, 5);
    });
}

#[tokio::test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
async fn test_partial_fsynced_lines() {
    let dir = tempdir().expect("Couldn't create temp dir...").into_path();
    let (server, received, shutdown_handle, addr) = common::start_http_ingester();
    let file_path = dir.join("test.log");
    File::create(&file_path).expect("Couldn't create temp log file...");
    let mut settings = AgentSettings::with_mock_ingester(dir.to_str().unwrap(), &addr);
    settings.exclusion_regex = Some(r"/var\w*");
    let mut agent_handle = common::spawn_agent(settings);
    let mut stderr_reader = BufReader::new(agent_handle.stderr.as_mut().unwrap());
    common::wait_for_file_event("initialized", &file_path, &mut stderr_reader);
    let (server_result, _) = tokio::join!(server, async {
        let mut file = OpenOptions::new()
            .append(true)
            .create(true)
            .open(&file_path)
            .unwrap();

        write!(file, "{}", "first part ").unwrap();
        write!(file, "{}", "second part").unwrap();

        file.sync_all().unwrap();
        common::force_client_to_flush(&dir).await;
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        {
            let map = received.lock().await;
            // The ingester should not have received any lines yet
            assert!(map.get(file_path.to_str().unwrap()).is_none());
        }

        write!(file, "{}", " third part\n").unwrap();
        write!(file, "{}", "begin second line").unwrap();

        file.sync_all().unwrap();
        common::force_client_to_flush(&dir).await;
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        {
            let map = received.lock().await;
            let file_info = map.get(file_path.to_str().unwrap()).unwrap();
            assert_eq!(
                file_info.values,
                vec!["first part second part third part\n".to_string()]
            );
        }

        shutdown_handle();
    });

    server_result.unwrap();
    agent_handle.kill().expect("Could not kill process");
}

#[tokio::test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
async fn test_tags() {
    let dir = tempdir().expect("Couldn't create temp dir...").into_path();

    let (server, received, shutdown_handle, addr) = common::start_http_ingester();
    let file_path = dir.join("test.log");
    let tag = "my-tag";
    File::create(&file_path).expect("Couldn't create temp log file...");
    let mut settings = AgentSettings::with_mock_ingester(dir.to_str().unwrap(), &addr);
    settings.tags = Some(tag);
    let mut agent_handle = common::spawn_agent(settings);
    let mut stderr_reader = BufReader::new(agent_handle.stderr.as_mut().unwrap());
    common::wait_for_file_event("initialized", &file_path, &mut stderr_reader);

    let (server_result, _) = tokio::join!(server, async {
        common::append_to_file(&file_path, 10, 5).unwrap();
        common::force_client_to_flush(&dir).await;

        // Wait for the data to be received by the mock ingester
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        let map = received.lock().await;
        let file_info = map.get(file_path.to_str().unwrap()).unwrap();
        assert_eq!(file_info.lines, 10);
        assert_eq!(file_info.values, vec![common::LINE.to_owned() + "\n"; 10]);
        assert_eq!(file_info.tags, Some(tag.to_string()));
        shutdown_handle();
    });

    server_result.unwrap();
    agent_handle.kill().expect("Could not kill process");
}

#[tokio::test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
async fn test_lookback_restarting_agent() {
    let _ = env_logger::Builder::from_default_env().try_init();
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    let line_count = Arc::new(AtomicUsize::new(0));

    let dir = tempdir().expect("Couldn't create temp dir...").into_path();
    let db_dir = tempdir().unwrap().into_path();
    let (server, received, shutdown_handle, addr) = common::start_http_ingester();

    let mut settings = AgentSettings::with_mock_ingester(dir.to_str().unwrap(), &addr);
    settings.state_db_dir = Some(&db_dir);
    settings.exclusion_regex = Some(r"/var\w*");
    settings.lookback = Some("smallfiles");

    let line_count_target = 5_000;

    let line_count_clone = line_count.clone();

    let (server_result, _) = tokio::join!(server, async {
        let file_path = dir.join("test.log");
        let mut file = OpenOptions::new()
            .append(true)
            .create(true)
            .open(&file_path)
            .unwrap();
        let writer_thread = std::thread::spawn(move || {
            for i in 0..line_count_target {
                writeln!(file, "Hello from line {}", i).unwrap();
                line_count_clone.fetch_add(1, Ordering::SeqCst);
                if i % 1000 == 0 {
                    file.sync_all().unwrap();
                }

                if i % 20 == 0 {
                    std::thread::sleep(core::time::Duration::from_millis(20));
                }
            }
        });

        debug!("Running first agent");
        let mut agent_handle = common::spawn_agent(settings.clone());
        let agent_stderr = agent_handle.stderr.take().unwrap();
        consume_output(agent_stderr);
        tokio::time::sleep(tokio::time::Duration::from_millis(2_000)).await;

        while line_count.load(Ordering::SeqCst) < line_count_target {
            tokio::time::sleep(tokio::time::Duration::from_millis(1_000)).await;
            agent_handle.kill().expect("Could not kill process");
            // Restart it back again
            debug!("Running next agent");
            agent_handle = common::spawn_agent(settings.clone());
            let agent_stderr = agent_handle.stderr.take().unwrap();
            consume_output(agent_stderr);
        }

        // Block til writing is definitely done

        debug!("Waiting a bit");
        task::spawn_blocking(move || writer_thread.join().unwrap())
            // Give the agent a chance to catch up
            .then(|_| tokio::time::sleep(tokio::time::Duration::from_millis(10000)))
            .await;

        // Sleep a bit more to give the agent a chance to process
        tokio::time::sleep(tokio::time::Duration::from_millis(5000)).await;

        let map = received.lock().await;
        assert!(map.len() > 0);
        let file_info = map.get(file_path.to_str().unwrap()).unwrap();

        assert!(file_info.values.len() > 100);
        debug!(
            "{}, {}",
            file_info.values.len(),
            line_count.load(Ordering::SeqCst)
        );
        assert!(file_info.values.len() >= line_count.load(Ordering::SeqCst));

        for i in 0..file_info.values.len() {
            assert_eq!(file_info.values[i], format!("Hello from line {}\n", i));
        }

        agent_handle.kill().expect("Could not kill process");
        shutdown_handle();
    });
    server_result.unwrap();
}

#[tokio::test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
#[cfg_attr(not(target_os = "linux"), ignore)]
async fn test_symlink_initialization_both_included() {
    let log_dir = tempdir().expect("Couldn't create temp dir...").into_path();
    let (server, received, shutdown_handle, addr) = common::start_http_ingester();
    let file_path = log_dir.join("test.log");
    let symlink_path = log_dir.join("test-symlink.log");
    File::create(&file_path).expect("Couldn't create temp log file...");
    let mut file = OpenOptions::new()
        .append(true)
        .create(true)
        .open(&file_path)
        .unwrap();
    for i in 0..10 {
        writeln!(file, "SAMPLE {}", i).unwrap();
    }
    file.sync_all().unwrap();
    std::os::unix::fs::symlink(&file_path, &symlink_path).unwrap();
    tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;
    let (server_result, _) = tokio::join!(server, async {
        let mut settings = AgentSettings::with_mock_ingester(log_dir.to_str().unwrap(), &addr);
        settings.lookback = Some("smallfiles");
        let mut agent_handle = common::spawn_agent(settings);
        let stderr_reader = agent_handle.stderr.take().unwrap();
        consume_output(stderr_reader);
        tokio::time::sleep(tokio::time::Duration::from_millis(2000)).await;
        for i in 10..20 {
            writeln!(file, "SAMPLE {}", i).unwrap();
        }
        file.sync_all().unwrap();
        common::force_client_to_flush(&log_dir).await;
        // Wait for the data to be received by the mock ingester
        tokio::time::sleep(tokio::time::Duration::from_millis(2000)).await;
        let map = received.lock().await;
        debug!("--TEST MAP KEYS: {:?}", map.keys());
        let file_info = map
            .get(file_path.to_str().unwrap())
            .expect("lines for file not found");
        for i in 0..20 {
            assert_eq!(file_info.values[i], format!("SAMPLE {}\n", i));
        }
        let file_info = map
            .get(symlink_path.to_str().unwrap())
            .expect("lines for symlink not found");
        for i in 0..20 {
            assert_eq!(file_info.values[i], format!("SAMPLE {}\n", i));
        }

        agent_handle.kill().expect("Could not kill process");
        shutdown_handle();
    });
    server_result.unwrap();
}

#[tokio::test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
#[cfg_attr(not(target_os = "linux"), ignore)]
async fn test_symlink_initialization_excluded_file() {
    let log_dir = tempdir().expect("Couldn't create temp dir...").into_path();
    let excluded_dir = tempdir().expect("Couldn't create temp dir...").into_path();
    let (server, received, shutdown_handle, addr) = common::start_http_ingester();
    let file_path = excluded_dir.join("test.log");
    let symlink_path = log_dir.join("test-symlink.log");
    File::create(&file_path).expect("Couldn't create temp log file...");
    let mut file = OpenOptions::new()
        .append(true)
        .create(true)
        .open(&file_path)
        .unwrap();
    for i in 0..10 {
        writeln!(file, "SAMPLE {}", i).unwrap();
    }
    file.sync_all().unwrap();
    std::os::unix::fs::symlink(&file_path, &symlink_path).unwrap();
    tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;
    let (server_result, _) = tokio::join!(server, async {
        let mut settings = AgentSettings::with_mock_ingester(log_dir.to_str().unwrap(), &addr);
        settings.lookback = Some("smallfiles");
        let mut agent_handle = common::spawn_agent(settings);
        let stderr_reader = agent_handle.stderr.take().unwrap();
        // Consume output
        consume_output(stderr_reader);
        tokio::time::sleep(tokio::time::Duration::from_millis(2000)).await;
        for i in 10..20 {
            writeln!(file, "SAMPLE {}", i).unwrap();
        }
        file.sync_all().unwrap();
        common::force_client_to_flush(&log_dir).await;
        // Wait for the data to be received by the mock ingester
        tokio::time::sleep(tokio::time::Duration::from_millis(2000)).await;
        let map = received.lock().await;
        let file_info = map
            .get(symlink_path.to_str().unwrap())
            .expect("symlink not found");
        for i in 0..20 {
            assert_eq!(file_info.values[i], format!("SAMPLE {}\n", i));
        }
        assert!(map.get(file_path.to_str().unwrap()).is_none());
        agent_handle.kill().expect("Could not kill process");
        shutdown_handle();
    });
    server_result.unwrap();
}

#[tokio::test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
#[cfg_attr(not(target_os = "linux"), ignore)]
async fn test_symlink_to_symlink_initialization_excluded_file() {
    let log_dir = tempdir().expect("Couldn't create temp dir...").into_path();
    let excluded_dir = tempdir().expect("Couldn't create temp dir...").into_path();
    let (server, received, shutdown_handle, addr) = common::start_http_ingester();
    let file_path = excluded_dir.join("test.log");
    let excluded_symlink_path = excluded_dir.join("test-symlink.log");
    let symlink_path = log_dir.join("test-symlink.log");
    File::create(&file_path).expect("Couldn't create temp log file...");
    let mut file = OpenOptions::new()
        .append(true)
        .create(true)
        .open(&file_path)
        .unwrap();
    for i in 0..10 {
        writeln!(file, "SAMPLE {}", i).unwrap();
    }
    file.sync_all().unwrap();
    std::os::unix::fs::symlink(&file_path, &excluded_symlink_path).unwrap();
    std::os::unix::fs::symlink(&excluded_symlink_path, &symlink_path).unwrap();
    tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;
    let (server_result, _) = tokio::join!(server, async {
        let mut settings = AgentSettings::with_mock_ingester(log_dir.to_str().unwrap(), &addr);
        settings.lookback = Some("smallfiles");
        let mut agent_handle = common::spawn_agent(settings);
        let stderr_reader = agent_handle.stderr.take().unwrap();
        // Consume output
        consume_output(stderr_reader);
        tokio::time::sleep(tokio::time::Duration::from_millis(2000)).await;
        for i in 10..20 {
            writeln!(file, "SAMPLE {}", i).unwrap();
        }
        file.sync_all().unwrap();
        common::force_client_to_flush(&log_dir).await;
        // Wait for the data to be received by the mock ingester
        tokio::time::sleep(tokio::time::Duration::from_millis(2000)).await;
        let map = received.lock().await;
        let file_info = map
            .get(symlink_path.to_str().unwrap())
            .expect("symlink not found");
        for i in 0..20 {
            assert_eq!(file_info.values[i], format!("SAMPLE {}\n", i));
        }
        agent_handle.kill().expect("Could not kill process");
        shutdown_handle();
    });
    server_result.unwrap();
}

#[tokio::test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
#[cfg_attr(not(target_os = "linux"), ignore)]
async fn test_symlink_to_hardlink_initialization_excluded_file() {
    let _ = env_logger::Builder::from_default_env().try_init();

    let db_dir = tempdir().expect("Couldn't create temp dir...");
    let db_dir_path = db_dir.path();

    let (server, received, shutdown_handle, cert_file, addr) = common::self_signed_https_ingester();

    let log_dir = tempdir().expect("Couldn't create temp dir...").into_path();
    let excluded_dir = tempdir().expect("Couldn't create temp dir...").into_path();
    let file_path = excluded_dir.join("test.log");
    let excluded_hardlink_path = excluded_dir.join("test-hardlink.log");
    let excluded_symlink_path = excluded_dir.join("test-symlink.log");
    let symlink_path = log_dir.join("test-symlink.log");

    File::create(&file_path).expect("Couldn't create temp log file...");
    std::fs::hard_link(&file_path, &excluded_hardlink_path).unwrap();
    let mut file = OpenOptions::new()
        .append(true)
        .create(true)
        .open(&file_path)
        .unwrap();
    for i in 0..10 {
        writeln!(file, "SAMPLE {}", i).unwrap();
    }
    file.sync_all().unwrap();
    std::os::unix::fs::symlink(&file_path, &excluded_symlink_path).unwrap();
    std::os::unix::fs::symlink(&excluded_symlink_path, &symlink_path).unwrap();

    debug!(
        "----{:?}: {}",
        file_path,
        file_path.metadata().unwrap().ino()
    );
    debug!(
        "----{:?}: {}",
        excluded_hardlink_path,
        excluded_hardlink_path.metadata().unwrap().ino()
    );
    debug!(
        "----{:?}: {}",
        excluded_symlink_path,
        excluded_symlink_path.metadata().unwrap().ino()
    );
    debug!(
        "----{:?}: {}",
        symlink_path,
        symlink_path.metadata().unwrap().ino()
    );

    let (server_result, _) = tokio::join!(server, async {
        tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;

        let settings = AgentSettings {
            log_dirs: log_dir.to_str().unwrap(),
            exclusion_regex: Some(r"/var\w*"),
            ssl_cert_file: Some(cert_file.path()),
            lookback: Some("start"),
            state_db_dir: Some(db_dir_path),
            host: Some(&addr),
            ..Default::default()
        };

        let mut agent_handle = common::spawn_agent(settings.clone());
        let stderr_reader = agent_handle.stderr.take().unwrap();
        // Consume output
        consume_output(stderr_reader);
        tokio::time::sleep(tokio::time::Duration::from_millis(2000)).await;
        for i in 10..20 {
            writeln!(file, "SAMPLE {}", i).unwrap();
        }
        file.sync_all().unwrap();
        common::force_client_to_flush(&log_dir).await;
        // Wait for the data to be received by the mock ingester
        tokio::time::sleep(tokio::time::Duration::from_millis(2000)).await;
        {
            let map = received.lock().await;
            let file_info = map
                .get(symlink_path.to_str().unwrap())
                .expect("symlink not found");
            for i in 0..20 {
                assert_eq!(file_info.values[i], format!("SAMPLE {}\n", i));
            }
        }
        agent_handle.kill().expect("Could not kill process");

        tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;

        // move 2nd symlink to point to hardlink
        std::fs::remove_file(&excluded_symlink_path).unwrap();
        std::os::unix::fs::symlink(&excluded_hardlink_path, &excluded_symlink_path).unwrap();

        debug!(
            "----{:?}: {}",
            file_path,
            file_path.metadata().unwrap().ino()
        );
        debug!(
            "----{:?}: {}",
            excluded_hardlink_path,
            excluded_hardlink_path.metadata().unwrap().ino()
        );
        debug!(
            "----{:?}: {}",
            excluded_symlink_path,
            excluded_symlink_path.metadata().unwrap().ino()
        );
        debug!(
            "----{:?}: {}",
            symlink_path,
            symlink_path.metadata().unwrap().ino()
        );

        let mut agent_handle = common::spawn_agent(settings);
        let stderr_reader = agent_handle.stderr.take().unwrap();
        // Consume output
        consume_output(stderr_reader);
        tokio::time::sleep(tokio::time::Duration::from_millis(2000)).await;
        for i in 20..30 {
            writeln!(file, "SAMPLE {}", i).unwrap();
        }
        file.sync_all().unwrap();
        common::force_client_to_flush(&log_dir).await;
        // Wait for the data to be received by the mock ingester
        tokio::time::sleep(tokio::time::Duration::from_millis(2000)).await;
        let map = received.lock().await;
        let file_info = map
            .get(symlink_path.to_str().unwrap())
            .expect("symlink not found");

        for v in file_info.values.iter() {
            debug!("line: {:?}", v);
        }
        for i in 0..30 {
            assert_eq!(file_info.values[i], format!("SAMPLE {}\n", i));
        }
        agent_handle.kill().expect("Could not kill process");
        tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;
        shutdown_handle();
    });
    server_result.unwrap();
}

#[tokio::test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
#[cfg_attr(not(target_os = "linux"), ignore)]
async fn test_symlink_initialization_with_stateful_lookback() {
    let log_dir = tempdir().expect("Couldn't create temp dir...").into_path();
    let excluded_dir = tempdir().expect("Couldn't create temp dir...").into_path();

    let db_dir = tempdir().expect("Couldn't create temp dir...");
    let db_dir_path = db_dir.path();

    let (server, received, shutdown_handle, cert_file, addr) = common::self_signed_https_ingester();

    let file_path = excluded_dir.join("test.log");
    let symlink_path = log_dir.join("test-symlink.log");

    File::create(&file_path).expect("Couldn't create temp log file...");
    let mut file = OpenOptions::new()
        .append(true)
        .create(true)
        .open(&file_path)
        .unwrap();

    for i in 0..10 {
        writeln!(file, "SAMPLE {}", i).unwrap();
    }
    file.sync_all().unwrap();

    std::os::unix::fs::symlink(&file_path, &symlink_path).unwrap();

    let (server_result, _, _) = tokio::join!(
        server,
        async {
            for i in 10..20 {
                writeln!(file, "SAMPLE {}", i).unwrap();
            }
            file.sync_all().unwrap();
            common::force_client_to_flush(&log_dir).await;

            // Wait for the data to be received by the mock ingester
            tokio::time::sleep(tokio::time::Duration::from_millis(5000)).await;

            let map = received.lock().await;
            let file_info = map
                .get(symlink_path.to_str().unwrap())
                .expect("symlink not found");
            for (i, line) in file_info.values.iter().enumerate() {
                assert_eq!(line.as_str(), &format!("SAMPLE {}\n", i));
            }
            shutdown_handle();
        },
        async {
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            let settings = AgentSettings {
                log_dirs: log_dir.to_str().unwrap(),
                exclusion_regex: Some(r"/var\w*"),
                ssl_cert_file: Some(cert_file.path()),
                lookback: Some("start"),
                state_db_dir: Some(db_dir_path),
                host: Some(&addr),
                ..Default::default()
            };
            let mut agent_handle = common::spawn_agent(settings.clone());
            let stderr_reader = agent_handle.stderr.take().unwrap();
            // Consume output
            consume_output(stderr_reader);

            tokio::time::sleep(tokio::time::Duration::from_millis(2000)).await;
            agent_handle.kill().expect("Could not kill process");

            let mut agent_handle = common::spawn_agent(settings);
            let stderr_reader = agent_handle.stderr.take().unwrap();
            // Consume output
            consume_output(stderr_reader);
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        }
    );

    server_result.unwrap();
}

async fn test_line_rules(
    exclusion: Option<&str>,
    inclusion: Option<&str>,
    redact: Option<&str>,
    to_write: Vec<&str>,
    expected: Vec<&str>,
) {
    let dir = tempdir().expect("Couldn't create temp dir...").into_path();

    let (server, received, shutdown_handle, addr) = common::start_http_ingester();
    let file_path = dir.join("test.log");
    let mut file = File::create(&file_path).expect("Couldn't create temp log file...");

    let mut settings = AgentSettings::with_mock_ingester(dir.to_str().unwrap(), &addr);
    settings.line_exclusion_regex = exclusion;
    settings.line_inclusion_regex = inclusion;
    settings.line_redact_regex = redact;
    let mut agent_handle = common::spawn_agent(settings);
    let mut stderr_reader = BufReader::new(agent_handle.stderr.as_mut().unwrap());
    common::wait_for_file_event("initialized", &file_path, &mut stderr_reader);

    let (server_result, _) = tokio::join!(server, async move {
        for item in to_write {
            write!(file, "{}", item).unwrap();

            if !item.ends_with('\n') {
                // Add partial lines on purpose
                file.sync_all().unwrap();
                common::force_client_to_flush(&dir).await;
            }
        }

        file.sync_all().unwrap();
        common::force_client_to_flush(&dir).await;

        // Wait for the data to be received by the mock ingester
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        let map = received.lock().await;
        let file_info = map.get(file_path.to_str().unwrap()).unwrap();
        assert_eq!(file_info.values, expected);
        shutdown_handle();
    });

    server_result.unwrap();
    agent_handle.kill().expect("Could not kill process");
}

#[tokio::test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
async fn test_line_exclusion_inclusion_redact() {
    let exclusion = Some("DEBUG,(?i:TRACE)");
    let inclusion = Some("(?i:ERROR),important");
    let redact = Some(r"\S+@\S+\.\S+,(?i:SENSITIVE)");

    let to_write = vec![
        // Included
        "some error\n",
        "something important\n",
        "important email@logdna.com\n",
        // Included and redacted
        "ERROR sensitive value\n",
        "This is an important",
        " partial line\n",
        // Excluded
        "important but trace message\n",
        "DEBUG message\n",
        "TRACE\n",
        "not included\n",
        "was an important DEBUG message\n",
        // Finally an included line
        "another ERROR line\n",
    ];

    let expected = vec![
        "some error\n",
        "something important\n",
        "important [REDACTED]\n",
        "ERROR [REDACTED] value\n",
        "This is an important partial line\n",
        "another ERROR line\n",
    ];

    test_line_rules(exclusion, inclusion, redact, to_write, expected).await;
}

#[tokio::test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
async fn test_line_exclusion() {
    let exclusion = Some("VERBOSE");

    let to_write = vec![
        "some message\n",
        "some verbose message\n",
        "another VERBOSE message\n",
        "a message\n",
    ];

    let expected = vec![
        "some message\n",
        "some verbose message\n", // Case-sensitive
        "a message\n",
    ];

    test_line_rules(exclusion, None, None, to_write, expected).await;
}

#[tokio::test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
async fn test_line_inclusion() {
    let inclusion = Some("only,included,message");

    let to_write = vec![
        "an included line\n",
        "only included messages\n",
        "sample\n",
        "an INCLUDED line? No, it is case-sensitive\n",
    ];

    let expected = vec!["an included line\n", "only included messages\n"];

    test_line_rules(None, inclusion, None, to_write, expected).await;
}

#[tokio::test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
async fn test_line_redact() {
    let redact = Some("(?i:SENSITIVE),(?i:SENSITIVE INFORMATION),(?i:VE )");

    let to_write = vec![
        "this is a SENSITIVE information\n",
        "this is another sensitive  value\n",
        "Five  messages\n",
        "a message\n",
    ];

    let expected = vec![
        "this is a [REDACTED]\n",
        "this is another [REDACTED] value\n",
        "Fi[REDACTED] messages\n",
        "a message\n",
    ];

    test_line_rules(None, None, redact, to_write, expected).await;
}

#[test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
fn lookback_stateful_lines_are_delivered() {
    let _ = env_logger::Builder::from_default_env().try_init();

    let db_dir = tempdir().expect("Couldn't create temp dir...");
    let db_dir_path = db_dir.path();
    let dir = tempdir().expect("Couldn't create temp dir...");

    let dir_path = format!("{}/", dir.path().to_str().unwrap());
    let log_lines = "This is a test log line";

    let file_path = dir.path().join("test.log");
    let mut file = File::create(&file_path).expect("Couldn't create temp log file...");

    // Enough bytes to get past the lookback threshold
    let line_write_count = (8192 / (log_lines.as_bytes().len() + 1)) + 1;

    (0..line_write_count)
        .for_each(|_| writeln!(file, "{}", log_lines).expect("Couldn't write to temp log file..."));
    file.sync_all().expect("Failed to sync file");

    // Write initial lines
    debug!("First agent run");
    let (server, received, shutdown_handle, cert_file, addr) = common::self_signed_https_ingester();
    thread::sleep(std::time::Duration::from_millis(250));
    let file_path1 = file_path.clone();
    let file_path_clone = file_path.clone();
    tokio_test::block_on(async {
        let (line_count, _, server) = tokio::join!(
            async {
                tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
                let mut handle = common::spawn_agent(AgentSettings {
                    log_dirs: &dir_path,
                    exclusion_regex: Some(r"/var\w*"),
                    ssl_cert_file: Some(cert_file.path()),
                    lookback: Some("start"),
                    state_db_dir: Some(db_dir_path),
                    host: Some(&addr),
                    ..Default::default()
                });

                consume_output(handle.stderr.take().unwrap());

                tokio::time::sleep(tokio::time::Duration::from_millis(5000)).await;

                handle.kill().unwrap();

                debug!("getting lines from {}", &file_path1.to_str().unwrap());
                let line_count = received
                    .lock()
                    .await
                    .get(&file_path1.to_str().unwrap().to_string())
                    .unwrap()
                    .lines;
                shutdown_handle();

                handle.wait().unwrap();
                line_count
            },
            async move {
                tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                let mut file = OpenOptions::new()
                    .append(true)
                    .open(&file_path_clone)
                    .expect("Couldn't create temp log file...");
                (0..5).for_each(|_| {
                    writeln!(file, "{}", log_lines).expect("Couldn't write to temp log file...");
                    file.sync_all().expect("Failed to sync file");
                });
                tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                file.sync_all().expect("Failed to sync file");
            },
            server
        );
        server.unwrap();
        assert_eq!(line_count, line_write_count + 5);
    });

    debug!("Second agent run");
    // Make sure the agent starts where it left off
    let file_path_clone = file_path.clone();
    let (server, received, shutdown_handle, cert_file, addr) = common::self_signed_https_ingester();
    thread::sleep(std::time::Duration::from_millis(250));
    tokio_test::block_on(async {
        let (line_count, _, server) = tokio::join!(
            async {
                tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
                let mut handle = common::spawn_agent(AgentSettings {
                    log_dirs: &dir_path,
                    exclusion_regex: Some(r"/var\w*"),
                    ssl_cert_file: Some(cert_file.path()),
                    lookback: Some("start"),
                    state_db_dir: Some(db_dir_path),
                    host: Some(&addr),
                    ..Default::default()
                });

                let stderr_reader = handle.stderr.take().unwrap();
                consume_output(stderr_reader);
                tokio::time::sleep(tokio::time::Duration::from_millis(3000)).await;

                handle.kill().unwrap();

                debug!("getting lines from {}", &file_path.to_str().unwrap());
                let line_count = received
                    .lock()
                    .await
                    .get(&file_path.to_str().unwrap().to_string())
                    .unwrap()
                    .lines;
                shutdown_handle();

                handle.wait().unwrap();
                line_count
            },
            async move {
                tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                let mut file = OpenOptions::new()
                    .append(true)
                    .create(false)
                    .open(&file_path_clone)
                    .expect("Couldn't create temp log file...");
                (0..5).for_each(|_| {
                    writeln!(file, "{}", log_lines).expect("Couldn't write to temp log file...");
                    file.sync_all().expect("Failed to sync file");
                });
                tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                file.sync_all().expect("Failed to sync file");
            },
            server
        );
        server.unwrap();
        assert_eq!(line_count, 5);
    });
}

#[tokio::test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
async fn test_tight_writes() {
    let _ = env_logger::Builder::from_default_env().try_init();
    let dir = tempdir().expect("Couldn't create temp dir...").into_path();

    let (server, received, shutdown_handle, addr) = common::start_http_ingester();
    let file_path = dir.join("test.log");
    File::create(&file_path).expect("Couldn't create temp log file...");
    let settings = AgentSettings::with_mock_ingester(dir.to_str().unwrap(), &addr);
    let mut agent_handle = common::spawn_agent(settings);
    let agent_stderr = agent_handle.stderr.take().unwrap();
    let mut stderr_reader = BufReader::new(agent_stderr);
    common::wait_for_file_event("initialized", &file_path, &mut stderr_reader);
    let agent_stderr = stderr_reader.into_inner();
    consume_output(agent_stderr);

    let (server_result, _) = tokio::join!(server, async {
        let line = "Nice short message";
        let lines = 500_000;
        let sync_every = 5_000;

        let mut file = OpenOptions::new()
            .append(true)
            .create(true)
            .open(&file_path)?;

        for i in 0..lines {
            if let Err(e) = writeln!(file, "{}", line) {
                eprintln!("Couldn't write to file: {}", e);
                return Err(e);
            }

            if i % sync_every == 0 {
                file.sync_all()?;
            }
        }
        file.sync_all()?;

        common::force_client_to_flush(&dir).await;

        // Wait for the data to be received by the mock ingester
        tokio::time::sleep(tokio::time::Duration::from_millis(20000)).await;
        writeln!(file, "And we're done").unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        let map = received.lock().await;
        let file_info = map.get(file_path.to_str().unwrap()).unwrap();
        assert_eq!(file_info.lines, lines + 1);
        shutdown_handle();
        Ok(())
    });

    server_result.unwrap();
    agent_handle.kill().expect("Could not kill process");
}

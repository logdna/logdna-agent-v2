use std::fs;
use std::fs::{File, OpenOptions};
use std::io::{BufReader, Write};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
#[cfg(any(target_os = "windows", target_os = "linux"))]
use std::thread;
use std::time::Duration;

use crate::common::{self, consume_output, AgentSettings};

use assert_cmd::prelude::*;
#[cfg(any(target_os = "windows", target_os = "linux"))]
use futures::FutureExt;
#[cfg(any(target_os = "windows", target_os = "linux"))]
use logdna_mock_ingester::FileInfo;
use predicates::prelude::*;
#[cfg(any(target_os = "windows", target_os = "linux"))]
use proptest::prelude::*;
use tempfile::tempdir;
#[cfg(any(target_os = "windows", target_os = "linux"))]
use test_types::strategies::random_line_string_vec;
use tokio::io::BufWriter;
#[cfg(any(target_os = "windows", target_os = "linux"))]
use tokio::task;
use tracing::debug;
#[cfg(any(target_os = "windows", target_os = "linux"))]
use tracing::info;

use test_log::test;

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
    let dir = tempdir().expect("Couldn't create temp dir...");

    let dir_path = format!("{}/", dir.path().to_str().unwrap());

    let before_file_path = dir.path().join("before.log");
    let mut file = File::create(before_file_path).expect("Couldn't create temp log file...");

    let mut handle = common::spawn_agent(AgentSettings {
        log_dirs: &dir_path,
        lookback: Some("start"),
        ..Default::default()
    });

    let mut stderr_reader = BufReader::new(handle.stderr.take().unwrap());
    common::wait_for_event("running version", &mut stderr_reader);

    let log_lines = "This is a test log line\nLook at me, another test log line\nMore log lines....\nAnother log line!";

    writeln!(file, "{}", log_lines).expect("Couldn't write to temp log file...");
    file.sync_all().unwrap();

    let test_file_path = dir.path().join("test.log");
    let mut file = File::create(test_file_path).expect("Couldn't create temp log file...");

    writeln!(file, "{}", log_lines).expect("Couldn't write to temp log file...");
    file.sync_all().unwrap();

    handle.kill().unwrap();
}

#[test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
#[cfg(any(target_os = "windows", target_os = "linux"))] // needs a refactor to use self signed server on mac
fn test_append_and_delete() {
    let dir = tempdir().expect("Could not create temp dir").into_path();
    let file_path = dir.join("file1.log");
    File::create(&file_path).expect("Could not create file");

    let mut agent_handle = common::spawn_agent(AgentSettings::new(dir.to_str().unwrap()));
    let mut stderr_reader = BufReader::new(agent_handle.stderr.take().unwrap());

    common::wait_for_file_event("initialize", &file_path, &mut stderr_reader);
    thread::sleep(std::time::Duration::from_millis(1000));

    debug!("got event, appending to file");
    common::append_to_file(&file_path, 1000, 50).expect("Could not append");
    fs::remove_file(&file_path).expect("Could not remove file");

    // Immediately, start appending in a new file
    common::append_to_file(&file_path, 5, 5).expect("Could not append");

    debug!("got event, waiting for unwatching");
    common::wait_for_file_event("unwatching", &file_path, &mut stderr_reader);
    debug!("got event, waiting for watching");
    common::wait_for_file_event("watching", &file_path, &mut stderr_reader);
    consume_output(stderr_reader.into_inner());

    common::assert_agent_running(&mut agent_handle);

    agent_handle.kill().expect("Could not kill process");
}

#[test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
#[cfg(any(target_os = "windows", target_os = "linux"))] // needs a refactor to use self signed server on mac
fn test_file_added_after_initialization() {
    let dir = tempdir().expect("Could not create temp dir").into_path();
    let mut agent_handle = common::spawn_agent(AgentSettings::new(dir.to_str().unwrap()));
    let mut reader = BufReader::new(agent_handle.stderr.take().unwrap());

    common::wait_for_event("Enabling filesystem", &mut reader);

    let file_path = dir.join("file1.log");
    File::create(&file_path).expect("Could not create file");

    common::wait_for_file_event("watching", &file_path, &mut reader);
    common::append_to_file(&file_path, 1000, 50).expect("Could not append");
    common::wait_for_file_event("tailer sendings lines for", &file_path, &mut reader);

    common::assert_agent_running(&mut agent_handle);

    agent_handle.kill().expect("Could not kill process");
}

#[test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
#[cfg(any(target_os = "windows", target_os = "linux"))]
fn test_delete_does_not_leave_file_descriptor() {
    let dir = tempdir().expect("Could not create temp dir").into_path();
    let file_path = dir.join("file1.log");
    File::create(&file_path).expect("Could not create file");
    let mut settings = AgentSettings::new(dir.to_str().unwrap());
    settings.log_level = Some("debug,notify_stream=trace,fs::cache=trace");
    let mut agent_handle = common::spawn_agent(settings);

    let process_id = agent_handle.id();
    let mut stderr_reader = BufReader::new(agent_handle.stderr.take().unwrap());

    common::wait_for_file_event("initialize", &file_path, &mut stderr_reader);
    common::append_to_file(&file_path, 100, 50).expect("Could not append");

    // Verify that the file is shown in the open files of the process
    assert!(common::open_files_include(process_id, &file_path).is_some());

    // Remove it
    fs::remove_file(&file_path).expect("Could not remove file");
    common::wait_for_file_event("unwatching", &file_path, &mut stderr_reader);

    consume_output(stderr_reader.into_inner());

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
    fn is_file_open(file: &std::path::Path) -> bool {
        let child = Command::new("lsof")
            .args([file.to_str().unwrap()])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("failed to execute child");

        let output = child.wait_with_output().expect("failed to wait on child");
        // lsof will success when file is found
        output.status.success()
    }

    use wait_timeout::ChildExt;

    let dir = tempdir().expect("Could not create temp dir").into_path();
    let file_path = dir.join("file1.log");
    File::create(&file_path).expect("Could not create file");

    let mut agent_handle = common::spawn_agent(AgentSettings::new(dir.to_str().unwrap()));

    let process_id = agent_handle.id();
    let mut stderr_reader = BufReader::new(agent_handle.stderr.as_mut().unwrap());

    common::wait_for_file_event("initialize", &file_path, &mut stderr_reader);
    common::append_to_file(&file_path, 10, 10).expect("Could not append");

    // Verify that the file is shown in the open files
    assert!(is_file_open(&file_path));

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
    assert!(!is_file_open(&file_path));
}

#[test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
#[cfg(any(target_os = "windows", target_os = "linux"))] // needs a refactor to use self signed server on mac
fn test_append_and_move() {
    let dir = tempdir().expect("Could not create temp dir").into_path();
    let file1_path = dir.join("file1.log");
    let file2_path = dir.join("file2.log");
    File::create(&file1_path).expect("Could not create file");

    let mut agent_handle = common::spawn_agent(AgentSettings::new(dir.to_str().unwrap()));
    let mut stderr_reader = BufReader::new(agent_handle.stderr.as_mut().unwrap());

    common::wait_for_file_event("initialize", &file1_path, &mut stderr_reader);
    common::append_to_file(&file1_path, 1000, 50).expect("Could not append");
    fs::rename(&file1_path, &file2_path).expect("Could not move file");
    fs::remove_file(&file2_path).expect("Could not remove file");

    // Immediately, start appending in a new file
    common::append_to_file(&file1_path, 5, 5).expect("Could not append");

    // Should be added back
    common::wait_for_file_event("watching", &file1_path, &mut stderr_reader);

    common::assert_agent_running(&mut agent_handle);

    agent_handle.kill().expect("Could not kill process");
}

#[test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
#[cfg(any(target_os = "windows", target_os = "linux"))]
fn test_truncate_file() {
    // K8s uses copytruncate, see https://github.com/kubernetes/kubernetes/issues/38495
    let dir = tempdir().expect("Could not create temp dir").into_path();
    let file_path = dir.join("file1.log");
    common::append_to_file(&file_path, 100, 50).expect("Could not append");

    let mut agent_handle = common::spawn_agent(AgentSettings::new(dir.to_str().unwrap()));

    let mut stderr_reader = BufReader::new(agent_handle.stderr.as_mut().unwrap());

    common::wait_for_file_event("initialize", &file_path, &mut stderr_reader);
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
#[cfg(any(target_os = "windows", target_os = "linux"))]
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

    let lines = common::wait_for_file_event("initialize", &included_file, &mut stderr_reader);

    let matches_excluded_file =
        predicate::str::is_match(r"initialize event for file [^\n]*file2\.log").unwrap();
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

#[ignore] // Broken by Glob syntax change
#[test]
// #[cfg_attr(not(feature = "integration_tests"), ignore)]
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
    let lines = common::wait_for_file_event("initialize", &included, &mut stderr_reader);

    let matches_excluded1 = predicate::str::is_match(format!(
        r"initialize event for file [^\n]*{}",
        excluded1.file_stem().unwrap().to_str().unwrap()
    ))
    .unwrap();
    let matches_excluded2 = predicate::str::is_match(format!(
        r"initialize event for file [^\n]*{}",
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
#[allow(clippy::needless_late_init)]
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
    let lines = common::wait_for_file_event("initialize", &included_file, &mut reader);

    for file_name in &not_included_files {
        let file_parts: Vec<&str> = file_name.split('.').collect();
        let regex;
        if file_parts.len() == 2 {
            regex = format!(
                "{}{}\\.{}",
                r"initialize event for file [^\n]*", file_parts[0], file_parts[1]
            );
        } else {
            regex = format!("{}{}", r"initialize event for file [^\n]*", file_name);
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
#[cfg_attr(not(all(target_os = "linux", feature = "integration_tests")), ignore)]
#[cfg(unix)]
fn test_dangling_symlinks() {
    let log_dir = tempdir().expect("Could not create temp dir").into_path();
    let log_sub_dir = log_dir.join("sub");
    fs::create_dir(&log_sub_dir).unwrap();
    let data_dir = tempdir().expect("Could not create temp dir").into_path();
    let file_path = data_dir.join("file1.log");
    let symlink_path = log_sub_dir.join("file1.log");
    common::append_to_file(&file_path, 100, 50).expect("Could not append");

    let mut settings = AgentSettings::new(log_dir.to_str().unwrap());
    settings.log_level = Some("debug,notify_stream=trace,fs::cache=trace");
    let mut agent_handle = common::spawn_agent(settings);

    let mut stderr_reader = BufReader::new(agent_handle.stderr.take().unwrap());

    std::os::unix::fs::symlink(&file_path, &symlink_path).unwrap();
    common::wait_for_file_event("initialize", &file_path, &mut stderr_reader);
    common::append_to_file(&file_path, 100, 20).expect("Could not append");

    // Remove original file first
    fs::remove_file(&file_path).expect("Could not remove file");

    common::wait_for_file_event("unwatching", &file_path, &mut stderr_reader);

    // Remove the dangling symlink also
    fs::remove_file(&symlink_path).expect("Could not remove symlink");
    common::wait_for_file_event("unwatching", &file_path, &mut stderr_reader);

    common::wait_for_file_event("unwatching", &log_sub_dir, &mut stderr_reader);

    consume_output(stderr_reader.into_inner());

    common::assert_agent_running(&mut agent_handle);
    agent_handle.kill().expect("Could not kill process");
}

#[test]
#[cfg_attr(not(all(target_os = "linux", feature = "integration_tests")), ignore)]
#[cfg(unix)]
fn test_append_after_symlinks_delete() {
    let log_dir = tempdir().expect("Could not create temp dir").into_path();
    let data_dir = tempdir().expect("Could not create temp dir").into_path();
    let file_path = data_dir.join("file1.log");
    let symlink_path = log_dir.join("file1.log");
    common::append_to_file(&file_path, 100, 50).expect("Could not append");

    let mut agent_handle = common::spawn_agent(AgentSettings::new(log_dir.to_str().unwrap()));
    let mut stderr_reader = BufReader::new(agent_handle.stderr.as_mut().unwrap());
    common::wait_for_event("Enabling filesystem", &mut stderr_reader);

    std::os::unix::fs::symlink(&file_path, &symlink_path).unwrap();
    common::wait_for_file_event("added", &file_path, &mut stderr_reader);
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
#[cfg_attr(not(all(target_os = "linux", feature = "integration_tests")), ignore)]
#[cfg(unix)]
fn test_directory_symlinks_delete() {
    fn create_dirs<P: AsRef<std::path::Path>>(dirs: &[P]) {
        for dir in dirs {
            std::fs::create_dir(dir).expect("Unable to create dir");
        }
    }

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

    create_dirs(&[&dir_1_path, &dir_1_1_path, &dir_1_2_path, &dir_1_2_1_path]);

    common::append_to_file(&file1_path, 100, 50).expect("Could not append");
    common::append_to_file(&file2_path, 100, 50).expect("Could not append");
    common::append_to_file(&file3_path, 100, 50).expect("Could not append");

    let mut agent_handle = common::spawn_agent(AgentSettings::new(log_dir.to_str().unwrap()));

    let mut stderr_reader = BufReader::new(agent_handle.stderr.take().unwrap());

    std::os::unix::fs::symlink(&dir_1_path, &symlink_path).unwrap();

    debug!("waiting for watching");
    common::wait_for_file_event("watching", &symlink_path, &mut stderr_reader);

    common::append_to_file(&file1_path, 1_000, 50).expect("Could not append");
    common::append_to_file(&file2_path, 1_000, 50).expect("Could not append");
    common::append_to_file(&file3_path, 1_000, 50).expect("Could not append");

    fs::remove_file(&symlink_path).expect("Could not remove symlink");

    debug!("waiting for unwatching");
    common::wait_for_file_event("unwatching", &file3_path, &mut stderr_reader);
    consume_output(stderr_reader.into_inner());

    common::assert_agent_running(&mut agent_handle);
    agent_handle.kill().expect("Could not kill process");
}

#[tokio::test]
#[cfg(all(target_os = "linux", feature = "integration_tests"))]
async fn test_z_journald_support() {
    tokio::time::sleep(Duration::from_millis(500)).await;
    let dir = "/var/log/journal";
    let (server, received, shutdown_handle, addr) = common::start_http_ingester();
    let mut settings = AgentSettings::with_mock_ingester("/var/log/journal", &addr);
    settings.journald_dirs = Some(dir);
    settings.features = Some("libjournald");
    settings.exclusion_regex = Some(r"^(?!/var/log/journal).*$");
    settings.log_journal_d = Some("true");

    assert_eq!(systemd::journal::print(6, "Sample info"), 0);
    let mut agent_handle = common::spawn_agent(settings);
    let mut agent_stderr = BufReader::new(agent_handle.stderr.take().unwrap());

    let (server_result, _) = tokio::join!(server, async {
        common::wait_for_event("monitoring journald path", &mut agent_stderr);
        consume_output(agent_stderr.into_inner());
        for _ in 0..10 {
            systemd::journal::print(1, "Sample alert");
            systemd::journal::print(6, "Sample info");
        }

        // Wait for the data to be received by the mock ingester
        tokio::time::sleep(tokio::time::Duration::from_millis(1500)).await;

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
#[cfg(all(target_os = "linux", feature = "integration_tests"))]
async fn test_z_journald_support_no_flag() {
    tokio::time::sleep(Duration::from_millis(500)).await;
    let dir = "/var/log/journal";
    let (server, _received, shutdown_handle, addr) = common::start_http_ingester();
    let mut settings = AgentSettings::with_mock_ingester("/var/log/journal", &addr);
    settings.journald_dirs = Some(dir);
    settings.features = Some("libjournald");
    settings.exclusion_regex = Some(r"^(?!/var/log/journal).*$");

    assert_eq!(systemd::journal::print(6, "Sample info"), 0);
    let mut agent_handle = common::spawn_agent(settings);
    let mut agent_stderr = BufReader::new(agent_handle.stderr.take().unwrap());

    let (server_result, _) = tokio::join!(server, async {
        common::wait_for_event("monitoring journald path", &mut agent_stderr);
        shutdown_handle();
    });

    server_result.unwrap();
    common::assert_agent_running(&mut agent_handle);
    agent_handle.kill().expect("Could not kill process");
}

#[tokio::test]
#[cfg(all(target_os = "linux", feature = "integration_tests"))]
async fn test_z_journalctl_support_true_flag_no_path() {
    tokio::time::sleep(Duration::from_millis(500)).await;
    let (server, _received, shutdown_handle, addr) = common::start_http_ingester();
    let mut settings = AgentSettings::with_mock_ingester("/var/log/journal", &addr);
    settings.features = Some("libjournald");
    settings.journald_dirs = None;
    settings.log_journal_d = Some("true");

    assert_eq!(systemd::journal::print(6, "Sample info"), 0);
    let mut agent_handle = common::spawn_agent(settings);
    let mut agent_stderr = BufReader::new(agent_handle.stderr.take().unwrap());

    let (server_result, _) = tokio::join!(server, async {
        common::wait_for_event("journalctl", &mut agent_stderr);
        shutdown_handle();
    });

    server_result.unwrap();
    common::assert_agent_running(&mut agent_handle);
    agent_handle.kill().expect("Could not kill process");
}

#[tokio::test]
#[cfg(all(target_os = "linux", feature = "integration_tests"))]
async fn test_journalctl_support() {
    assert_eq!(systemd::journal::print(6, "Sample info"), 0);
    tokio::time::sleep(Duration::from_millis(1000)).await;
    let (server, received, shutdown_handle, addr) = common::start_http_ingester();
    let mut settings = AgentSettings::with_mock_ingester("/var/log/journal", &addr);
    settings.journald_dirs = None;
    settings.log_journal_d = Some("true");
    settings.exclusion_regex = Some(r"^(?!/var/log/journal).*$");

    assert_eq!(systemd::journal::print(6, "Sample info"), 0);
    let mut agent_handle = common::spawn_agent(settings);
    let mut agent_stderr = BufReader::new(agent_handle.stderr.take().unwrap());

    common::wait_for_event("Listening to journalctl", &mut agent_stderr);
    consume_output(agent_stderr.into_inner());

    assert_eq!(systemd::journal::print(6, "Sample info"), 0);
    tokio::time::sleep(Duration::from_millis(1000)).await;
    common::assert_agent_running(&mut agent_handle);

    let (server_result, _) = tokio::join!(server, async {
        for _ in 0..10 {
            systemd::journal::print(1, "Sample alert");
            systemd::journal::print(6, "Sample info");
        }

        // Wait for the data to be received by the mock ingester
        tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;

        let map = received.lock().await;

        debug!("Received: {:?}", map);
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

#[cfg(any(target_os = "windows", target_os = "linux"))]
proptest! {
    #![proptest_config(ProptestConfig::with_cases(1))]
    #[test]
    #[cfg_attr(not(feature = "integration_tests"), ignore)]
    fn lookback_start_lines_are_delivered(log_lines in random_line_string_vec(150, 2000)) {
        let dir = tempdir().expect("Couldn't create temp dir...");

        let dir_path = format!("{}/", dir.path().to_str().unwrap());
        let (server, received, shutdown_handle, cert_file, addr) = common::self_signed_https_ingester(
             Some(common::HttpVersion::Http1),
             Some(Box::new(|req| {
                 assert_eq!(req.version(), hyper::Version::HTTP_11);
                 None
             })),
             None);

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

        thread::sleep(std::time::Duration::from_secs(1));

        tokio_test::block_on(async {
            let ((line_count, lines), _, server) = tokio::join!(
                async {
                    let mut handle = common::spawn_agent(AgentSettings {
                        log_dirs: &dir_path,
                        exclusion_regex: Some(r"/var\w*"),
                        ssl_cert_file: Some(cert_file.path()),
                        lookback: Some("start"),
                        host: Some(&addr),
                        ..Default::default()
                    });

                    let mut stderr_reader = std::io::BufReader::new(handle.stderr.take().unwrap());
                    common::wait_for_event("Enabling filesystem", &mut stderr_reader);
                    consume_output(stderr_reader.into_inner());

                    tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;

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
                    });
                    tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;
                    file.sync_all().expect("Failed to sync file");
                    tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;
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
            );
        });
    }
}

#[test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
#[cfg(any(target_os = "windows", target_os = "linux"))]
fn lookback_none_lines_are_delivered() {
    let dir = tempdir().expect("Couldn't create temp dir...");
    let dir_path = format!("{}/", dir.path().to_str().unwrap());

    let (server, received, shutdown_handle, cert_file, addr) = common::self_signed_https_ingester(
        Some(common::HttpVersion::Http2),
        Some(Box::new(|req| {
            assert_eq!(req.version(), hyper::Version::HTTP_2);
            None
        })),
        None,
    );
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
                let mut handle = common::spawn_agent(AgentSettings {
                    log_dirs: &dir_path,
                    exclusion_regex: Some(r"^/var.*"),
                    ssl_cert_file: Some(cert_file.path()),
                    lookback: Some("none"),
                    host: Some(&addr),
                    ..Default::default()
                });
                debug!("spawned agent");

                let mut stderr_reader = std::io::BufReader::new(handle.stderr.take().unwrap());
                common::wait_for_event("Enabling filesystem", &mut stderr_reader);
                consume_output(stderr_reader.into_inner());

                tokio::time::sleep(tokio::time::Duration::from_millis(3000)).await;

                handle.kill().unwrap();

                tokio::time::sleep(tokio::time::Duration::from_millis(5000)).await;

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
                (0..5).for_each(|_| {
                    writeln!(file, "{}", log_lines).expect("Couldn't write to temp log file...");
                });
                file.sync_all().expect("Failed to sync file");
                tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;
                debug!("wrote 5 lines");
            },
            server
        );
        server.unwrap();
        assert_eq!(line_count, 5);
    });
}

#[test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
#[cfg(any(target_os = "windows", target_os = "linux"))] // needs a refactor to use self signed server on mac
fn lookback_tail_lines_file_created_after_agent_start_at_beg() {
    let dir = tempdir().expect("Couldn't create temp dir...");
    let dir_path = format!("{}/", dir.path().to_str().unwrap());

    let (server, received, shutdown_handle, cert_file, addr) = common::self_signed_https_ingester(
        Some(common::HttpVersion::Http2),
        Some(Box::new(|req| {
            assert_eq!(req.version(), hyper::Version::HTTP_2);
            None
        })),
        None,
    );

    tokio_test::block_on(async {
        let (line_count, server) = tokio::join!(
            async {
                let mut handle = common::spawn_agent(AgentSettings {
                    log_dirs: &dir_path,
                    exclusion_regex: Some(r"^/var.*"),
                    ssl_cert_file: Some(cert_file.path()),
                    lookback: Some("tail"),
                    host: Some(&addr),
                    ..Default::default()
                });
                debug!("spawned agent");

                let file_path = dir.path().join("start-tail-test.log");

                async move {
                    let file_path = dir.path().join("start-tail-test.log");
                    let log_lines = "This is a test log line";
                    let mut file =
                        File::create(&file_path).expect("Couldn't create temp log file...");

                    tokio::time::sleep(tokio::time::Duration::from_millis(2000)).await;
                    (0..5).for_each(|_| {
                        writeln!(file, "{}", log_lines)
                            .expect("Couldn't write to temp log file...");
                        file.sync_all().expect("Failed to sync file");
                    });
                    file.sync_all().expect("Failed to sync file");
                    debug!("wrote 5 lines");
                }
                .await;

                let mut stderr_reader = std::io::BufReader::new(handle.stderr.take().unwrap());
                common::wait_for_event("Enabling filesystem", &mut stderr_reader);
                consume_output(stderr_reader.into_inner());

                tokio::time::sleep(tokio::time::Duration::from_millis(3000)).await;

                handle.kill().unwrap();

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
            server
        );
        server.unwrap();
        assert_eq!(line_count, 5);
    });
}

#[test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
#[cfg(any(target_os = "windows", target_os = "linux"))] // needs a refactor to use self signed server on mac
fn lookback_tail_lines_file_created_before_agent_start_at_end() {
    let dir = tempdir().expect("Couldn't create temp dir...");
    let dir_path = format!("{}/", dir.path().to_str().unwrap());

    let (server, received, shutdown_handle, cert_file, addr) = common::self_signed_https_ingester(
        Some(common::HttpVersion::Http2),
        Some(Box::new(|req| {
            assert_eq!(req.version(), hyper::Version::HTTP_2);
            None
        })),
        None,
    );
    let log_lines = "This is a test log line";

    let file_path = dir.path().join("end-tail-test.log");
    let mut file = File::create(&file_path).expect("Couldn't create temp log file...");

    debug!("test log: {}", file_path.to_str().unwrap());
    (0..5)
        .for_each(|_| writeln!(file, "{}", log_lines).expect("Couldn't write to temp log file..."));

    file.sync_all().expect("Failed to sync file");

    tokio_test::block_on(async {
        let (line_count, _, server) = tokio::join!(
            async {
                let mut handle = common::spawn_agent(AgentSettings {
                    log_dirs: &dir_path,
                    exclusion_regex: Some(r"^/var.*"),
                    ssl_cert_file: Some(cert_file.path()),
                    lookback: Some("tail"),
                    host: Some(&addr),
                    ..Default::default()
                });
                debug!("spawned agent");

                let mut stderr_reader = std::io::BufReader::new(handle.stderr.take().unwrap());
                common::wait_for_event("Enabling filesystem", &mut stderr_reader);
                consume_output(stderr_reader.into_inner());

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
#[cfg(any(target_os = "windows", target_os = "linux"))]
async fn test_partial_fsynced_lines() {
    let dir = tempdir().expect("Couldn't create temp dir...").into_path();
    let (server, received, shutdown_handle, addr) = common::start_http_ingester();
    let file_path = dir.join("test.log");
    File::create(&file_path).expect("Couldn't create temp log file...");
    let mut settings = AgentSettings::with_mock_ingester(dir.to_str().unwrap(), &addr);
    settings.exclusion_regex = Some(r"/var\w*");
    let mut agent_handle = common::spawn_agent(settings);
    let mut stderr_reader = BufReader::new(agent_handle.stderr.as_mut().unwrap());
    common::wait_for_file_event("initialize", &file_path, &mut stderr_reader);
    let (server_result, _) = tokio::join!(server, async {
        let mut file = OpenOptions::new()
            .append(true)
            .create(true)
            .open(&file_path)
            .unwrap();

        write!(file, "first part ").unwrap();
        write!(file, "second part").unwrap();

        file.sync_all().unwrap();
        common::force_client_to_flush(&dir).await;
        tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;

        {
            let map = received.lock().await;
            // The ingester should not have received any lines yet
            let line = map.get(file_path.to_str().unwrap());
            assert!(line.is_none(), "{:?}", line);
        }

        writeln!(file, " third part").unwrap();
        write!(file, "begin second line").unwrap();

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
#[cfg(target_os = "linux")]
async fn test_transient_access_denied() {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let dir = tempdir().expect("Couldn't create temp dir...").into_path();
    let (server, received, shutdown_handle, addr) = common::start_http_ingester();
    let file_path = dir.join("test.log");

    let mut settings = AgentSettings::with_mock_ingester(dir.to_str().unwrap(), &addr);
    settings.lookback = Some("start");

    settings.exclusion_regex = Some(r"/var\w*");
    let mut agent_handle = common::spawn_agent(settings);
    let agent_stderr = agent_handle.stderr.take();
    let mut stderr_reader = BufReader::new(agent_stderr.unwrap());
    common::wait_for_event("Enabling filesystem", &mut stderr_reader);
    consume_output(stderr_reader.into_inner());

    let (server_result, _) = tokio::join!(server, async {
        let mut file = OpenOptions::new()
            .append(true)
            .create(true)
            .mode(0o660)
            .open(&file_path)
            .unwrap();
        writeln!(file, "line 1").unwrap();
        file.sync_all().unwrap();
        fs::set_permissions(&file_path, fs::Permissions::from_mode(0o000)).unwrap();

        tokio::time::sleep(tokio::time::Duration::from_millis(2000)).await;

        // Agent has failed to open the file
        {
            let map = received.lock().await;
            // The ingester should not have received any lines yet
            let line = map.get(file_path.to_str().unwrap());
            assert!(line.is_none(), "{:?}", line);
        }

        fs::set_permissions(&file_path, fs::Permissions::from_mode(0o775)).unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(2000)).await;

        // Agent has successfully opened the file
        {
            let map = received.lock().await;
            // The ingester should not have received any lines yet
            let line = map.get(file_path.to_str().unwrap());
            assert!(line.is_some(), "{:?}", line);
        }

        writeln!(file, "line 2").unwrap();
        file.sync_all().unwrap();
        common::force_client_to_flush(&dir).await;

        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        {
            let map = received.lock().await;
            let file_info = map.get(file_path.to_str().unwrap()).unwrap();
            assert_eq!(
                file_info.values,
                vec!["line 1\n".to_string(), "line 2\n".to_string()]
            );
        }

        shutdown_handle();
    });

    server_result.unwrap();
    agent_handle.kill().expect("Could not kill process");
}

#[tokio::test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
#[cfg(any(target_os = "windows", target_os = "linux"))]
async fn test_tags() {
    let dir = tempdir().expect("Couldn't create temp dir...").into_path();

    let (server, received, shutdown_handle, addr) = common::start_http_ingester();
    let file_path = dir.join("test.log");
    let tag = "my-tag";
    File::create(&file_path).expect("Couldn't create temp log file...");
    let mut settings = AgentSettings::with_mock_ingester(dir.to_str().unwrap(), &addr);
    settings.tags = Some(tag);
    let mut agent_handle = common::spawn_agent(settings);

    let mut agent_stderr = BufReader::new(agent_handle.stderr.take().unwrap());
    common::wait_for_event("Enabling filesystem", &mut agent_stderr);

    consume_output(agent_stderr.into_inner());

    let (server_result, _) = tokio::join!(server, async {
        let total_lines: usize = 10;
        common::append_to_file(&file_path, total_lines as i32, 5).unwrap();
        common::force_client_to_flush(&dir).await;

        let mut file_info = FileInfo::default();
        let map_key = file_path.to_str().unwrap();

        for _ in 0..10 {
            let map = received.lock().await;
            file_info = map.get(map_key).unwrap_or(&file_info).clone();
            // Avoid awaiting while holding the lock
            drop(map);

            if file_info.lines < total_lines {
                // Wait for the data to be received by the mock ingester
                tokio::time::sleep(Duration::from_millis(200)).await;
                continue;
            }

            break;
        }

        assert_eq!(file_info.lines, total_lines);
        assert_eq!(
            file_info.values,
            vec![common::LINE.to_owned() + "\n"; total_lines]
        );
        assert_eq!(file_info.tags, Some(tag.to_string()));
        shutdown_handle();
    });

    server_result.unwrap();
    agent_handle.kill().expect("Could not kill process");
}

#[tokio::test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
#[cfg(any(target_os = "windows", target_os = "linux"))]
async fn test_lookback_restarting_agent() {
    let line_count = Arc::new(AtomicUsize::new(0));

    let dir = tempdir().expect("Couldn't create temp dir...").into_path();
    let db_dir = tempdir().unwrap().into_path();
    let (server, received, shutdown_handle, addr) = common::start_http_ingester();

    let mut settings = AgentSettings::with_mock_ingester(dir.to_str().unwrap(), &addr);
    settings.state_db_dir = Some(&db_dir);
    settings.exclusion_regex = Some(r"/var\w*");
    settings.lookback = Some("smallfiles");
    settings.log_level = Some("info,notify_stream=trace,fs::cache=trace");

    let line_count_target = 5_000;

    let line_count_clone = line_count.clone();

    let (server_result, _) = tokio::join!(server, async {
        let file_path = dir.join("test.log");
        let mut file = OpenOptions::new()
            .append(true)
            .create(true)
            .open(&file_path)
            .unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        debug!("Running first agent");
        let mut agent_handle = common::spawn_agent(settings.clone());
        let mut agent_stderr = BufReader::new(agent_handle.stderr.take().unwrap());

        common::wait_for_file_event("initialize", &file_path, &mut agent_stderr);
        consume_output(agent_stderr.into_inner());
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        let writer_thread = std::thread::spawn(move || {
            for i in 0..line_count_target {
                writeln!(file, "Hello from line {}", i).unwrap();
                line_count_clone.fetch_add(1, Ordering::SeqCst);
                if i % 1000 == 0 {
                    file.sync_all().unwrap();
                    info!("Syncing background file");
                }

                if i % 20 == 0 {
                    std::thread::sleep(core::time::Duration::from_millis(20));
                }
            }
        });

        tokio::time::sleep(tokio::time::Duration::from_millis(2_000)).await;

        while line_count.load(Ordering::SeqCst) < line_count_target {
            tokio::time::sleep(tokio::time::Duration::from_millis(1_500)).await;
            agent_handle.kill().expect("Could not kill process");
            // Restart it back again
            info!("Running next agent");
            agent_handle = common::spawn_agent(settings.clone());
            let agent_stderr = agent_handle.stderr.take().unwrap();
            consume_output(agent_stderr);
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            common::assert_agent_running(&mut agent_handle);
        }

        // Block til writing is definitely done

        info!("Waiting a bit");
        task::spawn_blocking(move || writer_thread.join().unwrap())
            // Give the agent a chance to catch up
            .then(|_| tokio::time::sleep(tokio::time::Duration::from_millis(10000)))
            .await;

        info!("Sleeping to wait for agent");
        // Sleep a bit more to give the agent a chance to process
        tokio::time::sleep(tokio::time::Duration::from_millis(2500)).await;

        let map = received.lock().await;
        assert!(map.len() > 0);
        let file_info = map.get(file_path.to_str().unwrap()).unwrap();

        assert!(file_info.values.len() > 100);
        debug!(
            "{}, {}",
            file_info.values.len(),
            line_count.load(Ordering::SeqCst)
        );
        assert!(
            file_info.values.len() >= line_count.load(Ordering::SeqCst),
            "not enough lines: file_info.values len: {} line_count: {}",
            file_info.values.len(),
            line_count.load(Ordering::SeqCst)
        );

        for i in 0..file_info.values.len() {
            assert_eq!(file_info.values[i], format!("Hello from line {}\n", i));
        }

        agent_handle.kill().expect("Could not kill process");
        shutdown_handle();
    });
    server_result.unwrap();
}

#[tokio::test]
#[cfg_attr(not(all(target_os = "linux", feature = "integration_tests")), ignore)]
#[cfg(unix)]
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
        let mut stderr_reader = BufReader::new(agent_handle.stderr.take().unwrap());
        common::wait_for_event("Enabling filesystem", &mut stderr_reader);
        consume_output(stderr_reader.into_inner());
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
#[cfg_attr(not(all(target_os = "linux", feature = "integration_tests")), ignore)]
#[cfg(unix)]
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
        let mut stderr_reader = BufReader::new(agent_handle.stderr.take().unwrap());
        common::wait_for_event("Enabling filesystem", &mut stderr_reader);
        // Consume output
        consume_output(stderr_reader.into_inner());
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
#[cfg_attr(not(all(target_os = "linux", feature = "integration_tests")), ignore)]
#[cfg(unix)]
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
        let mut stderr_reader = BufReader::new(agent_handle.stderr.take().unwrap());
        common::wait_for_event("Enabling filesystem", &mut stderr_reader);
        // Consume output
        consume_output(stderr_reader.into_inner());
        for i in 10..20 {
            writeln!(file, "SAMPLE {}", i).unwrap();
        }
        file.sync_all().unwrap();
        common::force_client_to_flush(&log_dir).await;
        // Wait for the data to be received by the mock ingester
        tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;
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
#[cfg_attr(not(all(target_os = "linux", feature = "integration_tests")), ignore)]
#[cfg(unix)]
async fn test_symlink_to_dir_initialization_excluded_file() {
    let log_dir = tempdir().expect("Couldn't create temp dir...").into_path();
    let test_dir = tempdir().expect("Couldn't create temp dir...").into_path();
    let excluded_dir = test_dir.join("excluded");
    std::fs::create_dir(&excluded_dir).unwrap();
    let tracked_dir = test_dir.join("tracked");
    std::fs::create_dir(&tracked_dir).unwrap();

    let (server, received, shutdown_handle, addr) = common::start_http_ingester();
    let file_path = excluded_dir.join("test.log");
    let excluded_symlink_path = excluded_dir.join("test-symlink.log");
    let tracked_dir_symlink_path = tracked_dir.join(excluded_dir.file_name().unwrap());
    let tracked_symlink_path = tracked_dir_symlink_path.join("test-symlink.log");
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
    std::os::unix::fs::symlink(&excluded_dir, &tracked_dir_symlink_path).unwrap();

    assert!(tracked_symlink_path.exists());

    tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;
    let (server_result, _) = tokio::join!(server, async {
        let mut settings = AgentSettings::with_mock_ingester(test_dir.to_str().unwrap(), &addr);
        settings.lookback = Some("smallfiles");
        let excluded_dir_str = format!("{}/**", excluded_dir.to_str().unwrap());
        settings.exclusion = Some(&excluded_dir_str);
        let mut agent_handle = common::spawn_agent(settings);
        let mut stderr_reader = BufReader::new(agent_handle.stderr.take().unwrap());
        common::wait_for_event("Enabling filesystem", &mut stderr_reader);
        // Consume output
        consume_output(stderr_reader.into_inner());
        for i in 10..20 {
            writeln!(file, "SAMPLE {}", i).unwrap();
        }
        file.sync_all().unwrap();
        common::force_client_to_flush(&log_dir).await;
        // Wait for the data to be received by the mock ingester
        tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;
        let map = received.lock().await;
        assert!(map.get(symlink_path.to_str().unwrap()).is_none());
        assert!(map.get(excluded_symlink_path.to_str().unwrap()).is_none());
        let file_info = map
            .get(tracked_symlink_path.to_str().unwrap())
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
#[cfg_attr(not(all(target_os = "linux", feature = "integration_tests")), ignore)]
#[cfg(unix)]
async fn test_symlink_to_hardlink_initialization_excluded_file() {
    let db_dir = tempdir().expect("Couldn't create temp dir...");
    let db_dir_path = db_dir.path();

    let (server, received, shutdown_handle, cert_file, addr) =
        common::self_signed_https_ingester(None, None, None);

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

    let (server_result, _) = tokio::join!(server, async {
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

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
        let mut stderr_reader = BufReader::new(agent_handle.stderr.take().unwrap());
        // Consume output
        common::wait_for_event("Enabling filesystem", &mut stderr_reader);
        consume_output(stderr_reader.into_inner());
        for i in 10..20 {
            writeln!(file, "SAMPLE {}", i).unwrap();
        }
        file.sync_all().unwrap();
        common::force_client_to_flush(&log_dir).await;
        // Wait for the data to be received by the mock ingester
        tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;
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

        let mut agent_handle = common::spawn_agent(settings);
        let mut stderr_reader = BufReader::new(agent_handle.stderr.take().unwrap());
        // Consume output
        common::wait_for_event("Enabling filesystem", &mut stderr_reader);
        consume_output(stderr_reader.into_inner());
        for i in 20..30 {
            writeln!(file, "SAMPLE {}", i).unwrap();
        }
        file.sync_all().unwrap();
        common::force_client_to_flush(&log_dir).await;
        // Wait for the data to be received by the mock ingester
        tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;
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
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        shutdown_handle();
    });
    server_result.unwrap();
}

#[tokio::test]
#[cfg_attr(not(all(target_os = "linux", feature = "integration_tests")), ignore)]
#[cfg(unix)]
async fn test_symlink_initialization_with_stateful_lookback() {
    let log_dir = tempdir().expect("Couldn't create temp dir...").into_path();
    let excluded_dir = tempdir().expect("Couldn't create temp dir...").into_path();

    let db_dir = tempdir().expect("Couldn't create temp dir...");
    let db_dir_path = db_dir.path();

    let (server, received, shutdown_handle, cert_file, addr) =
        common::self_signed_https_ingester(None, None, None);

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
            let mut stderr_reader = BufReader::new(agent_handle.stderr.take().unwrap());
            // Consume output
            common::wait_for_event("Enabling filesystem", &mut stderr_reader);
            consume_output(stderr_reader.into_inner());

            agent_handle.kill().expect("Could not kill process");

            let mut agent_handle = common::spawn_agent(settings);
            let mut stderr_reader = BufReader::new(agent_handle.stderr.take().unwrap());
            // Consume output
            common::wait_for_event("Enabling filesystem", &mut stderr_reader);
            consume_output(stderr_reader.into_inner());
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        }
    );

    server_result.unwrap();
}

#[cfg(any(target_os = "windows", target_os = "linux"))] // needs a refactor to use self signed server on mac
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
    common::wait_for_file_event("initialize", &file_path, &mut stderr_reader);

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
#[cfg(any(target_os = "windows", target_os = "linux"))]
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
#[cfg(any(target_os = "windows", target_os = "linux"))]
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
#[cfg(any(target_os = "windows", target_os = "linux"))]
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
#[cfg(any(target_os = "windows", target_os = "linux"))]
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

#[tokio::test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
#[cfg(any(target_os = "windows", target_os = "linux"))] // needs a refactor to use self signed server on mac
async fn test_directory_created_after_initialization() {
    let dir = tempdir().expect("Couldn't create temp dir...").into_path();
    let future_dir = dir.join("inner");

    let (server, received, shutdown_handle, addr) = common::start_http_ingester();
    let settings = AgentSettings::with_mock_ingester(future_dir.to_str().unwrap(), &addr);
    let mut agent_handle = common::spawn_agent(settings);
    let (server_result, _) = tokio::join!(server, async {
        let mut stderr_reader = std::io::BufReader::new(agent_handle.stderr.take().unwrap());
        common::wait_for_event("Enabling filesystem", &mut stderr_reader);
        consume_output(stderr_reader.into_inner());

        let file_path = future_dir.join("test.log");
        std::fs::create_dir(&future_dir).unwrap();
        File::create(&file_path).unwrap();

        // Wait for file to be picked up by agent
        tokio::time::sleep(Duration::from_millis(500)).await;

        common::append_to_file(&file_path, 10, 1).unwrap();

        // Wait for the data to be received by the mock ingester
        tokio::time::sleep(Duration::from_millis(500)).await;

        common::force_client_to_flush(&future_dir).await;

        let map = received.lock().await;
        let file_info = map.get(file_path.to_str().unwrap()).unwrap();
        assert_eq!(file_info.lines, 10);
        shutdown_handle();
    });
    server_result.unwrap();
    agent_handle.kill().expect("Could not kill process");
}

#[test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
#[cfg(any(target_os = "windows", target_os = "linux"))]
fn lookback_stateful_lines_are_delivered() {
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
    let (server, received, shutdown_handle, cert_file, addr) =
        common::self_signed_https_ingester(None, None, None);
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

                let mut stderr_reader = BufReader::new(handle.stderr.take().unwrap());
                common::wait_for_event("Enabling filesystem", &mut stderr_reader);
                consume_output(stderr_reader.into_inner());

                tokio::time::sleep(tokio::time::Duration::from_millis(2000)).await;
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
                    file.sync_all().expect("Failed to sync file");
                    writeln!(file, "{}", log_lines).expect("Couldn't write to temp log file...");
                });
                tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;
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
    let (server, received, shutdown_handle, cert_file, addr) =
        common::self_signed_https_ingester(None, None, None);
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

                let mut stderr_reader = BufReader::new(handle.stderr.take().unwrap());
                common::wait_for_event("Enabling filesystem", &mut stderr_reader);
                consume_output(stderr_reader.into_inner());
                tokio::time::sleep(tokio::time::Duration::from_millis(2000)).await;

                handle.kill().unwrap();

                tokio::time::sleep(tokio::time::Duration::from_millis(2000)).await;

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
#[cfg(any(target_os = "windows", target_os = "linux"))]
async fn test_tight_writes() {
    let dir = tempdir().expect("Couldn't create temp dir...").into_path();

    let (server, received, shutdown_handle, addr) = common::start_http_ingester();
    let file_path = dir.join("test.log");
    File::create(&file_path).expect("Couldn't create temp log file...");
    let settings = AgentSettings::with_mock_ingester(dir.to_str().unwrap(), &addr);
    let mut agent_handle = common::spawn_agent(settings);
    let agent_stderr = agent_handle.stderr.take().unwrap();
    let mut stderr_reader = BufReader::new(agent_stderr);
    common::wait_for_file_event("initialize", &file_path, &mut stderr_reader);
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
        tokio::time::sleep(tokio::time::Duration::from_millis(5000)).await;
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

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
#[cfg(any(target_os = "windows", target_os = "linux"))]
async fn test_tight_writes_with_slow_ingester() {
    let dir = tempdir().expect("Couldn't create temp dir...").into_path();

    let (server, received, shutdown_handle, cert_file, addr) = common::self_signed_https_ingester(
        None,
        Some(Box::new(|req| {
            assert_eq!(req.version(), hyper::Version::HTTP_2);
            None
        })),
        Some(Box::new(|_| {
            Some(Box::pin(tokio::time::sleep(Duration::from_millis(2500))))
        })),
    );

    let file_path = dir.join("test.log");
    File::create(&file_path).expect("Couldn't create temp log file...");
    let mut settings = AgentSettings::with_mock_ingester(dir.to_str().unwrap(), &addr);
    settings.use_ssl = true;
    settings.ssl_cert_file = Some(cert_file.path());
    settings.log_level = Some("rustls::client::hs=debug,info");
    let mut agent_handle = common::spawn_agent(settings);
    let agent_stderr = agent_handle.stderr.take().unwrap();
    let mut stderr_reader = BufReader::new(agent_stderr);
    common::wait_for_file_event("initialize", &file_path, &mut stderr_reader);
    let agent_stderr = stderr_reader.into_inner();
    consume_output(agent_stderr);

    let (server_result, _) = tokio::join!(server, async {
        let line = "Nice short message";
        let lines = 1_000_000;
        let sync_every = 50_000;

        let mut file = std::io::BufWriter::new(
            OpenOptions::new()
                .append(true)
                .create(true)
                .open(&file_path)?,
        );

        let delay_count = 45;
        for i in 0..lines - delay_count {
            if let Err(e) = writeln!(file, "{}", line) {
                eprintln!("Couldn't write to file: {}", e);
                return Err(e);
            }

            if i % sync_every == 0 {
                file.flush()?;
            }
        }
        file.flush()?;

        common::force_client_to_flush(&dir).await;

        for _ in 0..delay_count {
            tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;
            if let Err(e) = writeln!(file, "{}", line) {
                eprintln!("Couldn't write to file: {}", e);
                return Err(e);
            }
            file.flush()?;
        }

        // Wait for the data to be received by the mock ingester
        writeln!(file, "And we're done").unwrap();
        file.flush()?;
        tokio::time::sleep(tokio::time::Duration::from_millis(4000)).await;

        let map = received.lock().await;
        let file_info = map.get(file_path.to_str().unwrap()).unwrap();
        assert_eq!(file_info.lines, lines + 1);
        shutdown_handle();
        Ok(())
    });

    server_result.unwrap();
    agent_handle.kill().expect("Could not kill process");
}

#[tokio::test]
#[cfg_attr(not(feature = "slow_tests"), ignore)]
async fn test_endurance_writes() {
    use tokio::io::AsyncWriteExt;
    let dir = tempdir().expect("Couldn't create temp dir...").into_path();

    let line_count = Arc::new(AtomicUsize::new(0));

    let (server, received, shutdown_handle, addr) = common::start_ingester(Box::new(|_| None), {
        let counter = line_count.clone();
        Box::new(move |body| {
            counter.fetch_add(body.lines.len(), Ordering::SeqCst);
            None
        })
    });
    let file_path = dir.join("test.log");
    File::create(&file_path).expect("Couldn't create temp log file...");
    let mut settings = AgentSettings::with_mock_ingester(dir.to_str().unwrap(), &addr);
    settings.log_level = Some("info");
    let mut agent_handle = common::spawn_agent(settings);
    let agent_stderr = agent_handle.stderr.take().unwrap();
    let mut stderr_reader = BufReader::new(agent_stderr);
    common::wait_for_file_event("initialize", &file_path, &mut stderr_reader);
    let agent_stderr = stderr_reader.into_inner();
    consume_output(agent_stderr);

    let (server_result, _) = tokio::join!(server, async {
        let line = "Nice short message\n";

        let sync_every = 5_000;

        let mut file = BufWriter::new(
            tokio::fs::OpenOptions::new()
                .append(true)
                .create(true)
                .open(&file_path)
                .await?,
        );

        let now = std::time::Instant::now();

        let mut i = 0;
        let mut elapsed = 1u64;
        while elapsed < 90 {
            i += 1;
            if let Err(e) = file.write_all(line.as_bytes()).await {
                eprintln!("Couldn't write to file: {}", e);
                return Err(e);
            }
            if i % (sync_every / 10) == 0 {
                tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;
            }
            if i % sync_every == 0 {
                file.flush().await?;

                if elapsed > 0 {
                    let count = line_count.load(Ordering::SeqCst);
                    println!(
                        "written: {}, received: {}, written/sec, {}, received/sec: {}",
                        i,
                        count,
                        i / elapsed,
                        count as u64 / elapsed
                    );
                }
            }
            elapsed = now.elapsed().as_secs();
        }

        common::force_client_to_flush(&dir).await;

        // Wait for the data to be received by the mock ingester
        tokio::time::sleep(tokio::time::Duration::from_millis(10000)).await;
        file.write_all("One more and we're done\n".as_bytes())
            .await
            .unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        file.write_all("And we're done\n".as_bytes()).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        let map = received.lock().await;
        let file_info = map.get(file_path.to_str().unwrap()).unwrap();
        let diff = i as usize - file_info.lines;
        assert!(diff < 1000);
        shutdown_handle();
        Ok(())
    });

    server_result.unwrap();
    agent_handle.kill().expect("Could not kill process");
}

#[test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
#[cfg(any(target_os = "windows", target_os = "linux"))]
fn test_clear_cache() {
    let dir = tempdir().expect("Could not create temp dir").into_path();
    let file_path = dir.join("file1.log");
    File::create(&file_path).expect("Could not create file");

    let mut settings = AgentSettings::new(dir.to_str().unwrap());
    settings.clear_cache_interval = Some(3);

    let mut agent_handle = common::spawn_agent(settings);

    let mut stderr_reader = BufReader::new(agent_handle.stderr.take().unwrap());

    common::wait_for_file_event("initialize", &file_path, &mut stderr_reader);

    debug!("got event, appending to file");
    common::append_to_file(&file_path, 5, 50).expect("Could not append");
    thread::sleep(std::time::Duration::from_millis(5000));
    common::append_to_file(&file_path, 5, 50).expect("Could not append");

    debug!("waiting for restarting");
    common::wait_for_event("restarting stream, interval=3", &mut stderr_reader);
    debug!("got restarting");

    // check that fs tailer is functional after restart
    thread::sleep(std::time::Duration::from_millis(1000));
    debug!("delete & append again");
    fs::remove_file(&file_path).expect("Could not remove file");
    // Immediately, start appending in a new file
    common::append_to_file(&file_path, 5, 5).expect("Could not append");

    thread::sleep(std::time::Duration::from_millis(3000));

    debug!("waiting for watching");
    common::wait_for_file_event("watching", &file_path, &mut stderr_reader);
    consume_output(stderr_reader.into_inner());
    common::assert_agent_running(&mut agent_handle);

    agent_handle.kill().expect("Could not kill process");
}

#[test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
#[cfg(any(target_os = "windows", target_os = "linux"))]
fn test_offset_stream_state_gc() {
    use ::fs::cache::get_inode;

    let _ = env_logger::Builder::from_default_env().try_init();
    let dir = tempdir().expect("Could not create temp dir").into_path();
    let db_dir = tempdir().expect("Could not create temp dir").into_path();
    let file_path = dir.join("file1.log");

    File::create(&file_path).expect("Could not create file");
    let inode = get_inode(file_path.as_path(), None).expect("Failed to get inode");

    let mut settings = AgentSettings::new(dir.to_str().unwrap());
    settings.clear_cache_interval = Some(3);
    settings.state_db_dir = Some(db_dir.as_path());

    let mut agent_handle = common::spawn_agent(settings);

    let mut stderr_reader = BufReader::new(agent_handle.stderr.take().unwrap());

    common::wait_for_file_event("initialize", &file_path, &mut stderr_reader);

    common::append_to_file(&file_path, 15, 5).expect("Could not append");
    thread::sleep(std::time::Duration::from_millis(5000));
    common::append_to_file(&file_path, 15, 5).expect("Could not append");

    let msg = format!("GarbageCollect: [FileId({inode})]");
    debug!("waiting for GC event with specific inode to retain: '{msg}'");
    common::wait_for_event(msg.as_str(), &mut stderr_reader);
    debug!("got GC event");

    consume_output(stderr_reader.into_inner());

    common::assert_agent_running(&mut agent_handle);

    agent_handle.kill().expect("Could not kill process");
}

#[test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
#[cfg(any(target_os = "windows", target_os = "linux"))]
fn test_fs_rescan_on_initial_log_dir_delete() {
    let base_dir = tempdir().expect("Could not create temp dir").into_path();

    let dir_path = base_dir.join("initial_log_dir");
    std::fs::create_dir(dir_path.clone()).expect("Unable to create dir");
    let dir_path_str = dir_path.to_str().unwrap();

    let mut agent_handle = common::spawn_agent(AgentSettings::new(dir_path_str));
    let mut reader = BufReader::new(agent_handle.stderr.take().unwrap());

    common::wait_for_event("Enabling filesystem", &mut reader);

    Command::new("rmdir")
        .arg(dir_path)
        .spawn()
        .expect("Could not remove directory");

    common::wait_for_event("rescanning stream", &mut reader);

    common::assert_agent_running(&mut agent_handle);

    agent_handle.kill().expect("Could not kill process");
}

#[test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
#[cfg(any(target_os = "windows", target_os = "linux"))]
fn test_fs_rescan_on_initial_log_dir_create() {
    let base_dir = tempdir().expect("Could not create temp dir").into_path();

    let dir_path = base_dir.join("initial_log_dir");
    let dir_path_str = dir_path.to_str().unwrap();

    let mut agent_handle = common::spawn_agent(AgentSettings::new(dir_path_str));
    let mut reader = BufReader::new(agent_handle.stderr.take().unwrap());

    common::wait_for_event("Enabling filesystem", &mut reader);

    std::fs::create_dir(dir_path.clone()).expect("Unable to create dir");

    common::wait_for_event("rescanning stream", &mut reader);

    common::assert_agent_running(&mut agent_handle);

    agent_handle.kill().expect("Could not kill process");
}

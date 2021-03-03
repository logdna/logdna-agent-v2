use crate::common::AgentSettings;
use assert_cmd::prelude::*;
use log::debug;
use predicates::prelude::*;
use std::fs;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::Command;
use std::thread::{self, sleep};
use std::time::Duration;
use systemd::journal;
use tempfile::tempdir;

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
    let mut agent_handle = common::spawn_agent(AgentSettings::new(&dir.to_str().unwrap()));

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

    let mut agent_handle = common::spawn_agent(AgentSettings::new(&dir.to_str().unwrap()));

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

    let mut agent_handle = common::spawn_agent(AgentSettings::new(&dir.to_str().unwrap()));
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

    let mut agent_handle = common::spawn_agent(AgentSettings::new(&dir.to_str().unwrap()));
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
    thread::sleep(std::time::Duration::from_millis(200));

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
fn test_append_and_move() {
    let dir = tempdir().expect("Could not create temp dir").into_path();
    let file1_path = dir.join("file1.log");
    let file2_path = dir.join("file2.log");
    File::create(&file1_path).expect("Could not create file");

    let mut agent_handle = common::spawn_agent(AgentSettings::new(&dir.to_str().unwrap()));
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

    let mut agent_handle = common::spawn_agent(AgentSettings::new(&dir.to_str().unwrap()));

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
        log_dirs: &dir.to_str().unwrap(),
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

    let mut agent_handle = common::spawn_agent(AgentSettings::new(&dir.to_str().unwrap()));
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
            format!("{} should not been included", file_name)
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

    let mut agent_handle = common::spawn_agent(AgentSettings::new(&log_dir.to_str().unwrap()));
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

    let mut agent_handle = common::spawn_agent(AgentSettings::new(&log_dir.to_str().unwrap()));
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

    let mut agent_handle = common::spawn_agent(AgentSettings::new(&log_dir.to_str().unwrap()));
    let mut stderr_reader = BufReader::new(agent_handle.stderr.as_mut().unwrap());

    std::os::unix::fs::symlink(&dir_1_path, &symlink_path).unwrap();

    common::wait_for_file_event("initialized", &file1_path, &mut stderr_reader);

    common::append_to_file(&file1_path, 1_000, 50).expect("Could not append");
    common::append_to_file(&file2_path, 1_000, 50).expect("Could not append");
    common::append_to_file(&file3_path, 1_000, 50).expect("Could not append");

    fs::remove_file(&symlink_path).expect("Could not remove symlink");

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
        tokio::time::delay_for(tokio::time::Duration::from_millis(500)).await;

        let map = received.lock().await;
        let file_info = map.values().next().unwrap();
        assert!(predicate::str::contains("Sample alert").eval(file_info.value.as_str()));
        assert!(predicate::str::contains("Sample info").eval(file_info.value.as_str()));

        shutdown_handle();
    });

    server_result.unwrap();
    common::assert_agent_running(&mut agent_handle);
    agent_handle.kill().expect("Could not kill process");
}

#[test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
fn lookback_start_lines_are_delivered() {
    let _ = env_logger::Builder::from_default_env().try_init();
    let dir = tempdir().expect("Couldn't create temp dir...");

    let dir_path = format!("{}/", dir.path().to_str().unwrap());
    let (server, received, shutdown_handle, cert_file, addr) = common::self_signed_https_ingester();
    let log_lines = "This is a test log line";

    let file_path = dir.path().join("test.log");
    let mut file = File::create(&file_path).expect("Couldn't create temp log file...");

    // Enough bytes to get past the lookback threshold
    let line_write_count = (8192 / (log_lines.as_bytes().len() + 1)) + 1;

    (0..line_write_count)
        .for_each(|_| writeln!(file, "{}", log_lines).expect("Couldn't write to temp log file..."));
    file.sync_all().expect("Failed to sync file");

    let mut handle = common::spawn_agent(AgentSettings {
        log_dirs: &dir_path,
        exclusion_regex: Some(r"/var\w*"),
        ssl_cert_file: Some(cert_file.path()),
        lookback: Some("start"),
        host: Some(&addr),
        ..Default::default()
    });

    // Dump the agent's stdout
    // TODO: assert that it's successfully uploaded

    thread::sleep(std::time::Duration::from_secs(1));

    tokio_test::block_on(async {
        let (line_count, _, server) = tokio::join!(
            async {
                tokio::time::delay_for(tokio::time::Duration::from_millis(5000)).await;
                let mut output = String::new();

                handle.kill().unwrap();
                let stderr_ref = handle.stderr.as_mut().unwrap();

                stderr_ref.read_to_string(&mut output).unwrap();

                debug!("{}", output);
                let line_count = received
                    .lock()
                    .await
                    .get(file_path.to_str().unwrap())
                    .unwrap()
                    .lines;
                shutdown_handle();

                handle.wait().unwrap();
                line_count
            },
            async move {
                tokio::time::delay_for(tokio::time::Duration::from_millis(500)).await;
                (0..5).for_each(|_| {
                    writeln!(file, "{}", log_lines).expect("Couldn't write to temp log file...");
                    file.sync_all().expect("Failed to sync file");
                });
                tokio::time::delay_for(tokio::time::Duration::from_millis(500)).await;
                // Hack to drive stream forward
                writeln!(file, "{}", log_lines).expect("Couldn't write to temp log file...");
                file.sync_all().expect("Failed to sync file");
            },
            server
        );
        server.unwrap();
        assert_eq!(line_count, line_write_count + 5);
    });
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

    // Enough bytes to get past the lookback threshold
    let line_write_count = (8192 / (log_lines.as_bytes().len() + 1)) + 1;
    (0..line_write_count)
        .for_each(|_| writeln!(file, "{}", log_lines).expect("Couldn't write to temp log file..."));
    file.sync_all().expect("Failed to sync file");

    let mut handle = common::spawn_agent(AgentSettings {
        log_dirs: &dir_path,
        ssl_cert_file: Some(cert_file.path()),
        host: Some(&addr),
        ..Default::default()
    });

    // Dump the agent's stdout
    // TODO: assert that it's successfully uploaded

    thread::sleep(std::time::Duration::from_secs(1));
    tokio_test::block_on(async {
        let (line_count, _, server) = tokio::join!(
            async {
                tokio::time::delay_for(tokio::time::Duration::from_millis(5000)).await;

                let mut output = String::new();

                handle.kill().unwrap();
                let stderr_ref = handle.stderr.as_mut().unwrap();

                stderr_ref.read_to_string(&mut output).unwrap();

                debug!("{}", output);
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
                tokio::time::delay_for(tokio::time::Duration::from_millis(500)).await;
                (0..5).for_each(|_| {
                    writeln!(file, "{}", log_lines).expect("Couldn't write to temp log file...");
                    file.sync_all().expect("Failed to sync file");
                });
                tokio::time::delay_for(tokio::time::Duration::from_millis(500)).await;
                // Hack to drive stream forward
                writeln!(file, "{}", log_lines).expect("Couldn't write to temp log file...");
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
async fn test_tags() {
    let dir = tempdir().expect("Couldn't create temp dir...").into_path();
    let (server, received, shutdown_handle, addr) = common::start_http_ingester();
    let file_path = dir.join("test.log");
    let tag = "my-tag";
    File::create(&file_path).expect("Couldn't create temp log file...");
    let mut settings = AgentSettings::with_mock_ingester(&dir.to_str().unwrap(), &addr);
    settings.tags = Some(tag);
    let mut agent_handle = common::spawn_agent(settings);
    let mut stderr_reader = BufReader::new(agent_handle.stderr.as_mut().unwrap());
    common::wait_for_file_event("initialized", &file_path, &mut stderr_reader);

    let (server_result, _) = tokio::join!(server, async {
        common::append_to_file(&file_path, 10, 5).unwrap();
        common::force_client_to_flush(&dir).await;

        // Wait for the data to be received by the mock ingester
        tokio::time::delay_for(tokio::time::Duration::from_millis(500)).await;

        let map = received.lock().await;
        let file_info = map.get(file_path.to_str().unwrap()).unwrap();
        assert_eq!(file_info.lines, 10);
        assert_eq!(file_info.tags, Some(tag.to_string()));
        shutdown_handle();
    });

    server_result.unwrap();
    agent_handle.kill().expect("Could not kill process");
}

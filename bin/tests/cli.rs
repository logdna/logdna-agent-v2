use assert_cmd::prelude::*;
use predicates::prelude::*;

use tempfile::tempdir;

use std::fs;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::Command;
use std::thread;

mod common;

#[test]
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
    let mut file = File::create(&before_file_path).expect("Couldn't create temp log file...");

    let mut handle = common::spawn_agent(&dir_path);
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

    // Check that the agent logs that it has sent lines from each file
    assert!(predicate::str::contains(&format!(
        "watching \"{}\"",
        before_file_path.to_str().unwrap()
    ))
    .eval(&output));
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
    let mut agent_handle = common::spawn_agent(&dir.to_str().unwrap());

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

    let mut agent_handle = common::spawn_agent(&dir.to_str().unwrap());

    let mut stderr_reader = BufReader::new(agent_handle.stderr.as_mut().unwrap());

    common::wait_for_file_event("initialized", &file_path, &mut stderr_reader);
    common::append_to_file(&file_path, 10_000, 50).expect("Could not append");
    fs::remove_file(&file_path).expect("Could not remove file");

    // Immediately, start appending in a new file
    common::append_to_file(&file_path, 5, 5).expect("Could not append");

    common::wait_for_file_event("unwatching", &file_path, &mut stderr_reader);
    common::wait_for_file_event("added", &file_path, &mut stderr_reader);

    for _ in 0..5 {
        assert!(agent_handle.try_wait().ok().unwrap().is_none());
        thread::sleep(std::time::Duration::from_millis(20));
    }

    agent_handle.kill().expect("Could not kill process");
}

#[test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
fn test_append_and_move() {
    let dir = tempdir().expect("Could not create temp dir").into_path();
    let file1_path = dir.join("file1.log");
    let file2_path = dir.join("file2.log");
    File::create(&file1_path).expect("Could not create file");

    let mut agent_handle = common::spawn_agent(&dir.to_str().unwrap());
    let mut stderr_reader = BufReader::new(agent_handle.stderr.as_mut().unwrap());

    common::wait_for_file_event("initialized", &file1_path, &mut stderr_reader);
    common::append_to_file(&file1_path, 10_000, 50).expect("Could not append");
    fs::rename(&file1_path, &file2_path).expect("Could not move file");
    fs::remove_file(&file2_path).expect("Could not remove file");

    // Immediately, start appending in a new file
    common::append_to_file(&file1_path, 5, 5).expect("Could not append");

    // Should be added back
    common::wait_for_file_event("added", &file1_path, &mut stderr_reader);

    thread::sleep(std::time::Duration::from_millis(20));

    // Verify that the agent is still running
    assert!(agent_handle.try_wait().ok().unwrap().is_none());

    agent_handle.kill().expect("Could not kill process");
}

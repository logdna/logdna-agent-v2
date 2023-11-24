use predicate::str::{contains, is_match};
use predicates::prelude::predicate;
use predicates::Predicate;
use std::fs::{self, File};
use std::io;
use std::io::BufRead;
use std::io::Write;
use std::ops::{Deref, DerefMut};
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::str::from_utf8;
use std::sync::{Arc, Condvar, Mutex};
#[cfg(any(target_os = "windows", target_os = "linux"))]
use std::thread;
use std::time::Duration;
use tempfile::tempdir;
use tracing::debug;

use test_log::test;

#[test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
fn test_command_line_arguments_help() {
    let mut cmd = get_bin_command();
    let output: Output = cmd.env_clear().arg("--help").unwrap();
    assert!(output.status.success());
    let output = from_utf8(&output.stdout).unwrap();
    debug!("agent output:\n{:?}", output);

    vec![
        // Check the version is printed in the help
        "LogDNA Agent 3.",
        "A resource-efficient log collection agent that forwards logs to LogDNA",
        "The config filename",
        "The endpoint to forward logs to",
        "The host to forward logs to",
        "Adds log directories to scan, in addition to the default",
        // Verify short options
        "-k, --key",
        "The ingestion key associated with your LogDNA account",
        "-d, --logdir",
        "Adds log directories to scan, in addition to the default",
        "-t, --tags",
        "List of tags metadata to attach to lines forwarded from this agent",
        // Verify long only options
        "--db-path",
        "--endpoint-path",
        "--exclude-regex",
        "--exclude",
        "--gzip-level",
        "--host",
        "--include-regex",
        "--include",
        "--ingest-buffer-size",
        "--retry-dir",
        "--retry-disk-limit",
        "--ingest-timeout",
        "--ip",
        "--journald-paths",
        "--line-exclusion",
        "--line-inclusion",
        "--line-redact",
        "--log-k8s-events",
        "--lookback",
        "--mac-address",
        "--metrics-port",
        "--os-hostname",
        "--use-compression",
        "--use-k8s-enrichment",
        "--use-ssl",
    ]
    .iter()
    .for_each(|m| {
        assert!(contains(*m).eval(output), "Not found: {}", *m);
    });
}

#[test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
fn test_version_is_included() {
    let mut cmd = get_bin_command();
    let output = cmd.env_clear().arg("--version").unwrap();
    assert!(output.status.success());
    let output = from_utf8(&output.stdout).unwrap();
    assert_eq!(
        output,
        format!("LogDNA Agent {}\n", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
fn test_list_config_from_conf() -> io::Result<()> {
    let config_dir = tempdir()?;
    let config_file_path = config_dir.path().join("sample.conf");
    let mut file = File::create(&config_file_path)?;
    write!(file, "key = 1234567890\ntags = production")?;

    let mut cmd = get_bin_command();

    let output: Output = cmd
        .env_clear()
        .args(["-c", config_file_path.to_str().unwrap()])
        .arg("-l")
        .unwrap();
    assert!(output.status.success());
    let output = from_utf8(&output.stderr).unwrap();

    [
        "using settings from config file, environment variables and command line options",
        "ingestion_key: REDACTED",
        "tags: production",
        config_file_path.to_string_lossy().as_ref(),
    ]
    .iter()
    .for_each(|m| {
        assert!(contains(*m).eval(output));
    });

    Ok(())
}

#[test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
#[cfg(target_os = "linux")]
fn test_legacy_and_new_confs_merge() -> io::Result<()> {
    // Setting up an automatic finalizer for the test case that deletes the conf
    // files created in this test from their global directories. If they remain,
    // other integration tests may fail.
    struct Finalizer;
    impl Drop for Finalizer {
        fn drop(&mut self) {
            fs::remove_file(Path::new("/etc/logdna.conf")).unwrap_or(());
            fs::remove_file(Path::new("/etc/logdna/config.yaml")).unwrap_or(());
        }
    }
    let _defer_cleanup = Finalizer;

    let legacy_conf_path = Path::new("/etc/logdna.conf");
    let mut legacy_conf_file = File::create(legacy_conf_path)?;
    write!(
        legacy_conf_file,
        "key = 1234567890\nhost = legacyhost.logdna.test\nexclude_regex = /a/regex/exclude"
    )?;

    let new_conf_path = Path::new("/etc/logdna");
    fs::create_dir_all(new_conf_path)?;
    let new_conf_path = new_conf_path.join("config.yaml");
    let mut new_conf_file = File::create(new_conf_path)?;
    write!(
        new_conf_file,
        "
http:
  host: newhost.logdna.test
  ingestion_key: 9876543210
  params:
    hostname: abcd1234
    tags: newtag
    now: 0
log:
  dirs:
    - /var/log1/
journald: {{}}
startup: {{}}
"
    )?;

    let mut cmd = get_bin_command();
    let output: Output = cmd.env_clear().arg("-l").unwrap();
    assert!(output.status.success());

    let output = from_utf8(&output.stderr).unwrap();
    assert!(contains(
        "using settings from default config file, environment variables and command line options"
    )
    .eval(output));
    assert!(contains("host: newhost.logdna.test").eval(output));
    assert!(contains("- /a/regex/exclude").eval(output));

    Ok(())
}

#[test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
fn test_ibm_legacy_host_env_var() {
    let mut cmd = get_bin_command();
    let output: Output = cmd
        .env_clear()
        .env("LOGDNA_LOGHOST", "other.api.logdna.test")
        .arg("-l")
        .unwrap();

    assert!(output.status.success());

    let output = from_utf8(&output.stderr).unwrap();
    assert!(contains("host: other.api.logdna.test").eval(output));
}

#[test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
fn test_list_config_from_env() -> io::Result<()> {
    let mut cmd = get_bin_command();
    let output: Output = cmd
        .env_clear()
        .env("LOGDNA_INGESTION_KEY", "abc")
        .env("LOGDNA_TAGS", "sample_env_tag")
        .arg("-l")
        .unwrap();
    assert!(output.status.success());
    let output = from_utf8(&output.stderr).unwrap();
    assert!(
        contains("using settings from environment variables and command line options").eval(output)
    );
    assert!(contains("ingestion_key: REDACTED").eval(output));
    assert!(contains("tags: sample_env_tag").eval(output));

    Ok(())
}

#[test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
fn test_list_config_no_options() -> io::Result<()> {
    let mut cmd = get_bin_command();
    let result = cmd.env_clear().arg("-l").output();
    assert!(result.is_ok());
    let output = result.unwrap();
    // config is missing key
    assert!(!output.status.success());
    let output = from_utf8(&output.stderr).unwrap();
    assert!(
        contains("using settings from environment variables and command line options").eval(output)
    );
    assert!(contains("Configuration error: http.ingestion_key is missing").eval(output));

    Ok(())
}

#[test]
#[cfg_attr(not(all(target_os = "linux", feature = "integration_tests")), ignore)]
fn test_list_default_conf() -> io::Result<()> {
    let file_path = Path::new("/etc/logdna.conf");
    fs::write(file_path, "key = 1234\ntags = sample_tag_on_conf")?;

    let mut cmd = get_bin_command();
    let output: Output = cmd.env_clear().arg("-l").unwrap();

    // Remove file before any assert
    fs::remove_file(file_path)?;

    assert!(output.status.success());
    let output = from_utf8(&output.stderr).unwrap();
    assert!(contains(
        "using settings from default config file, environment variables and command line options"
    )
    .eval(output));
    assert!(contains("ingestion_key: REDACTED").eval(output));
    assert!(contains("tags: sample_tag_on_conf").eval(output));

    Ok(())
}

#[test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
fn test_command_line_arguments_should_set_config() {
    test_command(
        |cmd| {
            cmd.args(["-k", "my_secret"])
                .args(["-d", "/d1/", "/d2/"])
                .args(["-t", "a", "b"])
                .args(["--host", "remotehost"])
                .args(["--endpoint-path", "/path/to/endpoint"])
                .args(["--use-ssl", "true"])
                .args(["--use-compression", "false"])
                .args(["--gzip-level", "3"])
                .args(["--os-hostname", "os_host_name_sample"])
                .args(["--ip", "1.2.3.4"])
                .args(["--mac-address", "01-23-45-67-89-AB-CD-EF"])
                .args(["--exclude", "a.*"])
                .args(["--exclude-regex", "b.*"])
                .args(["--include", "c.*"])
                .args(["--include-regex", "d.*"])
                .args(["--journald-paths", "/run/systemd/journal"])
                .args(["--lookback", "start"])
                .args(["--use-k8s-enrichment", "never"])
                .args(["--log-k8s-events", "always"])
                .args(["--db-path", "/var/lib/some-agent/"])
                .args(["--metrics-port", "9898"])
                .args(["--line-exclusion", "abc"])
                .args(["--line-inclusion", "z_inc"])
                .args(["--line-redact", "a@b.com"])
                .args(["--ingest-buffer-size", "123456"])
                .args(["--ingest-timeout", "9876"])
                .args(["--retry-dir", "/tmp/logdna/argv"])
                .args(["--retry-disk-limit", "9 MB"]);
        },
        |d| {
            debug!("agent output: {:#?}", d);
            assert!(contains("tags: a,b").eval(d));
            assert!(is_match(r"log:\s+dirs:\s+\- /var/log/\s+\- /d1/\s+\- /d2/")
                .unwrap()
                .eval(d));
            assert!(contains("host: remotehost").eval(d));
            assert!(contains("endpoint: /path/to/endpoint").eval(d));
            assert!(contains("use_ssl: true").eval(d));
            assert!(contains("use_compression: false").eval(d));
            assert!(contains("gzip_level: 3").eval(d));
            assert!(contains("hostname: os_host_name_sample").eval(d));
            assert!(contains("ip: 1.2.3.4").eval(d));
            assert!(contains("mac: 01-23-45-67-89-AB-CD-EF").eval(d));
            assert!(contains("lookback: start").eval(d));
            assert!(contains("db_path: /var/lib/some-agent/").eval(d));
            assert!(contains("metrics_port: 9898").eval(d));
            assert!(contains("use_k8s_enrichment: never").eval(d));
            assert!(contains("log_k8s_events: always").eval(d));
            assert!(contains("a.*").eval(d));
            assert!(contains("b.*").eval(d));
            assert!(contains("c.*").eval(d));
            assert!(contains("d.*").eval(d));
            assert!(is_match(r"line_exclusion_regex:\s+\- abc").unwrap().eval(d));
            assert!(is_match(r"line_inclusion_regex:\s+\- z_inc")
                .unwrap()
                .eval(d));
            assert!(is_match(r"line_redact_regex:\s+\- a@b.com")
                .unwrap()
                .eval(d));
            assert!(contains("body_size: 123456").eval(d));
            assert!(contains("timeout: 9876").eval(d));
            assert!(contains("retry_dir: /tmp/logdna/argv").eval(d));
            assert!(contains("retry_disk_limit: 9000000").eval(d));
        },
    );
}

#[test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
fn test_invalid_command_line_arguments_should_exit() {
    cmd_line_invalid_test(&["--use-ssl", "ZZZ"]);
    cmd_line_invalid_test(&["--use-compression", "ZZZ"]);
    cmd_line_invalid_test(&["--gzip-level", "ZZZ"]);
    cmd_line_invalid_test(&["--metrics-port", "ZZZ"]);
    cmd_line_invalid_test(&["--lookback", "ZZZ"]);
    cmd_line_invalid_test(&["--use-k8s-enrichment", "ZZZ"]);
    cmd_line_invalid_test(&["--log-k8s-events", "ZZZ"]);
}

#[test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
fn test_environment_variables_should_set_config() {
    test_command(
        |cmd| {
            cmd.env("MZ_INGESTION_KEY", "123")
                .env("MZ_LOG_DIRS", "/d3/,/d4/")
                .env("MZ_TAGS", "d, e, f ")
                .env("MZ_HOST", "my_server")
                .env("MZ_ENDPOINT", "/path/endpoint")
                .env("MZ_USE_SSL", "false")
                .env("MZ_USE_COMPRESSION", "true")
                .env("MZ_GZIP_LEVEL", "1")
                .env("MZ_EXCLUSION_RULES", "w.*")
                .env("MZ_EXCLUSION_REGEX_RULES", "x.*")
                .env("MZ_INCLUSION_RULES", "y.*")
                .env("MZ_INCLUSION_REGEX_RULES", "z.*")
                .env("MZ_HOSTNAME", "my_os_host")
                .env("MZ_IP", "8.8.8.8")
                .env("MZ_MAC", "22-23-45-67-89-AB-CD-EF")
                .env("MZ_JOURNALD_PATHS", "/j/d")
                .env("MZ_LOOKBACK", "none")
                .env("MZ_DB_PATH", "/var/lib/some-other-folder/")
                .env("MZ_METRICS_PORT", "9097")
                .env("MZ_USE_K8S_LOG_ENRICHMENT", "always")
                .env("MZ_LOG_K8S_EVENTS", "never")
                .env("MZ_LINE_EXCLUSION_REGEX", "exc_re")
                .env("MZ_LINE_INCLUSION_REGEX", "inc_re")
                .env("MZ_REDACT_REGEX", "c@d.com")
                .env("MZ_INGEST_TIMEOUT", "123456")
                .env("MZ_INGEST_BUFFER_SIZE", "987654")
                .env("MZ_RETRY_DIR", "/tmp/logdna/env")
                .env("MZ_RETRY_DISK_LIMIT", "7 KB");
        },
        |d| {
            assert!(!d.is_empty());
            assert!(contains("tags: d,e,f").eval(d), "{}", d);
            assert!(is_match(r"log:\s+dirs:\s+\- /var/log/\s+\- /d3/\s+\- /d4/")
                .unwrap()
                .eval(d));
            assert!(contains("host: my_server").eval(d));
            assert!(contains("endpoint: /path/endpoint").eval(d));
            assert!(contains("use_ssl: false").eval(d));
            assert!(contains("use_compression: true").eval(d));
            assert!(contains("gzip_level: 1").eval(d));
            assert!(contains("hostname: my_os_host").eval(d));
            assert!(contains("ip: 8.8.8.8").eval(d));
            assert!(contains("mac: 22-23-45-67-89-AB-CD-EF").eval(d));
            assert!(contains("lookback: none").eval(d));
            assert!(contains("db_path: /var/lib/some-other-folder/").eval(d));
            assert!(contains("metrics_port: 9097").eval(d));
            assert!(contains("j/d").eval(d));
            assert!(contains("use_k8s_enrichment: always").eval(d));
            assert!(contains("log_k8s_events: never").eval(d));
            assert!(contains("w.*").eval(d));
            assert!(contains("x.*").eval(d));
            assert!(contains("y.*").eval(d));
            assert!(contains("z.*").eval(d));
            assert!(contains("j/d").eval(d));
            assert!(is_match(r"line_exclusion_regex:\s+\- exc_re")
                .unwrap()
                .eval(d));
            assert!(is_match(r"line_inclusion_regex:\s+\- inc_re")
                .unwrap()
                .eval(d));
            assert!(is_match(r"line_redact_regex:\s+\- c@d.com")
                .unwrap()
                .eval(d));
            assert!(contains("timeout: 123456").eval(d));
            assert!(contains("body_size: 987654").eval(d));
            assert!(contains("retry_dir: /tmp/logdna/env").eval(d));
            assert!(contains("retry_disk_limit: 7000").eval(d));
        },
    );
}

#[test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
fn test_logdna_environment_variables_should_set_config() {
    test_command(
        |cmd| {
            cmd.env("LOGDNA_INGESTION_KEY", "123")
                .env("LOGDNA_LOG_DIRS", "/d3/,/d4/")
                .env("LOGDNA_TAGS", "d, e, f ")
                .env("LOGDNA_HOST", "my_server")
                .env("LOGDNA_ENDPOINT", "/path/endpoint")
                .env("LOGDNA_USE_SSL", "false")
                .env("LOGDNA_USE_COMPRESSION", "true")
                .env("LOGDNA_GZIP_LEVEL", "1")
                .env("LOGDNA_EXCLUSION_RULES", "w.*")
                .env("LOGDNA_EXCLUSION_REGEX_RULES", "x.*")
                .env("LOGDNA_INCLUSION_RULES", "y.*")
                .env("LOGDNA_INCLUSION_REGEX_RULES", "z.*")
                .env("LOGDNA_HOSTNAME", "my_os_host")
                .env("LOGDNA_IP", "8.8.8.8")
                .env("LOGDNA_MAC", "22-23-45-67-89-AB-CD-EF")
                .env("LOGDNA_JOURNALD_PATHS", "/j/d")
                .env("LOGDNA_LOOKBACK", "none")
                .env("LOGDNA_DB_PATH", "/var/lib/some-other-folder/")
                .env("LOGDNA_METRICS_PORT", "9097")
                .env("LOGDNA_USE_K8S_LOG_ENRICHMENT", "always")
                .env("LOGDNA_LOG_K8S_EVENTS", "never")
                .env("LOGDNA_LINE_EXCLUSION_REGEX", "exc_re")
                .env("LOGDNA_LINE_INCLUSION_REGEX", "inc_re")
                .env("LOGDNA_REDACT_REGEX", "c@d.com")
                .env("LOGDNA_INGEST_TIMEOUT", "123456")
                .env("LOGDNA_INGEST_BUFFER_SIZE", "987654")
                .env("LOGDNA_RETRY_DIR", "/tmp/logdna/env")
                .env("LOGDNA_RETRY_DISK_LIMIT", "7 KB");
        },
        |d| {
            assert!(contains("tags: d,e,f").eval(d));
            assert!(is_match(r"log:\s+dirs:\s+\- /var/log/\s+\- /d3/\s+\- /d4/")
                .unwrap()
                .eval(d));
            assert!(contains("host: my_server").eval(d));
            assert!(contains("endpoint: /path/endpoint").eval(d));
            assert!(contains("use_ssl: false").eval(d));
            assert!(contains("use_compression: true").eval(d));
            assert!(contains("gzip_level: 1").eval(d));
            assert!(contains("hostname: my_os_host").eval(d));
            assert!(contains("ip: 8.8.8.8").eval(d));
            assert!(contains("mac: 22-23-45-67-89-AB-CD-EF").eval(d));
            assert!(contains("lookback: none").eval(d));
            assert!(contains("db_path: /var/lib/some-other-folder/").eval(d));
            assert!(contains("metrics_port: 9097").eval(d));
            assert!(contains("j/d").eval(d));
            assert!(contains("use_k8s_enrichment: always").eval(d));
            assert!(contains("log_k8s_events: never").eval(d));
            assert!(contains("w.*").eval(d));
            assert!(contains("x.*").eval(d));
            assert!(contains("y.*").eval(d));
            assert!(contains("z.*").eval(d));
            assert!(contains("j/d").eval(d));
            assert!(is_match(r"line_exclusion_regex:\s+\- exc_re")
                .unwrap()
                .eval(d));
            assert!(is_match(r"line_inclusion_regex:\s+\- inc_re")
                .unwrap()
                .eval(d));
            assert!(is_match(r"line_redact_regex:\s+\- c@d.com")
                .unwrap()
                .eval(d));
            assert!(contains("timeout: 123456").eval(d));
            assert!(contains("body_size: 987654").eval(d));
            assert!(contains("retry_dir: /tmp/logdna/env").eval(d));
            assert!(contains("retry_disk_limit: 7000").eval(d));
        },
    );
}

#[test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
fn test_deprecated_environment_variables_should_set_config() {
    test_command(
        |cmd| {
            cmd.env("LOGDNA_AGENT_KEY", "123")
                .env("LDLOGHOST", "my_server2")
                .env("LDLOGPATH", "/path2")
                .env("LDLOGSSL", "false")
                .env("COMPRESS", "true")
                .env("GZIP_COMPRESS_LEVEL", "4")
                .env("LOG_DIRS", "/dd/,/de/")
                .env("LOGDNA_EXCLUDE", "j.*")
                .env("LOGDNA_EXCLUDE_REGEX", "k.*")
                .env("LOGDNA_INCLUDE", "l.*")
                .env("LOGDNA_INCLUDE_REGEX", "m.*");
        },
        |d| {
            assert!(is_match(r"log:\s+dirs:\s+\- /var/log/\s+\- /dd/\s+\- /de/")
                .unwrap()
                .eval(d));
            assert!(contains("host: my_server2").eval(d));
            assert!(contains("endpoint: /path2").eval(d));
            assert!(contains("use_ssl: false").eval(d));
            assert!(contains("use_compression: true").eval(d));
            assert!(contains("gzip_level: 4").eval(d));
            assert!(contains("j.*").eval(d));
            assert!(contains("k.*").eval(d));
            assert!(contains("l.*").eval(d));
            assert!(contains("m.*").eval(d));
        },
    );
}

#[test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
fn test_command_line_arguments_should_merge_config_from_file() {
    let config_dir = tempdir().unwrap().into_path();
    let config_file_path = config_dir.join("config.yaml");
    let mut file = File::create(&config_file_path).unwrap();
    write!(
        file,
        "
http:
  host: logs.logdna.prod
  endpoint: /path/to/endpoint
  use_ssl: false
  timeout: 10000
  use_compression: false
  gzip_level: 4
  params:
    hostname: abc
    tags: tag1
    now: 0
  body_size: 2097152
log:
  dirs:
    - /var/log1/
  include:
    glob:
      - \"*.log\"
    regex: []
  exclude:
    glob:
      - /var/log/wtmp
      - /var/log/btmp
    regex: []
  use_k8s_enrichment: always
  log_k8s_events: always
journald: {{}}
startup: {{}}
    "
    )
    .unwrap();

    test_command(
        |cmd| {
            cmd.args(["-k", "123"])
                .args(["-c", config_file_path.to_str().unwrap()])
                .args(["-d", "/var/log2/,/var/log3/"])
                .args(["-t", "tag2", "tag3"])
                .args(["--exclude", "file.log"])
                .args(["--include", "file.zlog"]);
        },
        |d| {
            assert!(
                is_match(r"log:\s+dirs:\s+\- /var/log1/\s+\- /var/log2/\s+\- /var/log3/")
                    .unwrap()
                    .eval(d)
            );
            assert!(contains("host: logs.logdna.prod").eval(d));
            assert!(contains("endpoint: /path/to/endpoint").eval(d));
            assert!(contains("use_ssl: false").eval(d));
            assert!(contains("use_compression: false").eval(d));
            assert!(contains("gzip_level: 4").eval(d));
            assert!(is_match(
                r"exclude:\s+glob:\s+\- /var/log/wtmp\s+\- /var/log/btmp\s+\- file\.log"
            )
            .unwrap()
            .eval(d));
            assert!(is_match(r"include:\s+glob:\s+\- .\*\.log.\s+\- file\.zlog")
                .unwrap()
                .eval(d));
            assert!(contains("tags: tag1,tag2,tag3").eval(d));
        },
    );
}

#[test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
fn test_properties_config_min() -> io::Result<()> {
    let config_dir = tempdir()?;
    let config_file_path = config_dir.path().join("sample.conf");
    let mut file = File::create(&config_file_path)?;
    write!(file, "key = 1234567890")?;

    test_command(
        |cmd| {
            cmd.args(["-c", config_file_path.to_str().unwrap()]);
        },
        |d| {
            // Verify that it starts thanks to having the key is good enough
            assert!(is_match(r"log:\s+dirs:\s+\- /var/log/").unwrap().eval(d));
            assert!(contains("use_ssl: true").eval(d));
            assert!(contains("Enabling filesystem").eval(d));
        },
    );
    Ok(())
}

#[test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
fn test_properties_config_legacy() -> io::Result<()> {
    let config_dir = tempdir()?;
    let config_file_path = config_dir.path().join("sample.conf");
    let mut file = File::create(&config_file_path)?;
    write!(
        file,
        "
key = 1234567890
logdir = /var/my_log,/var/my_log2
tags = production, stable
exclude = /path/to/exclude/**, /second/path/to/exclude/**
exclude_regex = /a/regex/exclude
hostname = some-linux-instance"
    )?;

    test_command(
        |cmd| {
            cmd.args(["-c", config_file_path.to_str().unwrap()]);
        },
        |d| {
            assert!(is_match(r"log:\s+dirs:\s+\- /var/my_log\s+\- /var/my_log2")
                .unwrap()
                .eval(d));
            assert!(contains("tags: production,stable").eval(d));
            assert!(contains("hostname: some-linux-instance").eval(d));
            assert!(is_match(
                r"exclude:\s+glob:[^:]+- /path/to/exclude/\*\*\s+- /second/path/to/exclude/\*\*"
            )
            .unwrap()
            .eval(d));
            assert!(
                is_match(r"exclude:\s+glob:[^:]+regex:\s+- /a/regex/exclude")
                    .unwrap()
                    .eval(d)
            );
        },
    );
    Ok(())
}

#[test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
#[cfg(any(target_os = "windows", target_os = "linux"))] // This does work but is inconsistent, test wise not functionality on mac
fn test_properties_config_common() -> io::Result<()> {
    let config_dir = tempdir()?;
    let config_file_path = config_dir.path().join("sample.conf");
    let mut file = File::create(&config_file_path)?;
    write!(
        file,
        "
key = 1234567890
logdir = /var/log,/var/my_log2
tags = abc
exclude = /var/log/noisy/**/*, !(*sample*)
line_exclusion_regex = (?i:debug),(?i:trace)"
    )?;

    thread::sleep(std::time::Duration::from_millis(1000));

    test_command(
        |cmd| {
            cmd.args(["-c", config_file_path.to_str().unwrap()]);
        },
        |d| {
            assert!(is_match(r"log:\s+dirs:\s+\- /var/log\s+\- /var/my_log2")
                .unwrap()
                .eval(d));
            assert!(contains("tags: abc").eval(d));
            assert!(is_match(r"exclude:\s+glob:[^:]+- /var/log/noisy/\*\*/\*")
                .unwrap()
                .eval(d));
            assert!(
                is_match("line_exclusion_regex:\\s+- \\(\\?i:debug\\)")
                    .unwrap()
                    .eval(d),
                "{}",
                d
            );
        },
    );
    Ok(())
}

#[test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
#[cfg(target_os = "linux")]
fn test_properties_default_conf() -> io::Result<()> {
    let data = "key = 1234\ntags = sample_tag";
    let file_path = Path::new("/etc/logdna.conf");
    let mut conf_file = File::create(file_path)?;
    conf_file.write_all(data.as_bytes())?;
    conf_file.flush()?;

    test_command(
        |_| {
            // No command argument
        },
        |d| {
            fs::remove_file(file_path).unwrap();
            assert!(is_match(r"log:\s+dirs:\s+\- /var/log/").unwrap().eval(d));
            assert!(contains("tags: sample_tag").eval(d));
        },
    );
    Ok(())
}

#[test]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
#[cfg(target_os = "linux")]
fn test_properties_default_yaml() -> io::Result<()> {
    let dir = Path::new("/etc/logdna/");
    fs::create_dir_all(dir)?;
    let file_path = dir.join("config.yaml");
    fs::write(
        &file_path,
        "
http:
  ingestion_key: 0001020304
  host: logs.logdna.prod
  endpoint: /path/to/endpoint
  use_ssl: true
  timeout: 10000
  use_compression: true
  gzip_level: 2
  body_size: 2097152
  params:
    hostname: abc
    tags: tag1
    now: 0
log:
  dirs:
    - /var/log1/
journald: {}
startup: {}",
    )?;

    test_command(
        |_| {
            // No command argument
        },
        |d| {
            fs::remove_file(&file_path).unwrap();
            assert!(is_match("hostname: abc").unwrap().eval(d));
            assert!(contains("tags: tag1").eval(d));
        },
    );
    Ok(())
}

fn cmd_line_invalid_test(args: &[&str]) {
    let mut cmd = get_bin_command();
    match cmd
        .timeout(std::time::Duration::from_millis(500))
        .env_clear()
        .args(args)
        .args(["--key", "123"])
        .ok()
    {
        Ok(_) => panic!("it should have failed for {} but succeeded", args[0]),
        Err(error) => {
            let error = &error.to_string();
            assert!(
                contains("Invalid value for").eval(error),
                "when looking for {}",
                args[0]
            );
            assert!(contains(args[0]).eval(error));
        }
    }
}

fn test_command<CmdF, DataF>(cmd_f: CmdF, data_f: DataF)
where
    CmdF: Fn(&mut Command),
    DataF: Fn(&str),
{
    let mut cmd = crate::common::get_agent_command(None);
    cmd.env_clear()
        .env("RUST_LOG", "debug")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    // Set parameters
    cmd_f(&mut cmd);

    let mut handle = cmd.spawn().unwrap();
    let data = Arc::new((Mutex::new((true, String::new())), Condvar::new()));
    let d = data.clone();
    let stderr_reader = std::io::BufReader::new(handle.stderr.take().unwrap());

    std::thread::spawn(move || {
        let (lock, cvar) = &*d;
        for line in stderr_reader.lines() {
            let line = &line.unwrap();
            let mut guard = lock.lock().unwrap_or_else(|e| e.into_inner());
            let (pending, data) = guard.deref_mut();
            debug!("agent stderr: {}", line);
            data.push_str(line);
            data.push('\n');
            if data.contains("Enabling filesystem") {
                *pending = false;
                cvar.notify_one();
            };
        }
    });

    let stdout_reader = std::io::BufReader::new(handle.stdout.take().unwrap());

    std::thread::spawn(move || {
        for line in stdout_reader.lines() {
            let line = &line.unwrap();
            debug!("agent stdout: {}", line);
        }
    });

    // Wait for 30 seconds or until the agent has enabled the filesytem
    let (lock, cvar) = &*data;
    let guard = lock.lock().unwrap_or_else(|e| e.into_inner());
    let (guard, result) = cvar
        .wait_timeout_while(guard, Duration::from_secs(30), |&mut (pending, _)| pending)
        .unwrap();

    let (pending, data) = guard.deref();

    // Make sure the agent is still running
    assert!(
        matches!(handle.try_wait(), Ok(None)),
        "Process ended unexpectedly"
    );

    handle.kill().unwrap();
    handle.wait().unwrap();

    // Kill the agent
    if result.timed_out() || *pending {
        panic!("timed out waiting for agent to start: {}", *data);
    }

    debug!("Agent STDERR output:\n{}", data);
    // Validate data
    data_f(data.deref());
}

fn get_bin_command() -> assert_cmd::Command {
    assert_cmd::Command::from_std(crate::common::get_agent_command(None))
}

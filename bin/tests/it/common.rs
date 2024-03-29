use core::time;

use rand::seq::IteratorRandom;

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::{BufRead, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener};

use futures::Future;
use logdna_mock_ingester::{
    http_ingester, http_ingester_with_processors, https_ingester_with_processors, FileLineCounter,
    IngestError, ProcessFn, ReqFn,
};
use tracing::debug;

pub use logdna_mock_ingester::HttpVersion;

use once_cell::sync::Lazy;

use rcgen::generate_simple_self_signed;

type CargoCommandByFeatureMap = HashMap<Option<String>, Arc<Mutex<Option<escargot::CargoRun>>>>;
static AGENT_COMMANDS: Lazy<Mutex<CargoCommandByFeatureMap>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

pub static LINE: &str = "Nov 30 09:14:47 sample-host-name sampleprocess[1204]: Hello from process";

pub fn append_to_file(file_path: &Path, lines: i32, sync_every: i32) -> Result<(), std::io::Error> {
    let mut file = OpenOptions::new()
        .append(true)
        .create(true)
        .open(file_path)?;

    for i in 0..lines {
        if let Err(e) = writeln!(file, "{}", LINE) {
            eprintln!("Couldn't write to file: {}", e);
            return Err(e);
        }

        if i % sync_every == 0 {
            file.sync_all()?;
            thread::sleep(time::Duration::from_millis(50));
        }
    }
    file.sync_all()?;
    Ok(())
}

pub async fn force_client_to_flush(dir_path: &Path) {
    // Client flushing delay
    tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
    // Append to a dummy file
    append_to_file(&dir_path.join("force_flush.log"), 1, 1).unwrap();
}

#[cfg(not(target_os = "macos"))]
pub fn truncate_file(file_path: &Path) -> Result<(), std::io::Error> {
    OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(file_path)?;
    Ok(())
}

#[derive(Clone, Default)]
pub struct AgentSettings<'a> {
    pub log_dirs: &'a str,
    pub exclusion: Option<&'a str>,
    pub exclusion_regex: Option<&'a str>,
    pub features: Option<&'a str>,
    pub journald_dirs: Option<&'a str>,
    pub log_journal_d: Option<&'a str>,
    pub startup_lease: Option<&'a str>,
    pub ssl_cert_file: Option<&'a std::path::Path>,
    pub lookback: Option<&'a str>,
    pub host: Option<&'a str>,
    pub use_ssl: bool,
    pub ingester_key: Option<&'a str>,
    pub tags: Option<&'a str>,
    pub config_file: Option<&'a str>,
    pub state_db_dir: Option<&'a std::path::Path>,
    pub metrics_port: Option<u16>,
    pub line_exclusion_regex: Option<&'a str>,
    pub line_inclusion_regex: Option<&'a str>,
    pub line_redact_regex: Option<&'a str>,
    pub ingest_timeout: Option<&'a str>,
    pub ingest_buffer_size: Option<&'a str>,
    pub log_level: Option<&'a str>,
    pub clear_cache_interval: Option<u32>,
    pub metadata_retry_delay: Option<u32>,
}

impl<'a> AgentSettings<'a> {
    pub fn new(log_dirs: &'a str) -> Self {
        AgentSettings {
            log_dirs,
            exclusion_regex: Some(r"^/var.*"),
            use_ssl: true,
            log_journal_d: None,
            ..Default::default()
        }
    }

    pub fn with_mock_ingester(log_dirs: &'a str, server_address: &'a str) -> Self {
        AgentSettings {
            log_dirs,
            host: Some(server_address),
            use_ssl: false,
            ingester_key: Some("mock_key"),
            exclusion: Some("/var/log/**"),
            log_journal_d: None,
            ..Default::default()
        }
    }
}

pub fn get_agent_command(features: Option<String>) -> Command {
    let mut manifest_path = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    manifest_path.pop();
    let mut target_dir = manifest_path.clone();
    manifest_path.push("bin/Cargo.toml");
    target_dir.push("target");

    let feature_command_lock = {
        let agent_commands = AGENT_COMMANDS.lock();
        agent_commands
            .unwrap()
            .entry(features.clone())
            .or_insert_with(|| Arc::new(Mutex::new(None)))
            .clone()
    };

    let mut target_dir = std::env::var("CARGO_TARGET_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or(target_dir);
    if let Some(features) = features.clone() {
        target_dir.push(features.replace(',', ""))
    }

    let mut f_c = feature_command_lock.lock().unwrap();
    f_c.get_or_insert_with(|| {
        let mut cargo_build = escargot::CargoBuild::new()
            .target_dir(target_dir)
            .manifest_path(manifest_path)
            .bin("logdna-agent")
            .release()
            .current_target();

        if let Some(features) = features {
            cargo_build = cargo_build.no_default_features().features(features);
        }
        cargo_build.run().unwrap()
    })
    .command()
}

pub fn spawn_agent(settings: AgentSettings) -> Child {
    let features = settings.features.map(String::from);
    let mut cmd = get_agent_command(features);
    let ingestion_key = if let Some(key) = settings.ingester_key {
        key.to_string()
    } else {
        std::env::var("LOGDNA_INGESTION_KEY")
            .unwrap_or_else(|_| std::env::var("MZ_INGESTION_KEY").unwrap())
    };

    assert_ne!(ingestion_key, "", "Ingestion key not set. Set LOGDNA_INGESTION_KEY in your local env or update the test to use a mock ingestor.");

    let agent = cmd
        .env("RUST_LOG", settings.log_level.unwrap_or("debug"))
        .env("RUST_BACKTRACE", "full")
        .env("LOGDNA_LOG_DIRS", settings.log_dirs)
        .env("LOGDNA_INGESTION_KEY", ingestion_key)
        .env("MZ_FLUSH_DURATION", "250") // LOG-19388: always use short flush intervals
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    if let Some(cert_file_path) = settings.ssl_cert_file {
        agent.env("SSL_CERT_FILE", cert_file_path);
    } else {
        agent.env("LOGDNA_USE_SSL", settings.use_ssl.to_string());
    }

    if let Some(host) = settings.host {
        agent.env("LOGDNA_HOST", host);
    } else {
        agent.env(
            "LOGDNA_HOST",
            std::env::var("LOGDNA_HOST").expect("LOGDNA_HOST env var not set"),
        );
    }

    if let Some(lookback) = settings.lookback {
        agent.env("LOGDNA_LOOKBACK", lookback);
    }

    if let Some(state_db_dir) = settings.state_db_dir {
        agent.env("LOGDNA_DB_PATH", state_db_dir);
    }

    if let Some(port) = settings.metrics_port {
        agent.env("LOGDNA_METRICS_PORT", format!("{}", port));
    }

    if let Some(rules) = settings.exclusion {
        agent.env("LOGDNA_EXCLUSION_RULES", rules);
    }

    if let Some(rules) = settings.exclusion_regex {
        agent.env("LOGDNA_EXCLUSION_REGEX_RULES", rules);
    }

    if let Some(config_file) = settings.config_file {
        agent.env("LOGDNA_CONFIG_FILE", config_file);
    }

    if let Some(tags) = settings.tags {
        agent.env("LOGDNA_TAGS", tags);
    }

    if let Some(journald_dirs) = settings.journald_dirs {
        agent.env("LOGDNA_JOURNALD_PATHS", journald_dirs);
    }

    // Add in other config?
    if let Some(startup_lease) = settings.startup_lease {
        agent.env("LOGDNA_K8S_STARTUP_LEASE", startup_lease);
    }

    if let Some(regex_str) = settings.line_exclusion_regex {
        agent.env("LOGDNA_LINE_EXCLUSION_REGEX", regex_str);
    }

    if let Some(regex_str) = settings.line_inclusion_regex {
        agent.env("LOGDNA_LINE_INCLUSION_REGEX", regex_str);
    }

    if let Some(regex_str) = settings.line_redact_regex {
        agent.env("LOGDNA_REDACT_REGEX", regex_str);
    }

    if let Some(ingest_timeout) = settings.ingest_timeout {
        agent.env("LOGDNA_INGEST_TIMEOUT", ingest_timeout);
    }

    if let Some(ingest_buffer_size) = settings.ingest_buffer_size {
        agent.env("LOGDNA_INGEST_BUFFER_SIZE", ingest_buffer_size);
    }

    if let Some(log_journal_d) = settings.log_journal_d {
        agent.env("MZ_SYSTEMD_JOURNAL_TAILER", log_journal_d);
    }

    if let Some(clear_cache_interval) = settings.clear_cache_interval {
        agent.env(
            "LOGDNA_CLEAR_CACHE_INTERVAL",
            format!("{}", clear_cache_interval),
        );
    }

    if let Some(metadata_retry_delay) = settings.metadata_retry_delay {
        agent.env("METADATA_RETRY_DELAY", format!("{}", metadata_retry_delay));
    }

    agent.spawn().expect("Failed to start agent")
}

/// Blocks until a certain event referencing a file name is logged by the agent
pub fn wait_for_file_event(event: &str, file_path: &Path, reader: &mut dyn BufRead) -> String {
    let file_name = &file_path.file_name().unwrap().to_str().unwrap();
    wait_for_line(
        reader,
        event,
        |line| line.contains(event) && line.contains(file_name),
        None,
    )
}

/// Blocks until a certain event is logged by the agent
pub fn wait_for_event(event: &str, reader: &mut dyn BufRead) -> String {
    wait_for_line(reader, event, |line| line.contains(event), None)
}

fn wait_for_line<F>(
    reader: &mut dyn BufRead,
    event_info: &str,
    condition: F,
    delay: Option<std::time::Duration>,
) -> String
where
    F: Fn(&str) -> bool,
{
    let mut line = String::new();
    let mut lines_buffer = String::new();
    let instant = std::time::Instant::now();

    debug!("event info: {:?}", event_info);
    debug!("event info: {:?}", event_info);
    for _safeguard in 0..100_000 {
        assert!(
            instant.elapsed() < delay.unwrap_or(std::time::Duration::from_secs(20)),
            "Timed out waiting for condition"
        );

        reader.read_line(&mut line).unwrap();
        if line.is_empty() {
            continue;
        }
        debug!("{}", line.trim());

        lines_buffer.push_str(&line);
        lines_buffer.push('\n');
        if condition(&line) {
            debug!("condition found: {:?}", line);
            return lines_buffer;
        }
        line.clear();
    }

    panic!("event not found in agent output: {}", event_info);
}

/// Verifies that the agent is still running
pub fn assert_agent_running(agent_handle: &mut Child) {
    assert!(agent_handle.try_wait().ok().unwrap().is_none());
    for _ in 0..10 {
        thread::sleep(std::time::Duration::from_millis(20));
        assert!(agent_handle.try_wait().ok().unwrap().is_none());
    }
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
pub fn open_files_include(id: u32, file: &Path) -> Option<String> {
    let child = Command::new("lsof")
        .args(["-l", "-p", &id.to_string()])
        .stdout(Stdio::piped())
        .spawn()
        .expect("failed to execute child");

    let output = child.wait_with_output().expect("failed to wait on child");

    assert!(output.status.success());

    let output_str = std::str::from_utf8(&output.stdout).unwrap();
    debug!("lsof output:\n{:#?}", output_str);
    if output_str.contains(file.to_str().unwrap()) {
        Some(output_str.to_string())
    } else {
        None
    }
}

pub fn get_available_port() -> Option<u16> {
    let mut rng = rand::thread_rng();
    loop {
        let port = (30025..65535).choose(&mut rng).unwrap();
        if TcpListener::bind(("127.0.0.1", port)).is_ok() {
            break Some(port);
        }
    }
}

pub fn self_signed_https_ingester(
    http_version: Option<HttpVersion>,
    req_fn: Option<ReqFn>,
    process_fn: Option<ProcessFn>,
) -> (
    impl Future<Output = std::result::Result<(), IngestError>>,
    FileLineCounter,
    impl FnOnce(),
    tempfile::NamedTempFile,
    String,
) {
    let subject_alt_names = vec!["logdna.com".to_string(), "localhost".to_string()];

    let cert = generate_simple_self_signed(subject_alt_names).unwrap();

    let cert_bytes = cert.serialize_pem().unwrap();
    let certs = rustls_pemfile::certs(&mut std::io::BufReader::new(cert_bytes.as_bytes()))
        .map(|certs| certs.into_iter().map(rustls::Certificate).collect())
        .unwrap();

    let key_bytes = cert.serialize_private_key_pem();
    let keys: Vec<rustls::PrivateKey> =
        rustls_pemfile::pkcs8_private_keys(&mut std::io::BufReader::new(key_bytes.as_bytes()))
            .map(|keys| keys.into_iter().map(rustls::PrivateKey).collect())
            .unwrap();

    let port = get_available_port().expect("No ports free");

    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), port);

    let mut cert_file = tempfile::NamedTempFile::new().expect("Couldn't create cert file");
    cert_file
        .write_all(cert.serialize_pem().unwrap().as_bytes())
        .expect("Couldn't write cert file");

    let (server, received, shutdown_handle) = https_ingester_with_processors(
        addr,
        certs,
        keys[0].clone(),
        http_version,
        req_fn.unwrap_or_else(|| Box::new(|_| None)),
        process_fn.unwrap_or_else(|| Box::new(|_| None)),
    );
    debug!("Started https ingester on port {}", port);
    (
        server,
        received,
        shutdown_handle,
        cert_file,
        format!("localhost:{}", port),
    )
}

pub fn start_http_ingester() -> (
    impl Future<Output = std::result::Result<(), IngestError>>,
    FileLineCounter,
    impl FnOnce(),
    String,
) {
    let port = get_available_port().expect("No ports free");
    let address = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), port);

    let (server, received, shutdown_handle) = http_ingester(address, None);
    (
        server,
        received,
        shutdown_handle,
        format!("localhost:{}", port),
    )
}

pub fn consume_output<T: std::io::Read + std::marker::Send + 'static>(stderr_handle: T) {
    let stderr_reader = std::io::BufReader::new(stderr_handle);
    std::thread::spawn(move || {
        for line in stderr_reader.lines() {
            let line = line.unwrap();
            if line.is_empty() {
                continue;
            }
            debug!("{}", line.trim());
        }
    });
}

// The compiler/linter believes this function isn't used anywhere but it is currently
// used in the retries and http integration tests. This flag disables that false positive.
#[allow(dead_code)]
pub fn start_ingester(
    req_fn: ReqFn,
    process_fn: ProcessFn,
) -> (
    impl Future<Output = std::result::Result<(), IngestError>>,
    FileLineCounter,
    impl FnOnce(),
    String,
) {
    let port = get_available_port().expect("No ports free");
    let address = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), port);

    let (server, received, shutdown_handle) =
        http_ingester_with_processors(address, None, req_fn, process_fn);
    (
        server,
        received,
        shutdown_handle,
        format!("localhost:{}", port),
    )
}

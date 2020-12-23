use assert_cmd::cargo::CommandCargoExt;
use core::time;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::sync::mpsc::TryRecvError;
use std::thread;

static LINE: &str = "Nov 30 09:14:47 sample-host-name sampleprocess[1204]: Hello from process";

pub struct FileContext {
    pub file_path: PathBuf,
    pub stop_handle: Box<dyn FnOnce() -> i32>,
}

pub fn start_append_to_file(dir: &Path, delay_ms: u64) -> FileContext {
    let file_path = dir.join("appended.log");
    let inner_file_path = file_path.clone();
    let (tx, rx) = mpsc::channel();

    let thread = thread::spawn(move || {
        let mut file = OpenOptions::new()
            .append(true)
            .create(true)
            .open(&inner_file_path)?;

        let delay = time::Duration::from_millis(delay_ms);

        let mut lines_written = 0;
        while let Err(TryRecvError::Empty) = rx.try_recv() {
            if let Err(e) = writeln!(file, "{}", LINE) {
                eprintln!("Couldn't write to file: {}", e);
                return Err(e);
            }
            lines_written += 1;
            if lines_written % 10 == 0 {
                file.sync_all()?;
                thread::sleep(delay);
            }
        }

        file.flush()?;
        Ok(lines_written)
    });

    let stop_handle = move || {
        tx.send("STOP").unwrap();
        // Return the total lines
        thread.join().unwrap().ok().unwrap()
    };

    FileContext {
        file_path,
        stop_handle: Box::new(stop_handle),
    }
}

pub fn append_to_file(
    file_path: &PathBuf,
    lines: i32,
    sync_every: i32,
) -> Result<(), std::io::Error> {
    let mut file = OpenOptions::new()
        .append(true)
        .create(true)
        .open(&file_path)?;

    for i in 0..lines {
        if let Err(e) = writeln!(file, "{}", LINE) {
            eprintln!("Couldn't write to file: {}", e);
            return Err(e);
        }

        if i % sync_every == 0 {
            file.sync_all()?;
            thread::sleep(time::Duration::from_millis(5));
        }
    }
    file.sync_all()?;
    Ok(())
}

pub fn truncate_file(file_path: &PathBuf) -> Result<(), std::io::Error> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(&file_path)?
        .set_len(0)?;
    Ok(())
}

pub fn spawn_agent(dir_path: &str) -> Child {
    let mut cmd = Command::cargo_bin("logdna-agent").unwrap();

    let ingestion_key =
        std::env::var("LOGDNA_INGESTION_KEY").expect("LOGDNA_INGESTION_KEY env var not set");
    assert_ne!(ingestion_key, "");

    let agent = cmd
        .env_clear()
        .env("RUST_LOG", "debug")
        .env("RUST_BACKTRACE", "full")
        .env("LOGDNA_LOG_DIRS", &dir_path)
        .env(
            "LOGDNA_HOST",
            std::env::var("LOGDNA_HOST").expect("LOGDNA_HOST env var not set"),
        )
        .env("LOGDNA_INGESTION_KEY", ingestion_key)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    agent.spawn().expect("Failed to start agent")
}

/// Blocks until a certain event is logged by the agent
pub fn wait_for_file_event(event: &str, file_path: &PathBuf, stderr_reader: &mut dyn BufRead) {
    let mut line = String::new();
    let file_name = &file_path.file_name().unwrap().to_str().unwrap();
    for _safeguard in 0..10_000_000 {
        stderr_reader.read_line(&mut line).unwrap();
        if line.contains(event) && line.contains(file_name) {
            return;
        }
        line.clear();
    }

    panic!(
        "file {:?} event {:?} not found in agent output",
        file_path, event
    );
}

/// Verifies that the agent is still running
pub fn assert_agent_running(agent_handle: &mut Child) {
    thread::sleep(std::time::Duration::from_millis(20));
    assert!(agent_handle.try_wait().ok().unwrap().is_none());
}

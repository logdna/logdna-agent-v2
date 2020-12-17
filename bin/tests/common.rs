use assert_cmd::cargo::CommandCargoExt;
use core::time;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::sync::mpsc::TryRecvError;
use std::thread;

pub struct FileContext {
    pub file_path: PathBuf,
    pub stop_handle: Box<dyn FnOnce() -> i32>,
}

pub fn start_append_to_file(dir: &PathBuf, delay_ms: u64) -> FileContext {
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
        let line = "Nov 30 09:14:47 sample-host-name sampleprocess[1204]: Hello from process";
        while let Err(TryRecvError::Empty) = rx.try_recv() {
            if let Err(e) = writeln!(file, "{}", line) {
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

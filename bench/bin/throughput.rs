// Start mock ingester
// Start generating logs
// Start agent under flamegraph
// Kill agent

use std::convert::TryInto;
use std::future::Future;
use std::io::{BufRead, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};

use file_rotate::{FileRotate, RotationMode};
use memmap2::MmapOptions;
use nix::sys::signal::{self, Signal};
use nix::unistd::Pid;
use owning_ref::OwningHandle;
use rand::prelude::*;
use structopt::StructOpt;
use tokio::fs::{self};

use logdna_mock_ingester::{
    http_ingester_with_processors, FileLineCounter, IngestError, ProcessFn,
};

const CARGO_MANIFEST_DIR: &str = "CARGO_MANIFEST_DIR";

#[derive(Debug, StructOpt)]
#[structopt(name = "agent throughput bench")]
struct Opt {
    /// Dict file
    #[structopt(parse(from_os_str))]
    dict: PathBuf,

    /// Output directory
    #[structopt(parse(from_os_str), short)]
    out_dir: PathBuf,

    /// Number of files to retain during rotation
    #[structopt(long)]
    file_history: usize,

    /// Cut of Bytes before rotation
    #[structopt(long)]
    file_size: usize,
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

fn start_ingester(
    process_fn: ProcessFn,
) -> (
    impl Future<Output = std::result::Result<(), IngestError>>,
    FileLineCounter,
    impl FnOnce(),
    String,
) {
    let port = get_available_port().expect("No ports free");
    let address = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), port);

    let (server, received, shutdown_handle) = http_ingester_with_processors(address, process_fn);
    (
        server,
        received,
        shutdown_handle,
        format!("localhost:{}", port),
    )
}

#[tokio::main]
async fn main() -> Result<(), std::io::Error> {
    let opt = Opt::from_args();
    println!("{:?}", opt);

    // Parse mmap into Vec<&str>
    let words = OwningHandle::new_with_fn(
        Box::new(unsafe { MmapOptions::new().map(&std::fs::File::open(opt.dict)?)? }),
        |dict_arr_ptr| unsafe {
            dict_arr_ptr
                .as_ref()
                .unwrap()
                .split(|c| c == &b'\n')
                .map(|s| std::str::from_utf8(s).unwrap())
                .collect::<Vec<&str>>()
        },
    );

    let mut manifest_path = std::path::PathBuf::from(std::env::var(CARGO_MANIFEST_DIR).unwrap());
    manifest_path.pop();
    manifest_path.push("bin/Cargo.toml");
    println!("{:?}", manifest_path);

    let cargo_build = escargot::CargoBuild::new()
        .manifest_path(manifest_path)
        .bin("logdna-agent")
        .release()
        .current_target()
        .run()
        .unwrap();

    let ingestion_key = "thisIsAnApiKeyNot123456";
    let mut agent_cmd = cargo_build.command();

    let line_counter = std::sync::Arc::new(AtomicU64::new(0));

    let (server, _, shutdown_handle, address) = start_ingester(Box::new({
        let line_counter = line_counter.clone();
        move |body| {
            Some(Box::pin({
                line_counter.fetch_add(body.lines.len().try_into().unwrap(), Ordering::Relaxed);
                tokio::time::sleep(std::time::Duration::from_millis(200))
            }))
        }
    }));

    let agent_cmd = agent_cmd
        .env("LOGDNA_LOG_DIRS", opt.out_dir.clone())
        .env("LOGDNA_HOST", address)
        .env("LOGDNA_INGESTION_KEY", ingestion_key)
        .env("LOGDNA_USE_SSL", "false")
        .env("RUST_LOG", "info")
        .env("RUST_BACKTRACE", "full")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    println!("Spawning agent");
    let mut agent_handle = agent_cmd.spawn().unwrap();

    let agent_stdout = agent_handle.stdout.take().unwrap();
    let stdout_reader = std::io::BufReader::new(agent_stdout);
    std::thread::spawn(move || for _line in stdout_reader.lines() {});

    let agent_stderr = agent_handle.stderr.take().unwrap();
    let stderr_reader = std::io::BufReader::new(agent_stderr);
    std::thread::spawn(move || {
        for _line in stderr_reader.lines() {
            //()
            eprintln!("Agent STDERR: {}", _line.unwrap())
        }
    });

    println!("Spawning flamegraph");
    let mut flamegraph_cmd = std::process::Command::new("flamegraph");
    let flamegraph_cmd = flamegraph_cmd
        .args([
            "-p",
            &format!("{}", agent_handle.id()),
            "-o",
            "/tmp/flamegraph.svg",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    let mut flamegraph_handle = flamegraph_cmd.spawn().unwrap();

    println!("Waiting for mock ingester");
    let (server_result, _) = tokio::join!(server, {
        let out_dir = opt.out_dir.clone();
        let file_size = opt.file_size;
        let file_history = opt.file_history;

        async move {
            let mut out_file: PathBuf = out_dir.clone();
            out_file.push("test.log");

            fs::create_dir(out_dir).await.unwrap();

            let line_count = 10_000_000;
            tokio::task::spawn_blocking({
                let out_file = out_file.clone();
                move || {
                    let mut count = 0;
                    let mut log = FileRotate::new(
                        out_file.clone(),
                        RotationMode::BytesSurpassed(file_size),
                        file_history,
                    );

                    for word in words.iter().cycle().take(line_count / 20) {
                        count += 1;
                        if count % 10000 == 0 {
                            std::thread::sleep(std::time::Duration::from_millis(10));
                        }
                        writeln!(log, "{}", word).unwrap();
                    }

                    println!("Written lines: {}", count);
                    std::thread::sleep(std::time::Duration::from_secs(10));

                    for word in words.iter().cycle().take(line_count - line_count / 20) {
                        count += 1;
                        if count % 10000 == 0 {
                            std::thread::sleep(std::time::Duration::from_millis(10));
                        }
                        writeln!(log, "{}", word).unwrap();
                    }

                    println!("Written lines: {}", count);
                }
            })
            .await
            .unwrap();
            println!("waiting for 60 seconds");
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;

            let lines = line_counter.load(Ordering::Relaxed);
            println!("Lines: {:#?}", lines);
            shutdown_handle();
        }
    });

    server_result.unwrap();

    let agent_pid = agent_handle.id().try_into().unwrap();
    let proc_info = procfs::process::Process::new(agent_pid).unwrap();
    let stat = proc_info.stat().unwrap();
    let io = proc_info.io().unwrap();

    println!("Killing Agent");
    signal::kill(Pid::from_raw(agent_pid), Signal::SIGTERM).unwrap();

    println!("Waiting for agent");
    agent_handle.wait().unwrap();

    println!("/proc Stat:\n{:#?}", stat);
    println!("/proc IO:\n{:#?}", io);

    println!("Waiting on flamegraph");
    println!("{:#?}", flamegraph_handle.wait().unwrap());

    // rt.shutdown_timeout(std::time::Duration::from_millis(100));

    Ok(())
}

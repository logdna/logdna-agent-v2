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
use std::sync::Arc;

use file_rotate::{FileRotate, RotationMode};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
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

    /// Output directory
    #[structopt(long)]
    profile: bool,

    /// Number of files to retain during rotation
    #[structopt(long)]
    file_history: usize,

    /// Cut of Bytes before rotation
    #[structopt(long)]
    file_size: usize,

    /// Number of lines to write
    #[structopt(long)]
    line_count: usize,

    /// Ingester delay
    #[structopt(long)]
    ingester_delay: Option<u64>,
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

    let (server, received, shutdown_handle) =
        http_ingester_with_processors(address, None, process_fn);
    (
        server,
        received,
        shutdown_handle,
        format!("localhost:{}", port),
    )
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), std::io::Error> {
    let opt = Opt::from_args();

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

    println!("Building Agent");

    let cargo_build = escargot::CargoBuild::new()
        .manifest_path(manifest_path)
        .bin("logdna-agent")
        .release()
        .current_target()
        .run()
        .unwrap();

    println!("Agent Built");

    let line_count = opt.line_count;
    fs::create_dir(&opt.out_dir).await.unwrap_or(());

    println!("starting progress bars...");
    let m = Arc::new(MultiProgress::new());
    let sty = ProgressStyle::default_bar()
        .template("[{msg} {elapsed_precise}] {bar:40.cyan/blue} {pos:>7}/{len:7} {per_sec}")
        .progress_chars("##-");
    let wpb = m.add(ProgressBar::new(line_count as u64));
    wpb.set_style(sty.clone());
    wpb.set_message("Logged Lines:");
    wpb.tick();

    let rpb = m.add(ProgressBar::new(line_count as u64));
    rpb.set_style(sty.clone());
    rpb.set_message("Received:    ");
    rpb.tick();

    let mp = tokio::task::spawn_blocking({
        let mp = m.clone();
        move || mp.join().unwrap()
    });

    let ingestion_key = "thisIsAnApiKeyNot123456";
    let mut agent_cmd = cargo_build.command();

    let line_counter = std::sync::Arc::new(AtomicU64::new(0));

    let (server, _, shutdown_handle, address) = start_ingester(Box::new({
        let ingester_delay = opt.ingester_delay.unwrap_or(1000);
        let line_counter = line_counter.clone();
        let rpb1 = rpb.clone();
        move |body| {
            let lines = body.lines.len();
            rpb1.inc(lines.try_into().unwrap());
            line_counter.fetch_add(lines.try_into().unwrap(), Ordering::SeqCst);
            Some(Box::pin(tokio::time::sleep(
                std::time::Duration::from_millis(ingester_delay),
            )))
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

    let mut agent_handle = agent_cmd.spawn().unwrap();
    let agent_pid = agent_handle.id().try_into().unwrap();
    wpb.println(format!("Spawned agent, pid: {}", agent_pid));

    let agent_stdout = agent_handle.stdout.take().unwrap();
    let stdout_reader = std::io::BufReader::new(agent_stdout);
    std::thread::spawn({
        let wpb1 = wpb.clone();
        move || {
            for _line in stdout_reader.lines() {
                wpb1.println(format!("Agent STDOUT: {}", _line.unwrap()));
            }
        }
    });

    let agent_stderr = agent_handle.stderr.take().unwrap();
    let stderr_reader = std::io::BufReader::new(agent_stderr);
    std::thread::spawn({
        let wpb1 = wpb.clone();
        move || {
            for _line in stderr_reader.lines() {
                wpb1.println(format!("Agent STDOUT: {}", _line.unwrap()));
            }
        }
    });

    let flamegraph_handle = if opt.profile {
        wpb.println("Spawning flamegraph");
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

        Some(flamegraph_cmd.spawn().unwrap())
    } else {
        None
    };

    wpb.println("Waiting for mock ingester");
    let (server_result, _) = tokio::join!(server, {
        let out_dir = opt.out_dir.clone();
        let file_size = opt.file_size;
        let file_history = opt.file_history;

        let line_counter = line_counter.clone();
        async move {
            let mut out_file: PathBuf = out_dir.clone();
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            out_file.push("test.log");

            tokio::task::spawn_blocking({
                let out_file = out_file.clone();
                let wpb = wpb.clone();
                move || {
                    let mut count = 0;
                    let mut log = std::io::BufWriter::new(FileRotate::new(
                        out_file.clone(),
                        RotationMode::BytesSurpassed(file_size),
                        file_history,
                    ));

                    // Write first 5% of logs
                    for word in words.iter().cycle().take(line_count / 20) {
                        count += 1;
                        if count % 10_000 == 0 {
                            std::thread::sleep(std::time::Duration::from_millis(50));
                            wpb.inc(10_000);
                            if count % 100_000 == 0 {
                                log.flush().unwrap();
                            }
                        }
                        writeln!(log, "{}", word).unwrap();
                    }

                    // Write the rest of the logs
                    for word in words.iter().cycle().take(line_count - line_count / 20) {
                        count += 1;
                        if count % 10_000 == 0 {
                            wpb.inc(10_000);
                            std::thread::sleep(std::time::Duration::from_millis(50));
                            if count % 100_000 == 0 {
                                log.flush().unwrap();
                            }
                        }
                        writeln!(log, "{}", word).unwrap();
                    }
                }
            })
            .await
            .unwrap();

            let mut no_progress_count = 0;
            let mut last_count = 0;
            wpb.println("Waiting for agent to stop uploading");
            while line_counter.load(Ordering::SeqCst) < line_count as u64 {
                let lines = line_counter.load(Ordering::SeqCst);
                if last_count == lines {
                    no_progress_count += 1;
                    if no_progress_count > 5 {
                        wpb.println(format!("Final agent upload count: {}", lines));
                        break;
                    }
                }
                last_count = lines;
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
            wpb.finish_at_current_pos();
            println!("Finished up wpb");
            shutdown_handle();
        }
    });

    server_result.unwrap();
    rpb.finish_at_current_pos();
    mp.await.unwrap();

    println!("Finished up rpb");

    let proc_info = procfs::process::Process::new(agent_pid).unwrap();
    let stat = proc_info.stat().unwrap();
    let io = proc_info.io().unwrap();

    println!("Killing agent pid {}", agent_pid);
    signal::kill(Pid::from_raw(agent_pid), Signal::SIGTERM).unwrap();

    println!("Waiting for agent pid {}", agent_pid);
    agent_handle.wait().unwrap();

    println!("/proc Stat:\n{:#?}", stat);
    println!("/proc IO:\n{:#?}", io);

    if let Some(mut flamegraph_handle) = flamegraph_handle {
        println!("Waiting on flamegraph, this might take a while.");
        println!("{:#?}", flamegraph_handle.wait().unwrap());
    }
    Ok(())
}

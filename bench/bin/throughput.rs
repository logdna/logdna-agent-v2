// Start mock ingester
// Start generating logs
// Start agent under flamegraph
// Kill agent

use std::convert::TryInto;
use std::fs::File;
use std::future::Future;
use std::io::{BufRead, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use file_rotate::{compression::Compression, suffix::AppendCount, ContentLimit, FileRotate};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use logdna_metrics_recorder::*;
use memmap2::MmapOptions;
use nix::sys::signal::{self, Signal};
use nix::unistd::Pid;
use owning_ref::OwningHandle;
use prometheus_parse::{Sample, Value};
use rand::prelude::*;
use structopt::StructOpt;
use tokio::fs::{self};

use logdna_mock_ingester::{
    http_ingester_with_processors, FileLineCounter, IngestError, ProcessFn, ReqFn,
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

    /// stderr log file path
    #[structopt(long)]
    stderr_log_file_path: Option<String>,
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

fn data_pair_for(name: &str) -> impl Fn(&Sample) -> Option<(i64, f64)> + '_ {
    move |s: &Sample| match (&s.value, &s.labels.get("outcome")) {
        (Value::Untyped(raw), Some("success")) if s.metric.as_str() == name => {
            Some((s.timestamp.timestamp_millis(), *raw))
        }
        _ => None,
    }
}

fn is_agent_metric(sample: &prometheus_parse::Sample, metric_name: &str) -> bool {
    let mut full_metric_name = String::from("logdna_agent_");
    full_metric_name.push_str(metric_name);
    sample.metric.as_str() == full_metric_name
}

// Sample number of file system lines
fn calculate_fs_line_metrics(samples: &[Sample]) -> (f64, f64) {
    let metric_name = "fs_lines";
    let fs_sample_data = samples
        .iter()
        .filter_map(|s| match s.value {
            Value::Counter(raw) if is_agent_metric(s, metric_name) => {
                Some((raw, s.timestamp.timestamp_millis()))
            }
            _ => None,
        })
        .collect::<Vec<(f64, i64)>>();

    let (fs_last_val, fs_last_tv) = fs_sample_data.last().unwrap();
    let (_fs_first_val, fs_first_tv) = fs_sample_data[0];

    let fs_total_time = (fs_last_tv - fs_first_tv) / 1000;
    let fs_total_lines = *fs_last_val;
    let fs_lines_rate = fs_total_lines / fs_total_time as f64;

    (fs_total_lines, fs_lines_rate)
}

// Sample number of file system bytes
fn calculate_fs_byte_metrics(samples: &[Sample]) -> (i64, f64, f64) {
    let metric_name = "fs_bytes";
    let fs_sample_data = samples
        .iter()
        .filter_map(|s| match s.value {
            Value::Counter(raw) if is_agent_metric(s, metric_name) => {
                Some((raw, s.timestamp.timestamp_millis()))
            }
            _ => None,
        })
        .collect::<Vec<(f64, i64)>>();

    let (fs_last_val, fs_last_tv) = fs_sample_data.last().unwrap();
    let (_fs_first_val, fs_first_tv) = fs_sample_data[0];

    let fs_total_time = (fs_last_tv - fs_first_tv) / 1000;
    let fs_total_bytes = *fs_last_val;
    let fs_bytes_rate = fs_total_bytes / fs_total_time as f64;

    (fs_total_time, fs_total_bytes, fs_bytes_rate)
}

// Sample maximum memory
fn calculate_memory_max(samples: &[Sample]) -> f64 {
    let process_virtual_memory = samples.iter().filter_map(|m| match m.value {
        Value::Gauge(raw) if m.metric.as_str() == "process_virtual_memory_bytes" => Some(raw),
        _ => None,
    });

    process_virtual_memory.into_iter().reduce(f64::max).unwrap()
}

// Sample ingest requests
fn calculate_ingest_time_metrics(samples: &[Sample]) -> (i64, f64) {
    let ingest_duration_sample = samples
        .iter()
        .filter_map(data_pair_for(
            "logdna_agent_ingest_request_duration_seconds_sum",
        ))
        .zip(samples.iter().filter_map(data_pair_for(
            "logdna_agent_ingest_request_duration_seconds_count",
        )))
        .collect::<Vec<((i64, f64), (i64, f64))>>();

    let ((last_sum_tv, last_sum_val), (_last_count_tv, last_count_val)) =
        ingest_duration_sample.last().unwrap();
    let ((first_sum_tv, _first_sum_val), (_first_count_tv, _first_count_val)) =
        ingest_duration_sample[0];

    let ingest_total_time = (last_sum_tv - first_sum_tv) / 1000;
    let ingest_average_request_time = *last_sum_val / *last_count_val;
    (ingest_total_time, ingest_average_request_time)
}

fn calulate_ingest_size_metrics(samples: &[Sample]) -> (f64, f64, f64) {
    let ingest_size_sample = samples
        .iter()
        .filter_map(|s| match s.value {
            Value::Untyped(raw) if is_agent_metric(s, "ingest_request_size_sum") => Some(raw),
            _ => None,
        })
        .zip(samples.iter().filter_map(|t| match t.value {
            Value::Untyped(raw) if is_agent_metric(t, "ingest_request_size_count") => Some(raw),
            _ => None,
        }))
        .collect::<Vec<(f64, f64)>>();

    let (ingest_size_sum, ingest_size_count) = ingest_size_sample.last().unwrap();
    let ingest_size_rate = ingest_size_sum / ingest_size_count;
    (*ingest_size_sum, *ingest_size_count, ingest_size_rate)
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

    let mut cargo_build = escargot::CargoBuild::new()
        .manifest_path(manifest_path)
        .bin("logdna-agent")
        .release()
        .current_target();

    let features: Vec<std::ffi::OsString> = Vec::new();

    #[cfg(feature="dhat-heap")]
    let features = { let mut features= features; features.push("dhat-heap".into()); features};

    cargo_build = match features {
        features if !features.is_empty() => cargo_build.no_default_features().features(features.join(std::ffi::OsStr::new(","))),
        _ => cargo_build
    };

    let cargo_build = cargo_build
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

    let (server, _, shutdown_handle, address) = start_ingester(
        Box::new(|_| None),
        Box::new({
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
        }),
    );

    let agent_cmd = agent_cmd
        .env("MZ_LOG_DIRS", opt.out_dir.clone())
        .env("MZ_HOST", address)
        .env("MZ_INGESTION_KEY", ingestion_key)
        .env("MZ_METRICS_PORT", "9881")
        .env("MZ_USE_SSL", "false")
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
        let agent_stderr_filepath = opt.stderr_log_file_path.clone();
        move || {
            let mut agent_stderr_file = agent_stderr_filepath.map(|fp|std::io::LineWriter::new(std::fs::File::create(fp).unwrap()));
            for _line in stderr_reader.lines() {
                if let Some(ref mut agent_stderr_file) = agent_stderr_file {
                    agent_stderr_file.write(_line.as_ref().unwrap().as_bytes()).unwrap();
                    agent_stderr_file.write_all(b"\n").unwrap();
                }
                wpb1.println(format!("Agent STDERR: {}", _line.as_ref().unwrap()));
            }
            if let Some(ref mut agent_stderr_file) = agent_stderr_file {
                agent_stderr_file.flush().unwrap();
            }
        }
    });

    let flamegraph_handle = if opt.profile {
        wpb.println("Spawning flamegraph");
        let mut flamegraph_cmd = std::process::Command::new("flamegraph");
        let flamegraph_cmd = flamegraph_cmd.args([
            "-p",
            &format!("{}", agent_handle.id()),
            "-o",
            "/tmp/flamegraph.svg",
        ]);

        Some(flamegraph_cmd.spawn().unwrap())
    } else {
        None
    };

    wpb.println("Waiting for mock ingester");
    let (server_result, metrics_result) = tokio::join!(server, {
        let out_dir = opt.out_dir.clone();
        let file_size = opt.file_size;
        let file_history = opt.file_history;

        let line_counter = line_counter.clone();
        let metrics_port = 9881;
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        let recorder = MetricsRecorder::start(metrics_port, Some(Duration::from_millis(100)));

        async move {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            let mut out_file: PathBuf = out_dir.clone();
            out_file.push("test1.log");

            let writer_1 = tokio::task::spawn_blocking({
                let words = Vec::from(&*words).clone();
                let out_file = out_file.clone();
                let wpb = wpb.clone();
                move || {
                    let mut count = 0;
                    let mut log = std::io::BufWriter::new(FileRotate::new(
                        out_file.clone(),
                        AppendCount::new(file_history),
                        ContentLimit::Bytes(file_size),
                        Compression::None,
                    ));

                    // Write first 5% of logs
                    for word in words.iter().cycle().take(line_count / 20) {
                        count += 1;
                        if count % 10_000 == 0 {
                            std::thread::sleep(std::time::Duration::from_millis(12));
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
                            std::thread::sleep(std::time::Duration::from_millis(12));
                            if count % 100_000 == 0 {
                                log.flush().unwrap();
                            }
                        }
                        writeln!(log, "{}", word).unwrap();
                    }
                }
            });

            let mut out_file: PathBuf = out_dir.clone();
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            out_file.push("test2.log");

            let writer_2 = tokio::task::spawn_blocking({
                let out_file = out_file.clone();
                let wpb = wpb.clone();
                move || {
                    let mut count = 0;
                    let mut log = std::io::BufWriter::new(FileRotate::new(
                        out_file.clone(),
                        AppendCount::new(file_history),
                        ContentLimit::Bytes(file_size),
                        Compression::None,
                    ));

                    // Write first 5% of logs
                    for word in words.iter().cycle().take(line_count / 20) {
                        count += 1;
                        if count % 10_000 == 0 {
                            std::thread::sleep(std::time::Duration::from_millis(12));
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
                            std::thread::sleep(std::time::Duration::from_millis(12));
                            if count % 100_000 == 0 {
                                log.flush().unwrap();
                            }
                        }
                        writeln!(log, "{}", word).unwrap();
                    }
                }
            });

            let (w1r, w2r) = tokio::join!(writer_1, writer_2);
            w1r.unwrap();
            w2r.unwrap();

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

            let metrics_result = recorder.stop().await;

            wpb.finish_at_current_pos();
            println!("Finished up wpb");
            shutdown_handle();
            metrics_result
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

    // Calculate metrics
    let fs_line_metrics = calculate_fs_line_metrics(&metrics_result);
    let fs_size_metrics = calculate_fs_byte_metrics(&metrics_result);
    let ingest_time_metrics = calculate_ingest_time_metrics(&metrics_result);
    let ingest_size_metrics = calulate_ingest_size_metrics(&metrics_result);
    let max_memory = calculate_memory_max(&metrics_result);
    println!(
        "\nFILE SYSTEM METRICS\n . Total Time (sec): {:?}\n . Total Lines: {:?}\n . Line Rate (lines/sec): {:?}\n . Total Size (bytes): {:?}\n . File Rate (bytes/sec): {:?}",
        fs_size_metrics.0, fs_line_metrics.0, fs_line_metrics.1, fs_size_metrics.1, fs_size_metrics.2
    );
    println!(
        "\nINGESTION METRICS\n . Total Time (sec): {:?}\n . Total Size (bytes): {:?}\n . Total number of Samples: {:?}\n . Average Request Duration (sec): {:?}\n . Average Request Size (bytes): {:?}",
        ingest_time_metrics.0, ingest_size_metrics.0, ingest_size_metrics.1, ingest_time_metrics.1, ingest_size_metrics.2
    );
    println!(
        "\nMEMORY METRICS:\n . Max Process Virtual Memory (bytes): {:?}\n",
        max_memory
    );

    let metrics_file = File::create("metrics_output.log").expect("Could not open file.");
    writeln!(&metrics_file, "{:?}", metrics_result).expect("Cound not write to file.");
    if let Some(mut flamegraph_handle) = flamegraph_handle {
        println!("Waiting on flamegraph, this might take a while.");
        println!("{:#?}", flamegraph_handle.wait().unwrap());
    }

    Ok(())
}

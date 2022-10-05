#![cfg_attr(windows, no_main)]

#[cfg(not(windows))]
compile_error!("Can be compiled for Windows target only!");

#[macro_use]
extern crate log;
#[macro_use]
extern crate winservice;

use std::env;
use std::os::raw::{c_char, c_int, c_void};
use std::sync::mpsc::Receiver;
use std::sync::Arc;

use flexi_logger::{Age, Cleanup, Criterion, Duplicate, FileSpec, Logger, Naming, WriteMode};
use flexi_logger::{DeferredNow, Record, TS_DASHES_BLANK_COLONS_DOT_BLANK};
use tokio::sync::Mutex;
use tokio::time::Duration;

use config::{self, Config};

use crate::_main::_main;

mod _main;
#[cfg(feature = "dep_audit")]
mod dep_audit;
mod stream_adapter;

pub const SERVICE_NAME: &str = "Mezmo Agent";

#[allow(non_snake_case)]
#[no_mangle]
pub extern "system" fn WinMain(
    _hInstance: *const c_void,
    _hPrevInstance: *const c_void,
    _lpCmdLine: *const c_char,
    _nCmdShow: c_int,
) -> c_int {
    Service!(SERVICE_NAME, service_main)
}

fn service_main(_args: Vec<String>, svc_shutdown_rx: Receiver<()>) -> u32 {
    // covert logdna env vars to mezmo ones
    Config::process_logdna_env_vars();

    let _logger = Logger::try_with_env_or_str("info")
        .expect("failed to create log")
        .format_for_files(log_format)
        .use_utc()
        .log_to_file(
            FileSpec::default().directory(config::raw::default_log_dirs().first().unwrap()),
        )
        .write_mode(WriteMode::Direct)
        .duplicate_to_stderr(Duplicate::Warn)
        .rotate(
            Criterion::Age(Age::Day),
            Naming::Timestamps,
            Cleanup::KeepLogFiles(7),
        )
        .start()
        .expect("failed to start log");

    info!("running version: {}", env!("CARGO_PKG_VERSION"));
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let shutdown_tx = Arc::new(Mutex::new(Some(shutdown_tx)));

    let local_shutdown_tx = shutdown_tx.clone();
    runtime.spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(200)).await;
            if let Ok(_) | Err(std::sync::mpsc::TryRecvError::Disconnected) =
                svc_shutdown_rx.try_recv()
            {
                break;
            }
        }
        local_shutdown_tx
            .lock()
            .await
            .take()
            .unwrap()
            .send(())
            .unwrap();
        info!("requested shutdown");
    });

    // block on agent main loop
    runtime.block_on(_main(shutdown_tx, shutdown_rx));
    0
}

fn log_format(
    w: &mut dyn std::io::Write,
    now: &mut DeferredNow,
    record: &Record,
) -> Result<(), std::io::Error> {
    let ts = now.format(TS_DASHES_BLANK_COLONS_DOT_BLANK);
    write!(w, "[{:>}] {:<5} {}", ts, record.level(), &record.args())
}

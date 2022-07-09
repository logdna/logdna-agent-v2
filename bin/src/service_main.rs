#![cfg_attr(windows, no_main)]

#[cfg(not(windows))]
compile_error!("Only Windows target is supported.");

#[macro_use]
extern crate log;

#[macro_use]
extern crate winservice;

use std::sync::Arc;
use tokio::sync::Mutex;

use std::os::raw::{c_char, c_int, c_void};
use std::sync::mpsc::Receiver;

use config::{self, Config};
use flexi_logger::{Age, Cleanup, Criterion, Duplicate, FileSpec, Logger, Naming, WriteMode};

mod _main;
#[cfg(feature = "dep_audit")]
mod dep_audit;
mod stream_adapter;

use crate::_main::_main;
use tokio::time::Duration;

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
        .log_to_file(
            FileSpec::default().directory(config::raw::default_log_dirs().first().unwrap()),
        )
        .write_mode(WriteMode::Async)
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
        print!("begin");
        loop {
            tokio::time::sleep(Duration::from_millis(200)).await;
            if let Ok(_) = svc_shutdown_rx.try_recv() {
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
        print!("end");
    });

    // block on agent main loop
    runtime.block_on(_main(shutdown_tx, shutdown_rx));
    0
}

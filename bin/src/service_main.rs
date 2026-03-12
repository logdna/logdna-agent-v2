#![cfg_attr(windows, no_main)]

#[cfg(not(windows))]
compile_error!("Can be compiled for Windows target only!");

use std::env;
use std::ffi::OsString;
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;

use flexi_logger::{Cleanup, Criterion, Duplicate, FileSpec, Logger, Naming, WriteMode};
use flexi_logger::{DeferredNow, Record, TS_DASHES_BLANK_COLONS_DOT_BLANK};
use tokio::sync::Mutex;
use tokio::time::Duration;
use tracing::info;

use config::{self, Config};

use windows_service::{
    service::{
        ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
        ServiceType,
    },
    service_control_handler::{self, ServiceControlHandlerResult},
    service_dispatcher, Result,
};

use crate::_main::_main;

mod _main;
#[cfg(feature = "dep_audit")]
mod dep_audit;
mod stream_adapter;

pub const SERVICE_NAME: &str = "Mezmo Agent";

fn main() -> Result<()> {
    service_dispatcher::start(SERVICE_NAME, service_main)?;
    Ok(())
}

fn service_main(_args: Vec<OsString>) {
    let (shutdown_tx, shutdown_rx) = channel::<()>();

    let shutdown_tx_clone = shutdown_tx.clone();
    let status_handle =
        service_control_handler::register(SERVICE_NAME, move |control_event| match control_event {
            ServiceControl::Stop | ServiceControl::Shutdown => {
                let _ = shutdown_tx_clone.send(());
                ServiceControlHandlerResult::NoError
            }
            _ => ServiceControlHandlerResult::NotImplemented,
        })
        .expect("Failed to register service control handler");

    // report running status
    status_handle
        .set_service_status(ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: ServiceState::Running,
            controls_accepted: ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
            exit_code: ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: std::time::Duration::default(),
            process_id: None,
        })
        .expect("Unable to set service status");

    let exit_code = run_agent(shutdown_rx);

    // report stopped status
    status_handle
        .set_service_status(ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: ServiceState::Stopped,
            controls_accepted: ServiceControlAccept::empty(),
            exit_code: ServiceExitCode::Win32(exit_code),
            checkpoint: 0,
            wait_hint: std::time::Duration::default(),
            process_id: None,
        })
        .ok();
}

fn run_agent(svc_shutdown_rx: Receiver<()>) -> u32 {
    // convert logdna env vars to mezmo ones
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
            Criterion::Size(1024 * 1000 * 5),
            Naming::Timestamps,
            Cleanup::KeepLogFiles(5),
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
    {
        let shutdown_tx = shutdown_tx.clone();
        std::thread::spawn(move || {
            let _ = svc_shutdown_rx.recv();

            if let Some(tx) = futures::executor::block_on(shutdown_tx.lock()).take() {
                let _ = tx.send(());
            }

            info!("requested shutdown");
        })
    }

    match runtime.block_on(_main(shutdown_tx, shutdown_rx)) {
        Err(_) => 1,
        Ok(_) => 0,
    }
}

fn log_format(
    w: &mut dyn std::io::Write,
    now: &mut DeferredNow,
    record: &Record,
) -> Result<(), std::io::Error> {
    let ts = now.format(TS_DASHES_BLANK_COLONS_DOT_BLANK);
    write!(w, "[{:>}] {:<5} {}", ts, record.level(), &record.args())
}

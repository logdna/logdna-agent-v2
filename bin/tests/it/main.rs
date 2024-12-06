#[allow(clippy::zombie_processes)]
mod cli;
mod cmd_line_env;
mod common;
#[allow(clippy::zombie_processes)]
mod http;
mod metrics;
#[allow(clippy::zombie_processes)]
mod retries;

#[cfg(target_os = "linux")]
mod k8s;

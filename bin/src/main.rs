use config::{self, Config};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, info, trace, warn};
use tracing_subscriber::{EnvFilter, FmtSubscriber};

mod _main;
#[cfg(feature = "dep_audit")]
mod dep_audit;
mod stream_adapter;

use crate::_main::_main;

#[cfg(feature = "dhat-heap")]
#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

#[cfg(all(unix, feature = "jemalloc"))]
#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

#[cfg(all(feature = "dhat-heap", feature = "jemalloc"))]
compile_error!("feature \"dhat-heap\" and feature \"jemalloc\" cannot be enabled at the same time");

fn main() -> anyhow::Result<()> {
    #[cfg(feature = "dhat-heap")]
    let _profiler = dhat::Profiler::new_heap();

    // covert logdna env vars to mezmo ones
    Config::process_logdna_env_vars();

    let subscriber = FmtSubscriber::builder()
        .with_env_filter(EnvFilter::from_default_env())
        //.with_max_level(Level::INFO)
        .without_time()
        .with_writer(std::io::stderr)
        .finish();

    tracing::subscriber::set_global_default(subscriber).expect("failied to set subscriber");

    info!("running version: {}", env!("CARGO_PKG_VERSION"));

    // must be done at the very beginning and before other threads started
    #[cfg(target_os = "linux")]
    {
        // apply capabilities only when running under:
        // - k8s
        // - docker
        if (std::env::var_os("KUBERNETES_SERVICE_HOST").is_some()
            || std::path::Path::new("/.dockerenv").exists())
            && std::env::var_os(config::env_vars::NO_CAP).is_none()
        {
            match set_capabilities() {
                Ok(r) if r => debug!("Using Capabilities to bypass filesystem permissions"),
                _ => warn!("Failed to adopt capabilities to bypass DAC. The agent will only be able to access files accessible to it's user/group"),
            }
        }
        let status =
            std::fs::read_to_string("/proc/self/status").expect("Failed to read /proc/self/status");
        let re = regex::Regex::new(r"(?m)^((Cap|Cpu|Seccomp|Groups|Uid|Gid).+?)$").unwrap();
        for cap in re.captures_iter(status.as_str()) {
            info!("{}", &cap[0]);
        }
    }

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let shutdown_tx = Arc::new(Mutex::new(Some(shutdown_tx)));

    // Set up tokio runtime and block on agent main loop
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(_main(shutdown_tx, shutdown_rx))
}

#[cfg(target_os = "linux")]
fn set_capabilities() -> Result<bool, capctl::Error> {
    use capctl::caps::{Cap, CapState};

    // Get the caps for the current pid
    let mut cap_state = CapState::get_current()?;
    trace!(
        "initial caps -\npermitted {:?}\neffective {:?}\ninherited {:?}",
        cap_state.permitted,
        cap_state.effective,
        cap_state.inheritable
    );
    // needs in image:
    // # setcap "cap_dac_read_search+p" /work/logdna-agent
    cap_state.effective.add(Cap::DAC_READ_SEARCH);
    cap_state.inheritable.add(Cap::DAC_READ_SEARCH);
    cap_state.set_current()?;

    let cap_state = CapState::get_current()?;
    trace!(
        "new capabilities -\npermitted {:?}\neffective {:?}\ninherited {:?}",
        cap_state.permitted,
        cap_state.effective,
        cap_state.inheritable
    );
    // Check if we have DAC_READ_SEARCH or DAC_OVERRIDE
    Ok(cap_state
        .effective
        .iter()
        .any(|cap| [Cap::DAC_READ_SEARCH, Cap::DAC_OVERRIDE].contains(&cap)))
}

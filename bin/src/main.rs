#[macro_use]
extern crate log;

use std::path::PathBuf;
use std::thread::sleep;
use std::thread::spawn;
use std::time::Duration;

use config::Config;
use fs::tail::Tailer;
use fs::watch::Watcher;
use http::client::Client;
use http::retry::Retry;
use k8s::K8s;
use middleware::Executor;

fn main() {
    env_logger::init();
    info!("running version: {}", env!("CARGO_PKG_VERSION"));

    let config = match Config::new() {
        Ok(v) => v,
        Err(e) => {
            error!("config error: {}", e);
            std::process::exit(1);
        }
    };

    let watcher = Watcher::builder()
        .add_all(config.log.dirs)
        .append_all(config.log.rules)
        .build()
        .unwrap();

    let tailer = Tailer::new();
    let tailer_sender = tailer.sender();

    let mut client = Client::new(config.http.template);
    client.set_max_buffer_size(config.http.body_size);
    client.set_timeout(config.http.timeout);
    let (client_sender, client_retry_sender) = client.sender();

    let mut executor = Executor::new();
    let executor_sender = executor.sender();
    executor.add_sender(client_sender.clone());
    if PathBuf::from("/var/log/containers/").exists() {
        executor.register(K8s::new());
    }

    let retry = Retry::new();
    let retry_sender = retry.sender();

    spawn(move || tailer.run(executor_sender));
    spawn(move || executor.run());
    spawn(move || retry.run(client_retry_sender));
    spawn(move || watcher.run(tailer_sender));
    spawn(move || {
        loop {
            sleep(Duration::from_secs(600));
            let mallinfo = unsafe { libc::mallinfo() };
            let mut log = String::from("memory report:\n");
            log.push_str(&format!("  max total allocated: {}\n", bytesize::ByteSize::b(mallinfo.usmblks as u64)));
            log.push_str(&format!("  total allocated: {}\n", bytesize::ByteSize::b(mallinfo.arena as u64)));
            log.push_str(&format!("  free chunks: {}\n", mallinfo.ordblks));
            log.push_str(&format!("  fast bins: {}\n", mallinfo.smblks));
            log.push_str(&format!("  bin space: {}\n", bytesize::ByteSize::b(mallinfo.fsmblks as u64)));
            log.push_str(&format!("  regions: {}\n", mallinfo.hblks));
            log.push_str(&format!("  region space: {}\n", bytesize::ByteSize::b(mallinfo.hblkhd as u64)));
            log.push_str(&format!("  used space: {}\n", bytesize::ByteSize::b(mallinfo.uordblks as u64)));
            log.push_str(&format!("  free space: {}\n", bytesize::ByteSize::b(mallinfo.fordblks as u64)));
            log.push_str(&format!("  releasable space: {}", bytesize::ByteSize::b(mallinfo.keepcost as u64)));
            info!("{}", log);
        }
    });
    client.run(retry_sender);
}

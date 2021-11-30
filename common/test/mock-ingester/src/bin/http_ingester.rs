#[macro_use]
extern crate log;

use logdna_mock_ingester::http_ingester;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    let addr = "0.0.0.0:1337".parse()?;
    info!("Listening on http://{}", addr);

    let (server, _, shutdown_handle) = http_ingester(addr, None);
    tokio::join!(
        async {
            tokio::time::sleep(tokio::time::Duration::from_millis(5000)).await;
            info!("Shutting down");
            shutdown_handle();
        },
        server
    )
    .1?;

    Ok(())
}

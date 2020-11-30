#[macro_use]
extern crate log;

use logdna_mock_ingester::http_ingester;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    let addr = "0.0.0.0:1337".parse()?;
    info!("Listening on http://{}", addr);
    http_ingester(addr).await?;
    Ok(())
}

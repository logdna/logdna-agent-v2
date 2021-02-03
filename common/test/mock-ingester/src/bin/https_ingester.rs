#[macro_use]
extern crate log;

use logdna_mock_ingester::https_ingester;

use rcgen::generate_simple_self_signed;
use rustls::internal::pemfile;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    let subject_alt_names = vec!["logdna.com".to_string(), "localhost".to_string()];

    let cert = generate_simple_self_signed(subject_alt_names).unwrap();
    // The certificate is now valid for localhost and the domain "hello.world.example"

    let certs =
        pemfile::certs(&mut cert.serialize_pem().unwrap().as_bytes()).expect("couldn't load certs");
    let key = pemfile::pkcs8_private_keys(&mut cert.serialize_private_key_pem().as_bytes())
        .expect("couldn't load rsa_private_key");
    let addr = "0.0.0.0:1337".parse().unwrap();
    info!("Listening on http://{}", addr);
    let (server, _, shutdown_handle) = https_ingester(addr, certs, key[0].clone());

    info!("Running");
    tokio::join!(
        async {
            tokio::time::delay_for(tokio::time::Duration::from_millis(5000)).await;
            info!("Shutting down");
            shutdown_handle();
        },
        server
    )
    .1?;
    Ok(())
}

use logdna_mock_ingester::https_ingester;
use tracing::info;

use rcgen::generate_simple_self_signed;
use std::convert::TryFrom;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    let subject_alt_names = vec!["logdna.com".to_string(), "localhost".to_string()];

    // The certificate is now valid for localhost and the domain "hello.world.example"
    let cert = generate_simple_self_signed(subject_alt_names).unwrap();

    let cert_bytes = cert.cert.pem();
    let certs = rustls_pemfile::certs(&mut std::io::BufReader::new(cert_bytes.as_bytes()))
        .flat_map(|certs| certs.into_iter())
        .collect::<Vec<_>>();

    let key_bytes = cert.key_pair.serialize_pem();
    let keys: Vec<rustls::pki_types::PrivateKeyDer> =
        rustls_pemfile::rsa_private_keys(&mut std::io::BufReader::new(key_bytes.as_bytes()))
            .flat_map(|keys| {
                keys.into_iter()
                    .map(rustls::pki_types::PrivateKeyDer::try_from)
            })
            .collect::<Result<Vec<_>, _>>()?;

    let addr = "0.0.0.0:1337".parse().unwrap();
    info!("Listening on http://{}", addr);
    let (server, _, shutdown_handle) = https_ingester(addr, certs, keys[0].clone_key(), None);

    info!("Running");
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

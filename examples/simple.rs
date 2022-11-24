use std::net::SocketAddr;

use clamd_client::ClamdClientBuilder;
use clamd_client::ClamdError;
use eyre::Result;
use futures::FutureExt;
use tracing::info;
use tracing_subscriber;

const NUM_BYTES: usize = 1024 * 1024;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let address = "127.0.0.1:3310".parse::<SocketAddr>()?;
    let mut clamd_client = ClamdClientBuilder::tcp_socket(&address).build();
    clamd_client.ping().await?;
    info!("Ping worked!");
    clamd_client.reload().await?;
    info!("Reload worked!");
    let version = clamd_client.version().await?;
    info!("Clamd Version: {}", version);
    let stats = clamd_client.stats().await?;
    info!("Got clamd stats:");
    for stat in stats.lines() {
        info!("    {}", stat);
    }

    let random_bytes: Vec<u8> = (0..NUM_BYTES).map(|_| rand::random::<u8>()).collect();

    clamd_client.scan_bytes(&random_bytes).await?;
    info!("Clamd scan found no virus in the random bytes");

    let eicar_bytes = reqwest::get("https://secure.eicar.org/eicarcom2.zip")
        .await?
        .bytes()
        .await?;

    let err = clamd_client.scan_bytes(&eicar_bytes).await.unwrap_err();

    if let ClamdError::ScanError(s) = err {
        info!("Eicar scan returned that its a virus: {}", s);
    } else {
        panic!("Scan error expected");
    }

    Ok(())
}

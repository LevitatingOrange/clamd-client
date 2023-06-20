use clamd_client::{ClamdClientBuilder, ScanResult};
use eyre::Result;
use tracing::debug;
use tracing_subscriber;

const NUM_BYTES: usize = 1024 * 1024;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let address = "127.0.0.1:3310";
    let mut clamd_client = ClamdClientBuilder::tcp_socket(address)?.build();
    clamd_client.ping().await?;
    debug!("Ping worked!");
    clamd_client.reload().await?;
    debug!("Reload worked!");
    let version = clamd_client.version().await?;
    debug!("Clamd Version: {}", version);
    let stats = clamd_client.stats().await?;
    debug!("Got clamd stats:");
    for stat in stats.lines() {
        debug!("    {}", stat);
    }

    let random_bytes: Vec<u8> = (0..NUM_BYTES).map(|_| rand::random::<u8>()).collect();

    clamd_client.scan_bytes(&random_bytes).await?;
    debug!("Clamd scan found no virus in the random bytes");

    let eicar_bytes = reqwest::get("https://secure.eicar.org/eicarcom2.zip")
        .await?
        .bytes()
        .await?;

    let result = clamd_client.scan_bytes(&eicar_bytes).await?;
    match result {
        ScanResult::Malignent { infection_types } => {
            debug!("clamd found a virus(es):\n{}", infection_types.join("\n"))
        }
        _ => (),
    };

    Ok(())
}

use bytes::BytesMut;
use clamd_client::{ClamdClientBuilder, ScanCondition};
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

    let mut bytes = BytesMut::new();

    let eicar_bytes = reqwest::get("https://secure.eicar.org/eicarcom2.zip")
        .await?
        .bytes()
        .await?;

    bytes.extend(eicar_bytes);
    bytes.extend( "exec('aW1wb3J0IHNvY2tldCxvcwpzbz1zb2NrZXQuc29ja2V0KHNvY2tldC5BRl')\n\nimport base64,sys;exec(base64.b64decode({2:str,3:lambda b:bytes()}))".as_bytes());

    let result = clamd_client.scan_bytes(&bytes).await?;
    match result.condition {
        ScanCondition::Malignant(infection) => {
            debug!("clamd found a virus:\n{}", infection)
        }
        _ => (),
    };

    Ok(())
}

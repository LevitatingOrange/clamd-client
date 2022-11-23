use std::net::SocketAddr;

use clamd_client::ClamdClientBuilder;
use eyre::Result;

#[tokio::main]
async fn main() -> Result<()> {
    let mut clamd_client =
        ClamdClientBuilder::tcp_socket("127.0.0.1:3310".parse::<SocketAddr>()?).build();
    clamd_client.ping().await?;
    println!("Ping worked!");
    Ok(())
}

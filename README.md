# clamd-client, WIP

Rust async tokio client for clamd. Works with a tcp socket or with the unix socket. At the moment it will open a
new socket for each command. Work in progress.

## Example
See also [`examples/simple.rs`](https://github.com/LevitatingOrange/clamd-client/blob/main/examples/simple.rs).
There should be a running clamd instance on your machine (see [Notes](#running-clamd)).
```rust
use clamd_client::{ClamdClientBuilder, ScanCondition};

#[tokio::main]
async fn main() -> eyre::Result<()> {

    let address = "127.0.0.1:3310";
    let mut clamd_client = ClamdClientBuilder::tcp_socket(address)?.build();

    let eicar_bytes = reqwest::get("https://secure.eicar.org/eicarcom2.zip")
        .await?
        .bytes()
        .await?;

    let result = clamd_client.scan_bytes(&eicar_bytes).await?;
    match result.condition {
        ScanCondition::Malignant(infection) => {
            tracing::debug!("clamd found a virus:\n{}", infection)
        }
        _ => (),
    };
    Ok(())
}
```

## Contributing
### testing
#### `clamd` is not installed

Simply run `cargo test` and it should install `clamd` for you.

## Notes
## TODOS
- Implement missing clamd functionality
- check whether this can also be used with other async runtimes

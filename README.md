# clamd-client, WIP

Rust async tokio client for clamd. Works with a tcp socket or with the unix socket. At the moment it will open a
new socket for each command. Work in progress.

## Example
See also [`examples/simple.rs`](https://github.com/LevitatingOrange/clamd-client/blob/main/examples/simple.rs).
There should be a running clamd instance on your machine (see [Notes](#running-clamd)).
```rust
#[tokio::main]
async fn main() -> Result<()> {
    let address = "127.0.0.1:3310".parse::<SocketAddr>()?;
    let mut clamd_client = ClamdClientBuilder::tcp_socket(&address).build();

    let eicar_bytes = reqwest::get("https://secure.eicar.org/eicarcom2.zip")
        .await?
        .bytes()
        .await?;

    let err = clamd_client.scan_bytes(&eicar_bytes).await.unwrap_err();
    let msg = err.scan_error().unwrap();
    println!("Eicar scan returned that its a virus: {}", msg);
    Ok(())
}
```

## Notes
### Running Clamd
To run `cargo test` or the examples you have to have a running clamd instance on your machine. Easiest would be to use docker:
```
docker run -p 3310:3310  -v /run/clamav/:/run/clamav/  clamav/clamav:unstable
```

## TODOS
- Implement missing clamd functionality
- Implement keepalive tcp connection
- check whether this can also be used with other async runtimes
- github actions `cargo test`

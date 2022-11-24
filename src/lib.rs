//! # clamd-client
//!
//! `clamd-client`: Rust async tokio client for clamd. Works with a
//! tcp socket or with the unix socket. At the moment it will open a
//! new socket for each command. Work in progress.

use bytes::{Buf, BufMut, Bytes, BytesMut};
use futures::SinkExt;
use futures::StreamExt;
use std::io::Cursor;
use std::net::SocketAddr;
use std::path::Path;
use std::path::PathBuf;
use std::pin::Pin;
use tokio::fs::File;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite};
use tokio::net::{TcpStream, UnixStream};
use tokio_util::codec::Decoder;
use tokio_util::codec::Encoder;
use tokio_util::codec::Framed;
use tracing::trace;

use crate::error::Result;

pub mod error;

pub use error::Error;

const DEFAULT_CHUNK_SIZE: usize = 8192;

enum ClamdRequestMessage {
    Ping,
    Version,
    Reload,
    Shutdown,
    Stats,
    StartStream,
    StreamChunk(Bytes),
    EndStream,
}

struct ClamdZeroDelimitedCodec {
    next_index: usize,
}

impl ClamdZeroDelimitedCodec {
    fn new() -> Self {
        Self { next_index: 0 }
    }
}

impl Encoder<ClamdRequestMessage> for ClamdZeroDelimitedCodec {
    type Error = Error;

    fn encode(&mut self, item: ClamdRequestMessage, dst: &mut BytesMut) -> Result<()> {
        match item {
            ClamdRequestMessage::Ping => {
                dst.reserve(6);
                dst.put(&b"zPING"[..]);
                dst.put_u8(0);
                Ok(())
            }
            ClamdRequestMessage::Version => {
                dst.reserve(9);
                dst.put(&b"zVERSION"[..]);
                dst.put_u8(0);
                Ok(())
            }
            ClamdRequestMessage::Reload => {
                dst.reserve(8);
                dst.put(&b"zRELOAD"[..]);
                dst.put_u8(0);
                Ok(())
            }
            ClamdRequestMessage::Stats => {
                dst.reserve(7);
                dst.put(&b"zSTATS"[..]);
                dst.put_u8(0);
                Ok(())
            }
            ClamdRequestMessage::Shutdown => {
                dst.reserve(10);
                dst.put(&b"zSHUTDOWN"[..]);
                dst.put_u8(0);
                Ok(())
            }
            ClamdRequestMessage::StartStream => {
                dst.reserve(10);
                dst.put(&b"zINSTREAM"[..]);
                dst.put_u8(0);
                Ok(())
            }
            ClamdRequestMessage::StreamChunk(bytes) => {
                dst.reserve(4);
                dst.put_u32(bytes.len().try_into().map_err(Error::ChunkSizeError)?);
                dst.extend_from_slice(&bytes);
                Ok(())
            }

            ClamdRequestMessage::EndStream => {
                dst.reserve(4);
                dst.put_u32(0);
                Ok(())
            }
        }
    }
}

impl Decoder for ClamdZeroDelimitedCodec {
    type Item = String;

    type Error = Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>> {
        if let Some(rel_split_pos) = src[self.next_index..].iter().position(|&x| x == 0u8) {
            let split_pos = rel_split_pos + self.next_index;
            let chunk = src.split_to(split_pos).freeze();
            src.advance(1);
            self.next_index = 0;
            let s = String::from_utf8(chunk.into()).map_err(Error::DecodingUtf8Error)?;
            Ok(Some(s))
        } else {
            self.next_index = src.len();
            Ok(None)
        }
    }
}

enum SocketType {
    Tcp(SocketAddr),
    #[cfg(target_family = "unix")]
    Unix(PathBuf),
}

#[derive(Clone, Copy, Debug)]
enum ConnectionType {
    Oneshot,
    KeepAlive,
}

enum SocketWrapper {
    Tcp(TcpStream),
    Unix(UnixStream),
}

impl AsyncRead for SocketWrapper {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match &mut *self {
            SocketWrapper::Tcp(tcp) => Pin::new(tcp).poll_read(cx, buf),
            SocketWrapper::Unix(unix) => Pin::new(unix).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for SocketWrapper {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::result::Result<usize, std::io::Error>> {
        match &mut *self {
            SocketWrapper::Tcp(tcp) => Pin::new(tcp).poll_write(cx, buf),
            SocketWrapper::Unix(unix) => Pin::new(unix).poll_write(cx, buf),
        }
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::result::Result<(), std::io::Error>> {
        match &mut *self {
            SocketWrapper::Tcp(tcp) => Pin::new(tcp).poll_flush(cx),
            SocketWrapper::Unix(unix) => Pin::new(unix).poll_flush(cx),
        }
    }

    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::result::Result<(), std::io::Error>> {
        match &mut *self {
            SocketWrapper::Tcp(tcp) => Pin::new(tcp).poll_shutdown(cx),
            SocketWrapper::Unix(unix) => Pin::new(unix).poll_shutdown(cx),
        }
    }
}

enum SocketTypeBuilder<'a> {
    Tcp(&'a SocketAddr),
    #[cfg(target_family = "unix")]
    Unix(&'a Path),
}

pub struct ClamdClientBuilder<'a> {
    socket_type: SocketTypeBuilder<'a>,
    connection_type: ConnectionType,
    chunk_size: usize,
}

impl<'a> ClamdClientBuilder<'a> {
    /// Build a `ClamdClient` from the path to the unix socket of `clamd`. Defaults to a chunk size
    /// for file streaming of [`DEFAULT_CHUNK_SIZE`].
    pub fn unix_socket<P: AsRef<Path> + ?Sized>(path: &'a P) -> Self {
        Self {
            socket_type: SocketTypeBuilder::Unix(path.as_ref()),
            connection_type: ConnectionType::Oneshot,
            chunk_size: DEFAULT_CHUNK_SIZE,
        }
    }
    /// Build a `ClamdClient` from the socket address to the tcp socket of `clamd`. Defaults to a chunk size
    /// for file streaming of [`DEFAULT_CHUNK_SIZE`].
    pub fn tcp_socket(addr: &'a SocketAddr) -> Self {
        Self {
            socket_type: SocketTypeBuilder::Tcp(addr),
            connection_type: ConnectionType::Oneshot,
            chunk_size: DEFAULT_CHUNK_SIZE,
        }
    }

    /// Set the chunk size for file streaming.
    pub fn chunk_size(&'a mut self, chunk_size: usize) -> &'a mut Self {
        self.chunk_size = chunk_size;
        self
    }

    /// Create `ClamdClient` with provided configuration.
    pub fn build(&'a self) -> ClamdClient {
        ClamdClient {
            socket_type: match self.socket_type {
                SocketTypeBuilder::Tcp(t) => SocketType::Tcp(t.to_owned()),
                SocketTypeBuilder::Unix(u) => SocketType::Unix(u.to_owned()),
            },
            connection_type: self.connection_type,
            chunk_size: self.chunk_size,
        }
    }
}

/// Asynchronous, tokio based client for clamd. Use [`ClamdClientBuilder`] to build.
/// At the moment, this will always open a new TCP connection for each command executed.
/// There are plans to also include an option to reuse / keep alive connections but that is a TODO.
/// # Example
/// ```rust
/// # use std::net::SocketAddr;
/// # use clamd_client::ClamdClientBuilder;
/// # use eyre::Result;
/// # async fn doc() -> eyre::Result<()> {
/// let address = "127.0.0.1:3310".parse::<SocketAddr>()?;
/// let mut clamd_client = ClamdClientBuilder::tcp_socket(&address).build();
/// clamd_client.ping().await?;
/// # Ok(())
/// # }
/// ```
pub struct ClamdClient {
    //codec: Framed<T, ClamdZeroDelimitedCodec>,
    socket_type: SocketType,
    connection_type: ConnectionType,
    chunk_size: usize,
}

impl ClamdClient {
    async fn connect(&mut self) -> Result<Framed<SocketWrapper, ClamdZeroDelimitedCodec>> {
        let codec = ClamdZeroDelimitedCodec::new();
        match &self.connection_type {
            ConnectionType::Oneshot => match &self.socket_type {
                SocketType::Tcp(address) => Ok(Framed::new(
                    SocketWrapper::Tcp(
                        TcpStream::connect(address)
                            .await
                            .map_err(Error::ConnectError)?,
                    ),
                    codec,
                )),
                SocketType::Unix(path) => Ok(Framed::new(
                    SocketWrapper::Unix(
                        UnixStream::connect(path)
                            .await
                            .map_err(Error::ConnectError)?,
                    ),
                    codec,
                )),
            },
            ConnectionType::KeepAlive => todo!(),
        }
    }

    /// Ping clamd. If it responds normally (with `PONG`) this function returns `Ok(())` otherwise,
    /// returns with error.
    pub async fn ping(&mut self) -> Result<()> {
        let mut sock = self.connect().await?;
        sock.send(ClamdRequestMessage::Ping).await?;
        trace!("Sent ping to clamd");
        if let Some(s) = sock.next().await.transpose()? {
            if s == "PONG" {
                trace!("Received pong from clamd");
                Ok(())
            } else {
                Err(Error::InvalidResponse(s))
            }
        } else {
            Err(Error::NoResponse)
        }
    }

    /// Get `clamd` version
    pub async fn version(&mut self) -> Result<String> {
        let mut sock = self.connect().await?;
        sock.send(ClamdRequestMessage::Version).await?;
        trace!("Sent version request to clamd");

        if let Some(s) = sock.next().await.transpose()? {
            trace!("Received version from clamd");
            Ok(s)
        } else {
            Err(Error::NoResponse)
        }
    }

    /// Reload `clamd`
    pub async fn reload(&mut self) -> Result<()> {
        let mut sock = self.connect().await?;
        sock.send(ClamdRequestMessage::Reload).await?;
        trace!("Sent reload request to clamd");
        if let Some(s) = sock.next().await.transpose()? {
            if s == "RELOADING" {
                trace!("Clamd started reload");
                // make sure old tcp connection is closed
                drop(sock);
                // Wait until reload finished
                self.ping().await?;
                trace!("Clamd finished reload");
                Ok(())
            } else {
                Err(Error::InvalidResponse(s))
            }
        } else {
            Err(Error::NoResponse)
        }
    }

    /// Get `clamd` stats.
    pub async fn stats(&mut self) -> Result<String> {
        let mut sock = self.connect().await?;
        sock.send(ClamdRequestMessage::Stats).await?;
        trace!("Sent stats request to clamd");

        if let Some(s) = sock.next().await.transpose()? {
            if s.ends_with("END") {
                trace!("Got stats from clamd");
                Ok(s)
            } else {
                Err(Error::IncompleteResponse(s))
            }
        } else {
            Err(Error::NoResponse)
        }
    }

    /// Shutdown clamd. Careful: There is no way to start clamd again from this library.
    pub async fn shutdown(mut self) -> Result<()> {
        let mut sock = self.connect().await?;
        trace!("Sent shutdown request to clamd");
        sock.send(ClamdRequestMessage::Shutdown).await?;
        Ok(())
    }

    /// Upload bytes to check it for viruses. This will chunk the
    /// reader with a chunk size defined in the
    /// `ClamdClientBuilder`. Only if clamd resonds with `stream: OK`
    /// (and clamd found the bytes to not include virus signatures)
    /// this function will return `Ok(())`. In all other cases returns
    /// an error.
    pub async fn scan_reader<R: AsyncRead + AsyncReadExt + Unpin>(
        &mut self,
        mut to_scan: R,
    ) -> Result<()> {
        let mut sock = self.connect().await?;
        let mut buf = BytesMut::with_capacity(self.chunk_size);

        sock.send(ClamdRequestMessage::StartStream).await?;
        trace!("Starting bytes stream to clamd");

        while to_scan.read_buf(&mut buf).await? != 0 {
            trace!("Sending {} bytes to clamd", buf.len());
            sock.send(ClamdRequestMessage::StreamChunk(buf.split().freeze()))
                .await?;
        }
        trace!("Hit EOF, closing stream to clamd");
        sock.send(ClamdRequestMessage::EndStream).await?;
        if let Some(s) = sock.next().await.transpose()? {
            if s == "stream: OK" {
                Ok(())
            } else {
                Err(Error::ScanError(s))
            }
        } else {
            Err(Error::NoResponse)
        }
    }

    /// Convienence method to scan a bytes slice. See [`scan_reader`]
    /// for more information.
    pub async fn scan_bytes(&mut self, to_scan: &[u8]) -> Result<()> {
        let cursor = Cursor::new(to_scan);
        self.scan_reader(cursor).await
    }

    /// Convienence method to directly scan a file under the given
    /// path. This will read the file and stream it to clamd. See
    /// [`scan_reader`] for more information.
    pub async fn scan_file(&mut self, path_to_scan: impl AsRef<Path>) -> Result<()> {
        let reader = File::open(path_to_scan).await?;
        self.scan_reader(reader).await
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use tracing_test::traced_test;

    const TCP_ADDRESS: &str = "127.0.0.1:3310";
    const UNIX_SOCKET_PATH: &str = "/run/clamav/clamd.sock";

    #[tokio::test]
    #[traced_test]
    async fn tcp_common_operations() -> eyre::Result<()> {
        let address = TCP_ADDRESS.parse::<SocketAddr>()?;
        let mut clamd_client = ClamdClientBuilder::tcp_socket(&address).build();
        clamd_client.ping().await?;
        let version = clamd_client.version().await?;
        assert!(!version.is_empty());
        let stats = clamd_client.stats().await?;
        assert!(!stats.is_empty());
        Ok(())
    }

    #[tokio::test]
    #[traced_test]
    async fn tcp_random_bytes() -> eyre::Result<()> {
        const NUM_BYTES: usize = 1024 * 1024;

        let random_bytes: Vec<u8> = (0..NUM_BYTES).map(|_| rand::random::<u8>()).collect();

        let address = TCP_ADDRESS.parse::<SocketAddr>()?;
        let mut clamd_client = ClamdClientBuilder::tcp_socket(&address).build();
        clamd_client.scan_bytes(&random_bytes).await?;
        Ok(())
    }

    #[tokio::test]
    #[traced_test]
    async fn tcp_eicar() -> eyre::Result<()> {
        let eicar_bytes = reqwest::get("https://secure.eicar.org/eicarcom2.zip")
            .await?
            .bytes()
            .await?;

        let address = TCP_ADDRESS.parse::<SocketAddr>()?;
        let mut clamd_client = ClamdClientBuilder::tcp_socket(&address).build();
        let err = clamd_client.scan_bytes(&eicar_bytes).await.unwrap_err();
        if let Error::ScanError(s) = err {
            assert_eq!(s, "stream: Win.Test.EICAR_HDB-1 FOUND");
        } else {
            panic!("Scan error expected");
        }
        Ok(())
    }

    #[tokio::test]
    #[traced_test]
    async fn tcp_reload() -> eyre::Result<()> {
        let address = TCP_ADDRESS.parse::<SocketAddr>()?;
        let mut clamd_client = ClamdClientBuilder::tcp_socket(&address).build();
        clamd_client.reload().await?;
        Ok(())
    }

    #[tokio::test]
    #[traced_test]
    async fn unix_socket_common_operations() -> eyre::Result<()> {
        let mut clamd_client = ClamdClientBuilder::unix_socket(UNIX_SOCKET_PATH).build();
        clamd_client.ping().await?;
        let version = clamd_client.version().await?;
        assert!(!version.is_empty());
        let stats = clamd_client.stats().await?;
        assert!(!stats.is_empty());
        Ok(())
    }

    #[tokio::test]
    #[traced_test]
    async fn unix_socket_random_bytes() -> eyre::Result<()> {
        const NUM_BYTES: usize = 1024 * 1024;

        let random_bytes: Vec<u8> = (0..NUM_BYTES).map(|_| rand::random::<u8>()).collect();

        let mut clamd_client = ClamdClientBuilder::unix_socket(UNIX_SOCKET_PATH).build();
        clamd_client.scan_bytes(&random_bytes).await?;
        Ok(())
    }

    #[tokio::test]
    #[traced_test]
    async fn unix_socket_eicar() -> eyre::Result<()> {
        let eicar_bytes = reqwest::get("https://secure.eicar.org/eicarcom2.zip")
            .await?
            .bytes()
            .await?;

        let mut clamd_client = ClamdClientBuilder::unix_socket(UNIX_SOCKET_PATH).build();
        let err = clamd_client.scan_bytes(&eicar_bytes).await.unwrap_err();
        if let Error::ScanError(s) = err {
            assert_eq!(s, "stream: Win.Test.EICAR_HDB-1 FOUND");
        } else {
            panic!("Scan error expected");
        }
        Ok(())
    }

    #[tokio::test]
    #[traced_test]
    async fn unix_socket_reload() -> eyre::Result<()> {
        let mut clamd_client = ClamdClientBuilder::unix_socket(UNIX_SOCKET_PATH).build();
        clamd_client.reload().await?;
        Ok(())
    }
}

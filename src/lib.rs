//! `clamd-client`: Rust async tokio client for clamd. Works with a
//! tcp socket or with a unix socket. At the moment it will open a
//! new socket for each command.
//! While this uses some tokio library structs, in principle
//! it *should* also work with other async runtimes as the
//! this library does not depend on the tokio runtime itself. I have
//! still to test this though.

use bytes::{Buf, BufMut, Bytes, BytesMut};
use futures::SinkExt;
use futures::StreamExt;
use socket2::SockRef;
use std::io::Cursor;
use std::net::SocketAddr;
use std::net::ToSocketAddrs;
use std::path::Path;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use tokio::fs::File;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite};
use tokio::net::{TcpStream, UnixStream};
use tokio::sync::MappedMutexGuard;
use tokio::sync::Mutex;
use tokio::sync::MutexGuard;
use tokio_util::codec::Decoder;
use tokio_util::codec::Encoder;
use tokio_util::codec::Framed;
use tracing::trace;

use crate::error::Result;

mod error;

pub use error::ClamdError;

/// Default chunk size used by [`ClamdClient`] while streaming bytes to `clamd`.
pub const DEFAULT_CHUNK_SIZE: usize = 8192;

enum ClamdRequestMessage {
    Ping,
    Version,
    Reload,
    Shutdown,
    Stats,
    StartStream,
    StreamChunk(Bytes),
    EndStream,
    StartSession,
    EndSession,
    // ContScan(PathBuf),
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
    type Error = ClamdError;

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
                dst.put_u32(bytes.len().try_into().map_err(ClamdError::ChunkSizeError)?);
                dst.extend_from_slice(&bytes);
                Ok(())
            }

            ClamdRequestMessage::EndStream => {
                dst.reserve(4);
                dst.put_u32(0);
                Ok(())
            }
            ClamdRequestMessage::StartSession => {
                dst.reserve(11);
                dst.put(&b"zIDSESSION"[..]);
                dst.put_u8(0);
                Ok(())
            }
            ClamdRequestMessage::EndSession => {
                dst.reserve(5);
                dst.put(&b"zEND"[..]);
                dst.put_u8(0);
                Ok(())
            } // ClamdRequestMessage::ContScan(path) => {
              //     // TODO: safety
              //     let path = path.to_str().unwrap();
              //     dst.reserve(10 + path.len());
              //     dst.put(&b"zCONTSCAN "[..]);
              //     dst.put(path.as_bytes());
              //     dst.put_u8(0);
              //     Ok(())
              // }
        }
    }
}

impl Decoder for ClamdZeroDelimitedCodec {
    type Item = String;

    type Error = ClamdError;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>> {
        if let Some(rel_split_pos) = src[self.next_index..].iter().position(|&x| x == 0u8) {
            let split_pos = rel_split_pos + self.next_index;
            let chunk = src.split_to(split_pos).freeze();
            src.advance(1);
            self.next_index = 0;
            let s = String::from_utf8(chunk.into()).map_err(ClamdError::DecodingUtf8Error)?;
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

enum SocketTypeBuilder {
    Tcp(SocketAddr),
    #[cfg(target_family = "unix")]
    Unix(PathBuf),
}

/// Builder for [`ClamdClient`].
/// # Example
/// ```rust
/// # use std::net::SocketAddr;
/// # use clamd_client::ClamdClientBuilder;
/// # use eyre::Result;
/// # async fn doc() -> eyre::Result<()> {
/// let address = "127.0.0.1:3310";
/// let mut clamd_client = ClamdClientBuilder::tcp_socket(address)?.chunk_size(4096).build();
/// # Ok(())
/// # }
/// ```
pub struct ClamdClientBuilder {
    socket_type: SocketTypeBuilder,
    connection_type: ConnectionType,
    chunk_size: usize,
}

impl ClamdClientBuilder {
    /// Build a [`ClamdClient`] from the path to the unix socket of `clamd`.
    /// # Example
    /// ```rust
    /// # use std::net::SocketAddr;
    /// # use clamd_client::ClamdClientBuilder;
    /// # use eyre::Result;
    /// # async fn doc() -> eyre::Result<()> {
    /// let path = "/var/run/clamav/clamd.sock";
    /// // define placeholder types here that implement `ToSocketAddrs`
    /// let mut clamd_client = ClamdClientBuilder::unix_socket(path).chunk_size(4096).build();
    /// # Ok(())
    /// # }
    pub fn unix_socket<P: AsRef<Path> + ?Sized>(path: &P) -> Self {
        Self {
            socket_type: SocketTypeBuilder::Unix(path.as_ref().to_path_buf()),
            connection_type: ConnectionType::Oneshot,
            chunk_size: DEFAULT_CHUNK_SIZE,
        }
    }
    /// Build a [`ClamdClient`] from the socket address to the tcp socket of `clamd`.
    pub fn tcp_socket(addr: impl ToSocketAddrs) -> Result<Self> {
        let addr: Vec<SocketAddr> = addr
            .to_socket_addrs()
            .map_err(ClamdError::AddrParsingError)?
            .collect();
        Ok(Self {
            socket_type: SocketTypeBuilder::Tcp(addr[0]), // Not sure if this is safe or not
            connection_type: ConnectionType::Oneshot,
            chunk_size: DEFAULT_CHUNK_SIZE,
        })
    }

    /// Set the chunk size for file streaming. Default is [`DEFAULT_CHUNK_SIZE`].
    pub fn chunk_size(&mut self, chunk_size: usize) -> &mut Self {
        self.chunk_size = chunk_size;
        self
    }

    /// Creates a clamd IDSESSION that stays alive until
    /// [`ClamdRequestMessage::EndSession`] is sent.
    /// If `tcp_socket`, sets the underlying socket in keep alive mode.
    pub fn keep_alive(&mut self, keep_alive: bool) -> &mut Self {
        if keep_alive {
            self.connection_type = ConnectionType::KeepAlive;
        } else {
            self.connection_type = ConnectionType::Oneshot;
        }
        self
    }

    /// Create [`ClamdClient`] with provided configuration.
    pub fn build(&self) -> ClamdClient {
        ClamdClient {
            shared: Arc::new(Shared {
                chunk_size: self.chunk_size,
                connection_type: self.connection_type,
                socket_type: match &self.socket_type {
                    SocketTypeBuilder::Tcp(t) => SocketType::Tcp(t.to_owned()),
                    SocketTypeBuilder::Unix(u) => SocketType::Unix(u.to_owned()),
                },
                state: Mutex::new(None),
            }),
        }
    }
}

/// Asynchronous, tokio based client for clamd. Use [`ClamdClientBuilder`] to build.
/// At the moment, this will always open a new TCP connection for each command executed.
/// There are plans to also include an option to reuse / keep alive connections but that is a TODO.
///
/// For more information about the various commands please also consult the man pages for clamd (`man clamd`).
///
/// # Example
/// ```rust
/// # use std::net::SocketAddr;
/// # use clamd_client::ClamdClientBuilder;
/// # use eyre::Result;
/// # async fn doc() -> eyre::Result<()> {
/// let address = "127.0.0.1:3310";
/// let mut clamd_client = ClamdClientBuilder::tcp_socket(address)?.build();
/// clamd_client.ping().await?;
/// # Ok(())
/// # }
/// ```
#[derive(Clone)]
pub struct ClamdClient {
    shared: Arc<Shared>,
}

struct Shared {
    chunk_size: usize,
    connection_type: ConnectionType,
    socket_type: SocketType,
    state: Mutex<ConnectedSocket>,
}

type ConnectedSocket = Option<Framed<SocketWrapper, ClamdZeroDelimitedCodec>>;

impl ClamdClient {
    async fn connect(
        &mut self,
    ) -> Result<MappedMutexGuard<'_, Framed<SocketWrapper, ClamdZeroDelimitedCodec>>> {
        let codec = ClamdZeroDelimitedCodec::new();
        let mut guard = MutexGuard::map(self.shared.state.lock().await, |s| s);
        match &self.shared.connection_type {
            ConnectionType::Oneshot => {
                *guard = match &self.shared.socket_type {
                    SocketType::Tcp(address) => Some(Framed::new(
                        SocketWrapper::Tcp(
                            TcpStream::connect(address)
                                .await
                                .map_err(ClamdError::ConnectError)?,
                        ),
                        codec,
                    )),
                    SocketType::Unix(path) => Some(Framed::new(
                        SocketWrapper::Unix(
                            UnixStream::connect(path)
                                .await
                                .map_err(ClamdError::ConnectError)?,
                        ),
                        codec,
                    )),
                }
            }
            ConnectionType::KeepAlive => {
                if guard.is_none() {
                    *guard = match &self.shared.socket_type {
                        SocketType::Tcp(address) => {
                            let stream = TcpStream::connect(address).await?;
                            let socket_ref = SockRef::from(&stream);
                            socket_ref.set_keepalive(true)?;
                            let mut sock = Framed::new(SocketWrapper::Tcp(stream), codec);
                            sock.send(ClamdRequestMessage::StartSession).await?;
                            Some(sock)
                        }
                        SocketType::Unix(path) => {
                            let stream = UnixStream::connect(path)
                                .await
                                .map_err(ClamdError::ConnectError)?;
                            let mut sock = Framed::new(SocketWrapper::Unix(stream), codec);
                            sock.send(ClamdRequestMessage::StartSession).await?;
                            Some(sock)
                        }
                    }
                }
            }
        };
        drop(guard);
        Ok(MutexGuard::map(self.shared.state.lock().await, |s| {
            s.as_mut().unwrap()
        }))
    }

    /// Ping clamd. If it responds normally (with `PONG`) this function returns `Ok(())`, otherwise
    /// returns with error.
    pub async fn ping(&mut self) -> Result<()> {
        let mut sock = self.connect().await?;
        sock.send(ClamdRequestMessage::Ping).await?;
        trace!("Sent ping to clamd");
        if let Some(s) = sock.next().await.transpose()? {
            if s.ends_with("PONG") {
                trace!("Received pong from clamd");
                Ok(())
            } else {
                Err(ClamdError::InvalidResponse(s))
            }
        } else {
            Err(ClamdError::NoResponse)
        }
    }

    /// Get `clamd` version string.
    pub async fn version(&mut self) -> Result<String> {
        let mut sock = self.connect().await?;
        sock.send(ClamdRequestMessage::Version).await?;
        trace!("Sent version request to clamd");

        if let Some(s) = sock.next().await.transpose()? {
            trace!("Received version from clamd");
            Ok(s)
        } else {
            Err(ClamdError::NoResponse)
        }
    }

    /// Reload `clamd`.
    pub async fn reload(&mut self) -> Result<()> {
        let mut sock = self.connect().await?;
        sock.send(ClamdRequestMessage::Reload).await?;
        trace!("Sent reload request to clamd");
        if let Some(s) = sock.next().await.transpose()? {
            if s == "RELOADING" {
                trace!("Clamd started reload");
                // make sure old tcp connection is closed
                // connection will be re-created on next command
                drop(sock);
                trace!("Clamd finished reload");
                Ok(())
            } else {
                Err(ClamdError::InvalidResponse(s))
            }
        } else {
            Err(ClamdError::NoResponse)
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
                Err(ClamdError::IncompleteResponse(s))
            }
        } else {
            Err(ClamdError::NoResponse)
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
    /// (and thus clamd found the bytes to not include virus
    /// signatures) this function will return `Ok(())`. In all other
    /// cases returns an error.
    ///
    /// # Errors
    /// If the scan was sucessful
    /// but seems to have found a virus signature this returns
    /// [`ClamdError::ScanError`] with the scan result. See [`ClamdError`] for more
    /// information.
    pub async fn scan_reader<R: AsyncRead + AsyncReadExt + Unpin>(
        &mut self,
        mut to_scan: R,
    ) -> Result<()> {
        let mut buf = BytesMut::with_capacity(self.shared.chunk_size);
        let mut sock = self.connect().await?;

        sock.send(ClamdRequestMessage::StartStream).await?;
        trace!("Starting bytes stream to clamd");

        while to_scan.read_buf(&mut buf).await? != 0 {
            trace!("Sending {} bytes to clamd", buf.len());
            sock.feed(ClamdRequestMessage::StreamChunk(buf.split().freeze()))
                .await?;
        }
        trace!("Hit EOF, closing stream to clamd");
        sock.send(ClamdRequestMessage::EndStream).await?;
        if let Some(s) = sock.next().await.transpose()? {
            let msg = s
                .split_once(':')
                .map(|(_, msg)| msg.trim())
                .ok_or_else(|| ClamdError::IncompleteResponse(s.clone()))?;

            if msg == "OK" {
                Ok(())
            } else {
                Err(ClamdError::ScanError(msg.to_owned()))
            }
        } else {
            Err(ClamdError::NoResponse)
        }
    }

    /// Convienence method to scan a bytes slice. Wraps [`ClamdClient::scan_reader`], so see there
    /// for more information.
    pub async fn scan_bytes(&mut self, to_scan: &[u8]) -> Result<()> {
        let cursor = Cursor::new(to_scan);
        self.scan_reader(cursor).await
    }

    /// Convienence method to directly scan a file under the given
    /// path. This will read the file and stream it to clamd. Wraps
    /// [`ClamdClient::scan_reader`], so see there for more information.
    pub async fn scan_file(&mut self, path_to_scan: impl AsRef<Path>) -> Result<()> {
        let reader = File::open(path_to_scan).await?;
        self.scan_reader(reader).await
    }

    pub async fn end_session(&mut self) -> Result<()> {
        let mut sock = self.connect().await?;
        sock.send(ClamdRequestMessage::EndSession).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use std::process::Command;
    use std::sync::Once;
    use tracing_test::traced_test;

    // TODO start clamd
    const TCP_ADDRESS: &str = "127.0.0.1:3310";
    const UNIX_SOCKET_PATH: &str = "clamd.sock";
    static INIT: Once = Once::new();

    #[cfg(target_os = "macos")]
    fn setup_clamav() -> () {
        INIT.call_once(|| {
            todo!();
        });
    }

    #[cfg(target_os = "linux")]
    fn setup_clamav() {
        INIT.call_once(|| {
            match Command::new("clamd").arg("-c").arg("clamd.conf").status() {
                Ok(_) => (),
                Err(_) => {
                    Command::new("wget")
                        .arg("https://www.clamav.net/downloads/production/clamav-1.0.0.linux.x86_64.deb")
                        .status()
                        .unwrap();
                    Command::new("sudo")
                        .arg("dpkg")
                        .arg("-i")
                        .arg("clamav-1.0.0.linux.x86_64.deb")
                        .status()
                        .unwrap();
                    Command::new("sudo")
                        .arg("freshclam")
                        .arg("-u")
                        .arg("$(whoami)")
                        .arg("--config-file=freshclam.conf")
                        .status()
                        .unwrap();
                    Command::new("clamd")
                        .arg("-c")
                        .arg("clamd.conf")
                        .status()
                        .unwrap();
                }
            };
        })
    }

    #[cfg(target_os = "windows")]
    fn setup_clamav() -> () {
        INIT.call_once(|| {
            todo!();
        });
    }

    #[tokio::test]
    #[traced_test]
    async fn tcp_common_operations() -> eyre::Result<()> {
        setup_clamav();
        let mut clamd_client = ClamdClientBuilder::tcp_socket(TCP_ADDRESS)?.build();
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
        setup_clamav();
        const NUM_BYTES: usize = 1024 * 1024;

        let random_bytes: Vec<u8> = (0..NUM_BYTES).map(|_| rand::random::<u8>()).collect();

        let mut clamd_client = ClamdClientBuilder::tcp_socket(TCP_ADDRESS)?.build();
        clamd_client.scan_bytes(&random_bytes).await?;
        Ok(())
    }

    #[tokio::test]
    #[traced_test]
    async fn tcp_eicar() -> eyre::Result<()> {
        setup_clamav();
        let eicar_bytes = reqwest::get("https://secure.eicar.org/eicarcom2.zip")
            .await?
            .bytes()
            .await?;

        let mut clamd_client = ClamdClientBuilder::tcp_socket(TCP_ADDRESS)?.build();
        let err = clamd_client.scan_bytes(&eicar_bytes).await.unwrap_err();
        if let ClamdError::ScanError(s) = err {
            assert_eq!(s, "Win.Test.EICAR_HDB-1 FOUND");
        } else {
            panic!("Scan error expected");
        }
        Ok(())
    }

    #[tokio::test]
    #[traced_test]
    async fn tcp_reload() -> eyre::Result<()> {
        setup_clamav();
        let mut clamd_client = ClamdClientBuilder::tcp_socket(TCP_ADDRESS)?.build();
        clamd_client.reload().await?;
        Ok(())
    }

    #[tokio::test]
    #[traced_test]
    async fn unix_socket_common_operations() -> eyre::Result<()> {
        setup_clamav();
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
        setup_clamav();
        const NUM_BYTES: usize = 1024 * 1024;

        let random_bytes: Vec<u8> = (0..NUM_BYTES).map(|_| rand::random::<u8>()).collect();
        let mut clamd_client = ClamdClientBuilder::unix_socket(UNIX_SOCKET_PATH).build();

        clamd_client.scan_bytes(&random_bytes).await?;
        Ok(())
    }

    #[tokio::test]
    #[traced_test]
    async fn unix_socket_eicar() -> eyre::Result<()> {
        setup_clamav();
        let eicar_bytes = reqwest::get("https://secure.eicar.org/eicarcom2.zip")
            .await?
            .bytes()
            .await?;
        let mut clamd_client = ClamdClientBuilder::unix_socket(UNIX_SOCKET_PATH).build();

        let err = clamd_client.scan_bytes(&eicar_bytes).await.unwrap_err();
        if let ClamdError::ScanError(s) = err {
            assert_eq!(s, "Win.Test.EICAR_HDB-1 FOUND");
        } else {
            panic!("Scan error expected");
        }
        Ok(())
    }

    #[tokio::test]
    #[traced_test]
    async fn unix_socket_reload() -> eyre::Result<()> {
        setup_clamav();
        let mut clamd_client = ClamdClientBuilder::unix_socket(UNIX_SOCKET_PATH).build();

        clamd_client.reload().await?;
        Ok(())
    }

    #[tokio::test]
    #[traced_test]
    async fn keep_alive() -> eyre::Result<()> {
        setup_clamav();
        let eicar_bytes = reqwest::get("https://secure.eicar.org/eicarcom2.zip")
            .await?
            .bytes()
            .await?;

        let mut clamd_client = ClamdClientBuilder::tcp_socket(TCP_ADDRESS)?
            .keep_alive(true)
            .build();
        clamd_client.ping().await?;
        clamd_client.ping().await?;
        let stats = clamd_client.stats().await?;
        assert!(!stats.is_empty());
        let version = clamd_client.version().await?;
        assert!(!version.is_empty());
        let err = clamd_client.scan_bytes(&eicar_bytes).await.unwrap_err();
        if let ClamdError::ScanError(s) = err {
            assert_eq!(s, "stream: Win.Test.EICAR_HDB-1 FOUND");
        } else {
            panic!("Scan error expected");
        }
        clamd_client.end_session().await?;
        Ok(())
    }
}

#[cfg(doctest)]
mod test_readme {
    macro_rules! external_doc_test {
        ($x:expr) => {
            #[doc = $x]
            extern "C" {}
        };
    }

    external_doc_test!(include_str!("../README.md"));
}

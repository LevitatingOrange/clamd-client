use bytes::{Buf, BufMut, BytesMut};
use futures::SinkExt;
use futures::StreamExt;
use std::marker::PhantomData;
use std::net::SocketAddr;
use std::path::Path;
use std::path::PathBuf;
use std::pin::Pin;
use tokio::net::{TcpStream, UnixStream};
use tokio_util::codec::Decoder;
use tokio_util::codec::Encoder;
use tokio_util::codec::Framed;

use tokio::io::{AsyncRead, AsyncWrite};

use crate::error::Error;
use crate::error::Result;

pub mod error;

enum ClamdRequestMessage {
    Ping,
    Version,
    Reload,
    Shutdown,
}

enum ClamdResponseMessage {}

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
            ClamdRequestMessage::Version => todo!(),
            ClamdRequestMessage::Reload => todo!(),
            ClamdRequestMessage::Shutdown => todo!(),
        }
    }
}

impl Decoder for ClamdZeroDelimitedCodec {
    type Item = String;

    type Error = Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>> {
        if let Some(split_pos) = src[self.next_index..].iter().position(|&x| x == 0u8) {
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

enum ConnectionType {
    Oneshot,
    KeepAlive,
}

pub struct ClamdClient {
    //codec: Framed<T, ClamdZeroDelimitedCodec>,
    socket_type: SocketType,
    connection_type: ConnectionType,
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

pub struct ClamdClientBuilder {
    socket_type: SocketType,
    connection_type: ConnectionType,
}

impl ClamdClientBuilder {
    pub fn unix_socket(path: impl AsRef<Path>) -> Self {
        Self {
            socket_type: SocketType::Unix(path.as_ref().to_owned()),
            connection_type: ConnectionType::Oneshot,
        }
    }
    pub fn tcp_socket(addr: impl Into<SocketAddr>) -> Self {
        Self {
            socket_type: SocketType::Tcp(addr.into()),
            connection_type: ConnectionType::Oneshot,
        }
    }

    pub fn build(self) -> ClamdClient {
        ClamdClient {
            socket_type: self.socket_type,
            connection_type: self.connection_type,
        }
    }
}

impl ClamdClient {
    async fn connect(&mut self) -> Result<Framed<SocketWrapper, ClamdZeroDelimitedCodec>> {
        let codec = ClamdZeroDelimitedCodec::new();
        match &self.connection_type {
            Oneshot => match &self.socket_type {
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
            KeepAlive => todo!(),
        }
    }

    pub async fn ping(&mut self) -> Result<()> {
        let mut sock = self.connect().await?;
        sock.send(ClamdRequestMessage::Ping).await?;
        if let Some(s) = sock.next().await.transpose()? {
            if s == "PONG" {
                Ok(())
            } else {
                Err(Error::InvalidResponse(s))
            }
        } else {
            Err(Error::NoResponse)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_works() {
        // let result = add(2, 2);
        // assert_eq!(result, 4);
    }
}

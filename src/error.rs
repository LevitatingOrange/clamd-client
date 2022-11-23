use std::num::TryFromIntError;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not send chunk: too large: {0}")]
    ChunkSizeError(#[source] TryFromIntError),
    #[error("could not connect to socket: {0}")]
    ConnectError(#[source] std::io::Error),
    #[error("could not decode clamav response: {0}")]
    DecodingUtf8Error(#[source] std::string::FromUtf8Error),
    #[error("could not decode / encode clamav response: {0}")]
    DecodingIoError(
        #[from]
        #[source]
        std::io::Error,
    ),

    #[error("invalid response from clamd: {0}")]
    InvalidResponse(String),
    #[error("no response from clamd")]
    NoResponse,
    #[error("incomplete response from clamd: {0}")]
    IncompleteResponse(String),
    #[error("clamd returned error on scan, possible virus: {0}")]
    ScanError(String),
}

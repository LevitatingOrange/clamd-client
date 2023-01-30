// for nicer doc links
#[allow(unused_imports)]
use crate::ClamdClient;
use std::num::TryFromIntError;

use tracing::trace;

pub type Result<T> = std::result::Result<T, ClamdError>;

/// Errors that can occur when using [`ClamdClient`].
#[derive(Debug, thiserror::Error)]
pub enum ClamdError {
    /// Occurs when the socket address input could not be parsed.
    #[error("error parsing socket address: {0}")]
    AddrParsingError(#[source] std::io::Error),
    /// Occurs when the custom set chunk size is larger than
    /// [`std::u32::MAX`].
    #[error("could not send chunk: too large: {0}")]
    ChunkSizeError(#[source] TryFromIntError),
    /// Occurs when the library cannot connect to the tcp or unix
    /// socket. Contains underlying [`std::io::Error`].
    #[error("could not connect to socket: {0}")]
    ConnectError(#[source] std::io::Error),
    /// Occurs when the clamd response is not valid Utf8.
    #[error("could not decode clamav response: {0}")]
    DecodingUtf8Error(#[source] std::string::FromUtf8Error),
    /// Occurs when there was an [`std::io::Error`] while transfering
    /// commands or data to/from clamd.
    #[error("could not decode / encode clamav response: {0}")]
    DecodingIoError(
        #[from]
        #[source]
        std::io::Error,
    ),
    /// Occurs when the file path is not utf8.
    #[error("invalid path")]
    InvalidPath,
    /// Occurs when the response from clamd is not what the library
    /// expects. Contains the invalid response.
    #[error("invalid response from clamd: {0}")]
    InvalidResponse(String),
    /// Occurs when there should be a response from clamd but it just
    /// closed the connection without sending a response.
    #[error("no response from clamd")]
    NoResponse,
    /// Occurs when we expect a longer response from clamd, but it
    /// is somehow malformed. Contains the invalid response.
    #[error("incomplete response from clamd: {0}")]
    IncompleteResponse(String),
    #[error("unsupported feature: path must point to file")]
    UnsupportedFeature,
}

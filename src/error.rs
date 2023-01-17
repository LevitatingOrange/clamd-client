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
    /// Occurs when everything between this library and clamd went
    /// well but clamd seems to have found a virus signature. See also
    /// [`ClamdError::scan_error`].
    #[error("clamd returned error on scan, possible virus: {0}")]
    ScanError(String),
}

impl ClamdError {
    /// If you want to ignore any error but an actual malignent scan
    /// result from clamd. I do not recommend using this without careful thought, as any other error
    /// could hide that uploaded bytes are actually a virus.
    /// # Example
    /// ```rust
    /// # use std::net::SocketAddr;
    /// # use clamd_client::{ClamdClientBuilder, ScanResult};
    /// # use eyre::Result;
    /// # async fn doc() -> eyre::Result<()> {
    /// let address = "127.0.0.1:3310".parse::<SocketAddr>()?;
    /// let mut clamd_client = ClamdClientBuilder::tcp_socket(address)?.build();
    ///
    /// // This downloads a virus signature that is benign but trips clamd.
    /// let eicar_bytes = reqwest::get("https://secure.eicar.org/eicarcom2.zip")
    ///   .await?
    ///   .bytes()
    ///   .await?;
    ///
    /// let result = clamd_client.scan_bytes(&eicar_bytes).await?;
    /// match result {
    ///     ScanResult::Malignent { infection_types } => {
    ///         tracing::info!("clamd found a virus(es):\n{}", infection_types.join("\n"))
    ///     }
    ///     _ => (),
    /// };
    /// # Ok(())
    /// # }
    /// ```
    pub fn scan_error(self) -> Option<String> {
        match self {
            Self::ScanError(s) => Some(s),
            _ => {
                trace!("ignoring non-scan error {:?}", self);
                None
            }
        }
    }
}

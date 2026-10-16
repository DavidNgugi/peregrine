//! Why an indexer call failed. No variant carries a URL, a query, or a credential.

/// A failure that is the indexer's, the transport's, or the configuration's.
#[derive(Debug, thiserror::Error)]
pub enum IndexerError {
    /// The configured base URL is not an `http`/`https` endpoint.
    #[error("the indexer's base URL is not a valid http(s) endpoint")]
    InvalidBaseUrl,
    /// The request did not complete.
    #[error("the indexer could not be reached")]
    Transport,
    /// The indexer answered with a non-success status and said why.
    #[error("the indexer refused the request ({status}): {detail}")]
    Rejected {
        /// The HTTP status the indexer answered with.
        status: u16,
        /// The indexer's own validation messages, with anything secret removed.
        detail: String,
    },
    /// The indexer answered with a non-success status and said nothing useful.
    #[error("the indexer answered with status {0}")]
    Status(u16),
    /// The answer was not the documented shape.
    #[error("the indexer's answer did not have the documented shape")]
    SchemaDrift,
    /// The answer was larger than this build will read.
    #[error("the indexer's answer exceeded the size this build will read")]
    BodyTooLarge,
    /// No credential is configured, so no request was made.
    #[error("no indexer credential is configured")]
    Unconfigured,
}

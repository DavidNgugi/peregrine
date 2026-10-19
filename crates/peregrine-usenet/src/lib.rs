#![forbid(unsafe_code)]
#![allow(clippy::doc_markdown)] // The protocol vocabulary is capitalized in prose.
//! Usenet transfer: the NZB vocabulary, the NNTP transport, article assembly, and the
//! `UsenetEngine` port.
//!
//! The crate is a leaf: it depends on `peregrine-domain` for the safety report and the
//! protocol-neutral transfer types, and on runtime libraries for IO. It never names Tauri, the
//! database, or the keyring; the app resolves a provider and hands the engine a document.

pub mod engine;
pub mod nntp;
pub mod nzb;
pub mod yenc;

pub use engine::{
    AddNzbRequest, AddRefused, EngineError, EngineFuture, NntpEngine, ServerConfig, UsenetEngine,
    UsenetHandle, UsenetStats,
};
pub use nntp::{NntpConfig, NntpConnection, NntpError};
pub use nzb::{NzbDocument, NzbError, NzbFile, NzbSegment};
pub use yenc::{DecodedArticle, YencError, decode};

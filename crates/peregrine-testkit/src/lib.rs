#![forbid(unsafe_code)]
//! Test doubles for the ports, and the conformance suite they must pass.
//!
//! The crate holds a frozen [`FakeClock`], a deterministic [`CountingIds`], a scripted
//! [`FakeMetadataProvider`], and a stateful [`FakeTorrentEngine`], plus
//! [`clock_satisfies_the_port`], [`id_generator_satisfies_the_port`],
//! [`metadata_provider_satisfies_the_port`], and [`torrent_engine_satisfies_the_port`], which
//! assert the contract each port states. A suite is written once and run twice — against the
//! fake here, and against the real adapter in its own crate — so the fake and the adapter
//! cannot drift apart in silence.
//!
//! That is the whole reason for the crate: a fake every test crate shares is a fake that
//! cannot quietly disagree with the adapter it stands in for, and a test that injects one
//! never has to reach the wall clock or the network to exercise a rule (AGENTS R14).
//!
//! This crate is `unpublished` in `arch-allowlist.toml`, which is what lets [`FakeClock`]
//! advance: `cargo xtask check-determinism` skips a crate that never ships, so a clock
//! under a test's control is permitted here by design rather than by exemption. Nothing in
//! this crate reads the machine's clock — the point of a fake is that it does not.
//!
//! The fakes cover the five ports that exist: [`FakeClock`] and [`CountingIds`] for
//! `peregrine-domain`'s two, [`FakeMetadataProvider`] for the metadata provider port,
//! [`MemorySecretStore`] for secrets, and [`FakeTorrentEngine`] for the torrent engine port.
//! In-repertoire HTTP fakes and in-memory repositories are deliberately absent: the ports for
//! them have not been written, and a double for a trait that does not exist is speculative work
//! (AGENTS R3).

mod clock;
mod conformance;
mod ids;
mod metadata;
mod secrets;
mod torrent;

pub use clock::{DEFAULT_EPOCH_MILLISECONDS, FakeClock};
pub use conformance::{
    EngineProbe, clock_satisfies_the_port, id_generator_satisfies_the_port,
    metadata_provider_satisfies_the_port, torrent_engine_satisfies_the_port,
};
pub use ids::CountingIds;
pub use metadata::{FakeMetadataProvider, fake_descriptor};
pub use secrets::MemorySecretStore;
pub use torrent::{EngineCall, EngineOperation, FakeTorrentEngine};

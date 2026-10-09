#![forbid(unsafe_code)]
// xtask never ships and is not a library anyone depends on: its public surface
// exists so the fixture tests and the binary can share code. Demanding a rustdoc
// line on every gate-internal item would be documentation for its own sake.
#![allow(missing_docs)]
//! Repository gates that run in CI and locally.
//!
//! Each gate is a pure function over a repository root so it can be pointed at a
//! fixture tree in a test. The binary in `main.rs` is a thin argument parser over
//! these functions; there is no logic there.

pub mod arch;
pub mod backlog;
pub mod deaddeps;
pub mod migrations;
pub mod report;
pub mod todos;
pub mod workspace;

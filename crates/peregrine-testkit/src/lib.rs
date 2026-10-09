#![forbid(unsafe_code)]
//! Test doubles for the ports: fake clock, deterministic id generator, in-memory
//! repositories, and a scripted metadata provider.
//!
//! Built for determinism, so no test in this workspace has to reach the network or
//! the wall clock to exercise a rule.

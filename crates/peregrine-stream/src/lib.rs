#![forbid(unsafe_code)]
//! The loopback and LAN media servers: byte-range delivery, subtitle sidecars,
//! cast streams, token authentication, per-session lifetime, and bandwidth
//! accounting.
//!
//! Loopback and LAN listeners are separate bind addresses with separate token
//! scopes, and the LAN listener exists only while casting.

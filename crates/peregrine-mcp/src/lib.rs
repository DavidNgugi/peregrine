#![forbid(unsafe_code)]
//! The stdio MCP server that agent clients launch: JSON-RPC framing, protocol
//! version negotiation, tool schemas, and a bridge to the running app over a
//! per-user local socket.
//!
//! Contains no business logic and never opens the database. Post-1.0: see
//! docs/adr/0017.

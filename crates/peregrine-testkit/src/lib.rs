#![forbid(unsafe_code)]
//! Test doubles for the ports.
//!
//! **Nothing is here yet.** The crate is declared so the fakes have one agreed home, and
//! `PG-E4-19` owns filling it: a fake clock, a deterministic id generator, in-memory
//! repositories, and a scripted metadata provider. Until that lands, the doubles live beside
//! the tests that use them.
//!
//! The reason it exists at all is determinism: a fake in one shared crate is a fake every
//! crate can use, so no test anywhere has to reach the network or the wall clock to exercise
//! a rule. The doc above used to claim those doubles were already here, which sent a reader
//! looking for a `FrozenClock` that did not exist.

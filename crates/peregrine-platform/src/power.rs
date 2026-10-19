//! Holding the system awake while a device is being served (PG-E1-17).
//!
//! The mechanism is per-platform — an `IOPMAssertion` on macOS, `SetThreadExecutionState` on
//! Windows, a logind or `ScreenSaver` inhibit on Linux — and `keepawake` is the one binding that
//! covers all three. The assertion is created and dropped on its own thread: the binding's handle
//! is not guaranteed to be `Send` everywhere, and keeping both ends in one place is what makes the
//! guard's release actually release.
//!
//! A refusal to take the assertion is logged rather than returned. A machine that cannot be held
//! awake still serves while it is awake, which is better than a listener that refuses to start
//! because of a power setting.

use std::sync::mpsc;
use std::thread::JoinHandle;

use tracing::warn;

/// One held claim that keeps the system from sleeping on its own until it is dropped.
pub trait Inhibitor: Send {}

/// Takes and releases the system's sleep assertions.
pub trait PowerSave: Send + Sync {
    /// Holds one assertion for as long as the returned guard lives.
    fn inhibit(&self, reason: &str) -> Box<dyn Inhibitor>;
}

/// The production adapter.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemPower;

impl PowerSave for SystemPower {
    fn inhibit(&self, reason: &str) -> Box<dyn Inhibitor> {
        let (stop, wait) = mpsc::channel::<()>();
        let reason = reason.to_owned();
        let handle = std::thread::spawn(move || {
            match keepawake::Builder::default()
                .idle(true)
                .reason(reason)
                .app_name("Peregrine")
                .app_reverse_domain("app.peregrine.desktop")
                .create()
            {
                Ok(held) => {
                    let _ = wait.recv();
                    drop(held);
                }
                Err(error) => warn!(%error, "the system could not be kept awake"),
            }
        });
        Box::new(ThreadInhibitor {
            stop: Some(stop),
            handle: Some(handle),
        })
    }
}

/// A guard whose only job is to end the thread that holds the assertion.
struct ThreadInhibitor {
    stop: Option<mpsc::Sender<()>>,
    handle: Option<JoinHandle<()>>,
}

impl Inhibitor for ThreadInhibitor {}

impl Drop for ThreadInhibitor {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// A policy that holds nothing: for a host with no mechanism, and for every test that is not
/// about the assertion itself.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoPowerSave;

impl PowerSave for NoPowerSave {
    fn inhibit(&self, _reason: &str) -> Box<dyn Inhibitor> {
        struct Nothing;
        impl Inhibitor for Nothing {}
        Box::new(Nothing)
    }
}

#[cfg(test)]
mod tests {
    use super::{NoPowerSave, PowerSave};

    #[test]
    fn the_no_op_policy_hands_back_a_guard_that_can_be_dropped() {
        let held = NoPowerSave.inhibit("a test");
        drop(held);
    }
}

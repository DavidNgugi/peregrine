#![forbid(unsafe_code)]
//! Tauri commands, event emitters, DTO mapping, error mapping, and the specta
//! type export.
//!
//! Commands are twenty lines or fewer: parse, call one service, map the result.
//! There is no logic here.
//!
//! [`builder`] is the single registration point for the command surface. The
//! desktop shell passes it to Tauri, and `peregrine-ipc`'s `gen-ipc` binary
//! exports the TypeScript bindings from the same builder, so a command that is
//! invokable is also a command that is typed.

pub mod error;
pub mod system;

use tauri_specta::{Builder, collect_commands, collect_events};

/// The command surface, registered once.
///
/// This is the only place a command is registered; the desktop shell and the
/// binding generator both read it, so a command that is invokable is also
/// exported. Today the surface is the `system` namespace's `build_info`;
/// everything in docs/ARCHITECTURE.md §7 joins this list as it is built.
#[must_use]
pub fn builder() -> Builder<tauri::Wry> {
    Builder::<tauri::Wry>::new()
        // The command is generic over the runtime, so the macro cannot infer
        // which one this builder registers; the desktop shell uses `Wry`.
        .commands(collect_commands![system::build_info::<tauri::Wry>])
        .events(collect_events![])
}

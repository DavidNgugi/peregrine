#![forbid(unsafe_code)]
//! The `peregrine-desktop` binary: it starts the shell and reports a failed
//! launch on stderr with a non-zero exit code.

use std::process::ExitCode;

fn main() -> ExitCode {
    match peregrine_desktop::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            // No subscriber is installed for the process yet, so the user-facing
            // message has to reach stderr as well as the tracing event.
            tracing::error!(target: "peregrine_desktop", %error, "the desktop shell stopped");
            eprintln!("peregrine: {error}");
            ExitCode::FAILURE
        }
    }
}

#![forbid(unsafe_code)]
//! Exports the TypeScript bindings for the IPC command surface.
//!
//! `cargo xtask gen-ipc` runs this binary and owns the file at the repository
//! path, so the generator only ever writes where it is told. That keeps the
//! binding generator testable without a repository and gives the drift gate one
//! place to compare the committed file against a fresh export.
//!
//! Two things are written, in one file: what `specta` derives from the Rust types, and
//! the event-topic manifest, which is not a type it can see. Appending the second here
//! rather than hand-editing the committed file is what keeps the drift gate meaningful —
//! the committed file is exactly what this produces.

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("gen-ipc: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let mut out: Option<PathBuf> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--out" => {
                let value = args
                    .next()
                    .ok_or("--out needs a file path; run `cargo xtask gen-ipc`")?;
                out = Some(PathBuf::from(value));
            }
            other => return Err(format!("unrecognised argument `{other}`")),
        }
    }
    let out = out.ok_or("--out is required")?;

    // The target directory is created here because the writer does not create
    // intermediate directories and a first run has none.
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            format!(
                "cannot create the bindings directory {}: {error}",
                parent.display()
            )
        })?;
    }
    peregrine_ipc::builder()
        .export(specta_typescript::Typescript::default(), &out)
        .map_err(|error| format!("cannot export the TypeScript bindings: {error}"))?;

    let mut bindings = std::fs::read_to_string(&out)
        .map_err(|error| format!("cannot read back {}: {error}", out.display()))?;
    bindings.push_str(&peregrine_ipc::bindings::topics_block());
    std::fs::write(&out, bindings)
        .map_err(|error| format!("cannot write {}: {error}", out.display()))
}

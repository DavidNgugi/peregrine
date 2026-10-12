#![forbid(unsafe_code)]
//! The `cargo xtask` entry point: a thin argument parser over the gates in the
//! library. Anything that decides something lives in a module, so it can be
//! tested against a fixture tree.

use anyhow::{Context, Result, bail};
use std::path::PathBuf;
use std::process::ExitCode;
use xtask::{
    arch, backlog, commands, deaddeps, genipc, migrations, notices, offset, theme, todos, workspace,
};

fn main() -> ExitCode {
    match run() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) => {
            eprintln!("xtask: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<bool> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some((command, rest)) = args.split_first() else {
        print_help();
        return Ok(false);
    };

    let mut root_flag: Option<PathBuf> = None;
    let mut update = false;
    let mut check = false;
    let mut index = 0;
    while index < rest.len() {
        match rest[index].as_str() {
            "--root" => {
                let value = rest
                    .get(index + 1)
                    .with_context(|| "--root needs a directory".to_owned())?;
                root_flag = Some(PathBuf::from(value));
                index += 2;
            }
            "--update" => {
                update = true;
                index += 1;
            }
            "--check" => {
                check = true;
                index += 1;
            }
            other => bail!("unrecognised argument `{other}`; run `cargo xtask help`"),
        }
    }
    if update && check {
        bail!("--update and --check ask for opposite things; pass one");
    }

    let root = if let Some(root) = root_flag {
        root
    } else {
        let cwd = std::env::current_dir().context("cannot read the current directory")?;
        workspace::find_root(&cwd)?
    };

    match command.as_str() {
        "check-arch" => arch::run(&root),
        "check-migrations" => {
            if update {
                let added = migrations::update(&root)?;
                println!("check-migrations: recorded {added} new migration(s)");
                return Ok(true);
            }
            // A repository with no migrations yet is legitimate and clean. A
            // journal that points at a missing directory is still a violation,
            // caught below by the per-entry existence check.
            let (report, checked) = migrations::check(&root)?;
            Ok(xtask::report::finish("check-migrations", &report, checked))
        }
        "check-todos" => {
            let (report, checked) = todos::check(&root)?;
            Ok(xtask::report::finish("check-todos", &report, checked))
        }
        "check-commands" => {
            let (report, checked) = commands::check(&root)?;
            Ok(xtask::report::finish("check-commands", &report, checked))
        }
        "check-offset" => {
            let (report, checked) = offset::check(&root)?;
            Ok(xtask::report::finish("check-offset", &report, checked))
        }
        "backlog-stats" => backlog::run(&root),
        "check-dead-deps" => deaddeps::run(&root),
        "check-theme" => theme::run(&root),
        "check-notices" => {
            let mode = if update {
                notices::Mode::Update
            } else {
                notices::Mode::Check
            };
            notices::run(&root, mode)
        }
        "gen-ipc" => {
            let mode = if update {
                genipc::Mode::Update
            } else {
                genipc::Mode::Check
            };
            genipc::run(&root, mode)
        }
        "help" | "--help" | "-h" => {
            print_help();
            Ok(true)
        }
        other => bail!("unknown command `{other}`; run `cargo xtask help`"),
    }
}

fn print_help() {
    println!(
        "\
cargo xtask — repository gates

  check-arch                 dependency edges and pure-crate sources against arch-allowlist.toml
  check-migrations           migrations are append-only and journaled
  check-migrations --update  record new migrations (never rewrites an existing entry)
  check-todos                every deferral marker names a task or an issue
  check-commands             an IPC command parses, calls, and maps in twenty lines or fewer
  check-offset               no query pages with OFFSET; list reads are keyset-paginated
  check-dead-deps            no manifest declares a dependency its sources never use
  check-theme                every shipped theme clears the DESIGN_SYSTEM §2.2 contrast matrix
  check-notices              sidecar licences and source offers; THIRD_PARTY.md is current
  check-notices --update     rewrite THIRD_PARTY.md from sidecars.toml and Cargo.lock
  gen-ipc                    the committed TS bindings match a fresh export
  gen-ipc --check            the same, stated explicitly
  gen-ipc --update           rewrite the TS bindings from the Rust command surface
  backlog-stats              roll up docs/TASKS.md by phase and state

  --root <dir>               run against another tree, used by the fixture tests
  --update                   where a gate supports it, rewrite instead of check

Every gate exits non-zero on a violation and prints the path, the line where it
applies, and what to do about it."
    );
}

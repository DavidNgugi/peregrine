#![forbid(unsafe_code)]
//! The `cargo xtask` entry point: a thin argument parser over the gates in the
//! library. Anything that decides something lives in a module, so it can be
//! tested against a fixture tree.

use anyhow::{Context, Result, bail};
use std::path::PathBuf;
use std::process::ExitCode;
use xtask::{arch, backlog, deaddeps, migrations, todos, workspace};

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
            other => bail!("unrecognised argument `{other}`; run `cargo xtask help`"),
        }
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
        "backlog-stats" => backlog::run(&root),
        "check-dead-deps" => deaddeps::run(&root),
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
  check-dead-deps            no manifest declares a dependency its sources never use
  backlog-stats              roll up docs/TASKS.md by phase and state

  --root <dir>               run against another tree, used by the fixture tests

Every gate exits non-zero on a violation and prints the path, the line where it
applies, and what to do about it."
    );
}

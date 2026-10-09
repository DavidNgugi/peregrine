//! Build script.
//!
//! The `migrate!` macro expands to a static `Migrator` built from the files in
//! `migrations/`, but on stable Rust a procedural macro cannot tell cargo to watch a
//! directory. Without the line below, adding a migration that no `.rs` file mentions can
//! leave a stale migration set embedded in the binary.

fn main() {
    println!("cargo:rerun-if-changed=migrations");
}

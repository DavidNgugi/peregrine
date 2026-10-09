//! Build script: runs `tauri-build`, which embeds the frontend assets, processes
//! `capabilities/`, and generates the permission schemas under `gen/`.
fn main() {
    tauri_build::build();
}

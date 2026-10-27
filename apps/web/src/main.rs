//! `cargo run -p apicraft-web` (or RustRover's "Run apicraft-web"): builds the
//! browser app and serves it with the local agent at http://127.0.0.1:8787/.
//! The app itself is the library, compiled to WebAssembly; this native
//! launcher just runs `cargo xtask serve`, passing its arguments along
//! (e.g. `--release`, `--port 9000`).

#[cfg(not(target_arch = "wasm32"))]
fn main() -> std::process::ExitCode {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    let status = std::process::Command::new(cargo).current_dir(root).args(["run", "--package", "xtask", "--", "serve"]).args(std::env::args().skip(1)).status();
    match status {
        Ok(s) if s.success() => std::process::ExitCode::SUCCESS,
        Ok(_) => std::process::ExitCode::FAILURE,
        Err(e) => {
            eprintln!("could not run cargo xtask serve: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

// In the wasm build the library's `#[wasm_bindgen(start)]` is the entry point.
#[cfg(target_arch = "wasm32")]
fn main() {}

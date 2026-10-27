//! Project tasks, written in Rust so they work the same on Windows, macOS and
//! Linux (and as RustRover "before launch" steps). Run `cargo xtask help`.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

const WASM_BINDGEN_VERSION: &str = "0.2.129";
const APP_NAME: &str = "Fizl ApiCraft";
const BIN: &str = "fizl-apicraft";

const HELP: &str = "\
cargo xtask <task> [--release]

  desktop    Run the desktop app (same as `cargo run -p apicraft-desktop`)
  web        Build the browser app into dist/web (wasm-bindgen output + web/ shell)
  serve      Build the browser app, then serve it with the agent at http://127.0.0.1:8787/
  dist       Release builds of everything, packaged for this OS under dist/
  icons      Regenerate the app icons in apps/desktop/assets/ from apicraft_ui::brand
  ci         Formatting, clippy, tests and a wasm build: what CI runs

  --release  Optimised build (dist always is)";

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("workspace root").to_path_buf()
}

fn cargo() -> Command {
    let mut c = Command::new(env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    c.current_dir(root());
    c
}

fn run(mut cmd: Command) -> Result<(), String> {
    let shown = format!("{cmd:?}");
    eprintln!("$ {shown}");
    let status = cmd.status().map_err(|e| format!("could not start {shown}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{shown} failed ({status})"))
    }
}

fn have(tool: &str) -> bool {
    Command::new(tool).arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

fn profile(release: bool) -> &'static str {
    if release {
        "release"
    } else {
        "debug"
    }
}

fn with_profile(mut c: Command, release: bool) -> Command {
    if release {
        c.arg("--release");
    }
    c
}

fn copy_dir(from: &Path, to: &Path) -> Result<(), String> {
    fs::create_dir_all(to).map_err(|e| e.to_string())?;
    for entry in fs::read_dir(from).map_err(|e| format!("{}: {e}", from.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), &target).map_err(|e| format!("copy {}: {e}", entry.path().display()))?;
        }
    }
    Ok(())
}

/// Builds the wasm app and assembles dist/web.
fn web(release: bool) -> Result<PathBuf, String> {
    let installed = Command::new("rustup")
        .args(["target", "list", "--installed"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("wasm32-unknown-unknown"))
        .unwrap_or(false);
    if !installed {
        let mut c = Command::new("rustup");
        c.args(["target", "add", "wasm32-unknown-unknown"]);
        run(c)?;
    }
    let bindgen_ok =
        Command::new("wasm-bindgen").arg("--version").output().map(|o| String::from_utf8_lossy(&o.stdout).contains(WASM_BINDGEN_VERSION)).unwrap_or(false);
    if !bindgen_ok {
        let mut c = cargo();
        c.args(["install", "wasm-bindgen-cli", "--version", WASM_BINDGEN_VERSION, "--locked"]);
        run(c)?;
    }
    let mut c = cargo();
    c.args(["build", "-p", "apicraft-web", "--lib", "--target", "wasm32-unknown-unknown"]);
    run(with_profile(c, release))?;

    let out = root().join("dist").join("web");
    let _ = fs::remove_dir_all(&out);
    let wasm = root().join("target/wasm32-unknown-unknown").join(profile(release)).join("apicraft_web.wasm");
    let mut c = Command::new("wasm-bindgen");
    c.args(["--target", "web", "--no-typescript", "--out-dir"]).arg(out.join("pkg")).arg(&wasm);
    run(c)?;
    if release && have("wasm-opt") {
        let file = out.join("pkg/apicraft_web_bg.wasm");
        let mut c = Command::new("wasm-opt");
        c.args(["-Oz", "--enable-bulk-memory", "--enable-nontrapping-float-to-int", "-o"]).arg(&file).arg(&file);
        run(c)?;
    }
    copy_dir(&root().join("web"), &out)?;
    let size = fs::metadata(out.join("pkg/apicraft_web_bg.wasm")).map(|m| m.len() / 1024).unwrap_or(0);
    eprintln!("web app ready in {} ({size} KB of wasm)", out.display());
    Ok(out)
}

fn serve(release: bool, port: &str) -> Result<(), String> {
    let out = web(release)?;
    let mut c = cargo();
    c.args(["run", "-p", "apicraft-agent"]);
    let mut c = with_profile(c, release);
    c.args(["--", "serve", "--port", port, "--web"]).arg(out);
    run(c)
}

// ---------------------------------------------------------------- icons

fn png_bytes(size: u32) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, size, size);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().expect("png header");
        w.write_image_data(&apicraft_ui::brand::icon_rgba(size)).expect("png data");
    }
    out
}

/// A .ico holding PNG images (supported since Windows Vista).
fn ico_bytes(sizes: &[u32]) -> Vec<u8> {
    let images: Vec<(u32, Vec<u8>)> = sizes.iter().map(|&s| (s, png_bytes(s))).collect();
    let mut out = vec![0, 0, 1, 0];
    out.extend_from_slice(&(images.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * images.len() as u32;
    for (size, data) in &images {
        let dim = if *size >= 256 { 0 } else { *size as u8 };
        out.extend_from_slice(&[dim, dim, 0, 0]);
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&32u16.to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += data.len() as u32;
    }
    for (_, data) in images {
        out.extend_from_slice(&data);
    }
    out
}

fn icons() -> Result<(), String> {
    let dir = root().join("apps/desktop/assets");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    fs::write(dir.join(format!("{BIN}.ico")), ico_bytes(&[16, 24, 32, 48, 64, 128, 256])).map_err(|e| e.to_string())?;
    fs::write(dir.join(format!("{BIN}.png")), png_bytes(512)).map_err(|e| e.to_string())?;
    // macOS iconset (iconutil turns it into .icns when bundling).
    let set = dir.join(format!("{BIN}.iconset"));
    fs::create_dir_all(&set).map_err(|e| e.to_string())?;
    for s in [16, 32, 128, 256, 512] {
        fs::write(set.join(format!("icon_{s}x{s}.png")), png_bytes(s)).map_err(|e| e.to_string())?;
        fs::write(set.join(format!("icon_{s}x{s}@2x.png")), png_bytes(s * 2)).map_err(|e| e.to_string())?;
    }
    eprintln!("icons written to {}", dir.display());
    Ok(())
}

// ---------------------------------------------------------------- packaging

fn version() -> String {
    let toml = fs::read_to_string(root().join("Cargo.toml")).unwrap_or_default();
    toml.lines().find_map(|l| l.trim().strip_prefix("version = \"").and_then(|v| v.strip_suffix('"')).map(str::to_string)).unwrap_or_else(|| "0.0.0".into())
}

fn arch() -> &'static str {
    match env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => other,
    }
}

fn dist() -> Result<(), String> {
    let mut c = cargo();
    c.args(["build", "--release", "-p", "apicraft-desktop", "-p", "apicraft-agent"]);
    run(c)?;
    let web_dir = web(true)?;
    let rel = root().join("target/release");
    let exe = env::consts::EXE_SUFFIX;
    let ver = version();
    let dist = root().join("dist");

    if cfg!(target_os = "macos") {
        // Fizl ApiCraft.app with the agent and web app inside Resources.
        let app = dist.join(format!("{APP_NAME}.app"));
        let _ = fs::remove_dir_all(&app);
        let contents = app.join("Contents");
        fs::create_dir_all(contents.join("MacOS")).map_err(|e| e.to_string())?;
        fs::create_dir_all(contents.join("Resources")).map_err(|e| e.to_string())?;
        fs::copy(rel.join(BIN), contents.join("MacOS").join(BIN)).map_err(|e| e.to_string())?;
        fs::copy(rel.join("apicraft-agent"), contents.join("Resources/apicraft-agent")).map_err(|e| e.to_string())?;
        copy_dir(&web_dir, &contents.join("Resources/web"))?;
        let iconset = root().join(format!("apps/desktop/assets/{BIN}.iconset"));
        let mut c = Command::new("iconutil");
        c.args(["-c", "icns", "-o"]).arg(contents.join("Resources/AppIcon.icns")).arg(&iconset);
        if let Err(e) = run(c) {
            eprintln!("warning: no .icns ({e}); run `cargo xtask icons` first");
        }
        let plist = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>{APP_NAME}</string>
  <key>CFBundleDisplayName</key><string>{APP_NAME}</string>
  <key>CFBundleIdentifier</key><string>com.fizl.apicraft</string>
  <key>CFBundleVersion</key><string>{ver}</string>
  <key>CFBundleShortVersionString</key><string>{ver}</string>
  <key>CFBundleExecutable</key><string>{BIN}</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSHumanReadableCopyright</key><string>Copyright (C) Fizl Ltd &amp; Simon Soanes. Apache License 2.0</string>
</dict></plist>
"#
        );
        fs::write(contents.join("Info.plist"), plist).map_err(|e| e.to_string())?;
        for f in ["LICENSE", "NOTICE"] {
            fs::copy(root().join(f), contents.join("Resources").join(f)).map_err(|e| e.to_string())?;
        }
        let zip = dist.join(format!("{BIN}-{ver}-macos-{}.zip", arch()));
        let mut c = Command::new("ditto");
        c.args(["-c", "-k", "--keepParent"]).arg(&app).arg(&zip);
        run(c)?;
        eprintln!("packaged {}", zip.display());
        return Ok(());
    }

    let os = if cfg!(windows) { "windows" } else { "linux" };
    let name = format!("{BIN}-{ver}-{os}-{}", arch());
    let pkg = dist.join(&name);
    let _ = fs::remove_dir_all(&pkg);
    fs::create_dir_all(&pkg).map_err(|e| e.to_string())?;
    fs::copy(rel.join(format!("{BIN}{exe}")), pkg.join(format!("{BIN}{exe}"))).map_err(|e| e.to_string())?;
    fs::copy(rel.join(format!("apicraft-agent{exe}")), pkg.join(format!("apicraft-agent{exe}"))).map_err(|e| e.to_string())?;
    copy_dir(&web_dir, &pkg.join("web"))?;
    for f in ["README.md", "LICENSE", "NOTICE"] {
        fs::copy(root().join(f), pkg.join(f)).map_err(|e| e.to_string())?;
    }
    if os == "linux" {
        fs::copy(root().join(format!("apps/desktop/assets/{BIN}.png")), pkg.join(format!("{BIN}.png"))).map_err(|e| e.to_string())?;
        let desktop = format!("[Desktop Entry]\nType=Application\nName={APP_NAME}\nComment=API client for REST, GraphQL, OData, SOAP, gRPC and WebSocket\nExec={BIN}\nIcon={BIN}\nCategories=Development;Network;\nTerminal=false\n");
        fs::write(pkg.join(format!("{BIN}.desktop")), desktop).map_err(|e| e.to_string())?;
    }
    // bsdtar ships with Windows 10+, macOS and most Linux distributions.
    let archive = if os == "windows" { format!("{name}.zip") } else { format!("{name}.tar.gz") };
    let mut c = Command::new("tar");
    c.current_dir(&dist);
    if os == "windows" {
        c.args(["-a", "-c", "-f", &archive, &name]);
    } else {
        c.args(["-czf", &archive, &name]);
    }
    run(c)?;
    eprintln!("packaged {}", dist.join(archive).display());
    Ok(())
}

fn ci() -> Result<(), String> {
    let mut c = cargo();
    c.args(["fmt", "--all", "--check"]);
    run(c)?;
    let mut c = cargo();
    c.args(["clippy", "--workspace", "--all-targets", "--", "-D", "warnings"]);
    run(c)?;
    let mut c = cargo();
    c.args(["test", "--workspace"]);
    run(c)?;
    let mut c = cargo();
    c.args(["build", "-p", "apicraft-web", "--lib", "--target", "wasm32-unknown-unknown"]);
    run(c)
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let release = args.iter().any(|a| a == "--release");
    let port = args.iter().position(|a| a == "--port").and_then(|i| args.get(i + 1)).cloned().unwrap_or_else(|| "8787".into());
    let result = match args.first().map(String::as_str) {
        Some("desktop") => {
            let mut c = cargo();
            c.args(["run", "-p", "apicraft-desktop"]);
            run(with_profile(c, release))
        }
        Some("web") => web(release).map(|_| ()),
        Some("serve") => serve(release, &port),
        Some("dist") => dist(),
        Some("icons") => icons(),
        Some("ci") => ci(),
        _ => {
            println!("{HELP}");
            Ok(())
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

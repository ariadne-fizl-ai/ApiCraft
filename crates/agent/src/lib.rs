//! Native networking and storage shared by the desktop app and the
//! `apicraft-agent` command-line tool: an HTTP/1.1 client, native gRPC over
//! HTTP/2, the local web server/proxy for the browser build, and the default
//! catalog location.

pub mod connect;
pub mod folders;
pub mod mcp_client;
pub mod mcp_server;
pub mod server;
pub mod upstream;

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use apicraft_core::http::{HttpRequest, HttpResponse, LocalBoxFuture, Transport};

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Sends with the native stack (blocking calls inside the future), so it
/// suits a worker thread driven by a simple executor such as `pollster`.
pub struct NativeTransport;

impl Transport for NativeTransport {
    fn send(&self, req: HttpRequest) -> LocalBoxFuture<'_, Result<HttpResponse, String>> {
        let r = upstream::send(req);
        Box::pin(async move { r })
    }
    fn native_grpc(&self) -> bool {
        true
    }
    fn sleep(&self, ms: u32) -> LocalBoxFuture<'_, ()> {
        std::thread::sleep(std::time::Duration::from_millis(ms as u64));
        Box::pin(async {})
    }
    fn now_ms(&self) -> u64 {
        now_ms()
    }
    fn mcp_session(
        &self,
        launch: apicraft_core::mcp::McpLaunch,
        messages: Vec<serde_json::Value>,
    ) -> LocalBoxFuture<'_, Result<apicraft_core::mcp::McpExchange, String>> {
        let r = mcp_client::session(launch, messages);
        Box::pin(async move { r })
    }
}

/// Where the catalog lives by default, shared by the desktop app, the CLI
/// and the agent's browser mirror:
/// - Windows: `%APPDATA%\Fizl ApiCraft\catalog.json.gz`
/// - macOS: `~/Library/Application Support/Fizl ApiCraft/catalog.json.gz`
/// - Linux: `$XDG_DATA_HOME/fizl-apicraft/catalog.json.gz` (`~/.local/share/...`)
pub fn data_dir() -> PathBuf {
    let base = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
    if cfg!(any(target_os = "windows", target_os = "macos")) {
        base.join("Fizl ApiCraft")
    } else {
        base.join("fizl-apicraft")
    }
}

/// The folder used before the rename to ApiCraft.
fn legacy_data_dir() -> PathBuf {
    let base = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
    if cfg!(any(target_os = "windows", target_os = "macos")) {
        base.join("Fizl ApiPost")
    } else {
        base.join("fizl-apipost")
    }
}

/// The default catalog file. On first use after the rename, a catalog left in
/// the old ApiPost folder is copied across (the old file is kept).
pub fn default_catalog_path() -> PathBuf {
    let path = data_dir().join("catalog.json.gz");
    if !path.exists() {
        let old = legacy_data_dir().join("catalog.json.gz");
        if old.exists() && std::fs::create_dir_all(data_dir()).is_ok() {
            let _ = std::fs::copy(&old, &path);
        }
    }
    path
}

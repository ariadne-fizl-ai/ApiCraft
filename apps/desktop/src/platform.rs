//! The desktop's [`Platform`]: worker threads with the native HTTP/gRPC
//! stack, the catalog file in the OS data folder, native file dialogs,
//! tungstenite WebSockets and a loopback listener for OAuth sign-in.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use apicraft_agent::{default_catalog_path, server::write_atomic, NativeTransport};
use apicraft_core::model::Catalog;
use apicraft_core::store;
use apicraft_ui::{run_job, Inbox, Job, Level, Msg, Platform, Status, WsConn, WsEvent};

const DEFAULT_REDIRECT: &str = "http://127.0.0.1:7878/callback";

pub struct DesktopPlatform {
    catalog_path: PathBuf,
    /// Modification time of the catalog as we last read or wrote it, and when we last looked.
    known_mtime: Mutex<Option<std::time::SystemTime>>,
    last_poll: Mutex<Instant>,
    /// A problem found while loading, reported once the UI is up.
    startup_note: Mutex<Option<String>>,
}

impl DesktopPlatform {
    pub fn new() -> Self {
        DesktopPlatform {
            catalog_path: default_catalog_path(),
            known_mtime: Mutex::new(None),
            last_poll: Mutex::new(Instant::now()),
            startup_note: Mutex::new(None),
        }
    }
}

impl DesktopPlatform {
    fn remember_mtime(&self) {
        let m = std::fs::metadata(&self.catalog_path).and_then(|m| m.modified()).ok();
        if let Ok(mut k) = self.known_mtime.lock() {
            *k = m;
        }
    }
}

/// Segoe UI on Windows reads better than egui's bundled font at UI sizes.
pub fn system_ui_font() -> Option<Vec<u8>> {
    if cfg!(target_os = "windows") {
        let windir = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
        std::fs::read(PathBuf::from(windir).join("Fonts").join("segoeui.ttf")).ok()
    } else {
        None
    }
}

impl Platform for DesktopPlatform {
    fn start(&self, inbox: Inbox) {
        if let Some(note) = self.startup_note.lock().ok().and_then(|mut n| n.take()) {
            inbox.toast(note, true);
        }
    }

    fn status(&self) -> Status {
        Status {
            label: "Native".into(),
            detail: "Requests are sent directly by this app: no CORS limits, native gRPC over HTTP/2, and WebSocket handshake headers.".into(),
            level: Level::Good,
        }
    }

    fn now_ms(&self) -> u64 {
        apicraft_agent::now_ms()
    }

    fn spawn(&self, job: Job, inbox: Inbox) {
        std::thread::spawn(move || pollster::block_on(run_job(job, &NativeTransport, &inbox)));
    }

    fn load_catalog(&self) -> Option<Catalog> {
        let bytes = std::fs::read(&self.catalog_path).ok()?;
        self.remember_mtime();
        match store::decode(&bytes) {
            Ok(imported) => Some(imported.into_catalog()),
            Err(e) => {
                // Keep the unreadable file rather than overwrite it.
                let backup = self.catalog_path.with_extension(format!("unreadable-{}.gz", apicraft_agent::now_ms()));
                let _ = std::fs::rename(&self.catalog_path, &backup);
                if let Ok(mut n) = self.startup_note.lock() {
                    *n = Some(format!("The catalog file could not be read ({e}); it was kept as {}", backup.display()));
                }
                None
            }
        }
    }

    fn save_catalog(&self, catalog: &Catalog) -> Result<(), String> {
        write_atomic(&self.catalog_path, &store::to_gzip(catalog)).map_err(|e| format!("{}: {e}", self.catalog_path.display()))?;
        self.remember_mtime();
        Ok(())
    }

    fn poll_external_catalog(&self) -> Option<Catalog> {
        {
            let mut last = self.last_poll.lock().ok()?;
            if last.elapsed() < Duration::from_millis(1500) {
                return None;
            }
            *last = Instant::now();
        }
        let mtime = std::fs::metadata(&self.catalog_path).and_then(|m| m.modified()).ok()?;
        let known = *self.known_mtime.lock().ok()?;
        if known == Some(mtime) {
            return None;
        }
        let bytes = std::fs::read(&self.catalog_path).ok()?;
        let catalog = store::decode(&bytes).ok()?.into_catalog();
        *self.known_mtime.lock().ok()? = Some(mtime);
        Some(catalog)
    }

    fn storage_description(&self) -> String {
        format!("Saved automatically to {}", self.catalog_path.display())
    }

    fn save_file(&self, suggested_name: &str, bytes: Vec<u8>, inbox: Inbox) {
        let Some(path) = rfd::FileDialog::new().set_file_name(suggested_name).save_file() else { return };
        match std::fs::write(&path, bytes) {
            Ok(()) => inbox.toast(format!("Saved {}", path.display()), false),
            Err(e) => inbox.toast(format!("Could not save {}: {e}", path.display()), true),
        }
    }

    fn open_files(&self, inbox: Inbox) {
        let Some(paths) = rfd::FileDialog::new()
            .add_filter("Catalogs, Postman, OpenAPI, WSDL, GraphQL, OData", &["json", "gz", "yaml", "yml", "wsdl", "xml", "edmx"])
            .add_filter("All files", &["*"])
            .pick_files()
        else {
            return;
        };
        for p in paths {
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            match std::fs::read(&p) {
                Ok(bytes) => inbox.post(Msg::FileOpened { name, bytes }),
                Err(e) => inbox.toast(format!("Could not read {name}: {e}"), true),
            }
        }
    }

    fn ws_connect(&self, url: &str, headers: &[(String, String)], protocols: &[String], seq: u32, inbox: Inbox) -> Result<Box<dyn WsConn>, String> {
        use tungstenite::client::IntoClientRequest;
        use tungstenite::http::{HeaderName, HeaderValue};
        let mut req = url.into_client_request().map_err(|e| e.to_string())?;
        for (k, v) in headers {
            let lk = k.to_ascii_lowercase();
            if matches!(lk.as_str(), "host" | "connection" | "upgrade" | "content-length") || lk.starts_with("sec-websocket-") {
                continue;
            }
            let name = HeaderName::from_bytes(k.as_bytes()).map_err(|e| format!("header {k}: {e}"))?;
            let value = HeaderValue::from_str(v).map_err(|e| format!("header {k}: {e}"))?;
            req.headers_mut().append(name, value);
        }
        if !protocols.is_empty() {
            req.headers_mut().insert("Sec-WebSocket-Protocol", HeaderValue::from_str(&protocols.join(", ")).map_err(|e| e.to_string())?);
        }
        let (tx, rx) = mpsc::channel::<Outgoing>();
        std::thread::spawn(move || ws_thread(req, rx, seq, inbox));
        Ok(Box::new(NativeWs { tx }))
    }

    fn ws_sends_headers(&self) -> bool {
        true
    }

    fn default_redirect_uri(&self) -> String {
        DEFAULT_REDIRECT.into()
    }

    fn authorize(&self, url: &str, redirect_uri: &str, inbox: Inbox) -> Result<(), String> {
        let u = apicraft_core::url::Url::parse(redirect_uri)?;
        if u.scheme != "http" || !matches!(u.host.as_str(), "127.0.0.1" | "localhost" | "[::1]") {
            return Err("On the desktop, the redirect URI must be a loopback address such as http://127.0.0.1:7878/callback".into());
        }
        let addr = if u.host == "[::1]" { format!("[::1]:{}", u.port_or_default()) } else { format!("127.0.0.1:{}", u.port_or_default()) };
        let listener = TcpListener::bind(&addr).map_err(|e| format!("could not listen on {addr} for the sign-in redirect: {e}"))?;
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        webbrowser::open(url).map_err(|e| format!("could not open the browser: {e}"))?;
        let path = u.path.clone();
        std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(300);
            while Instant::now() < deadline {
                match listener.accept() {
                    Ok((stream, _)) => {
                        if let Some(query) = answer_redirect(stream, &path) {
                            inbox.post(Msg::AuthRedirect { query });
                            return;
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(150)),
                    Err(_) => break,
                }
            }
            inbox.toast("Sign-in timed out", true);
        });
        Ok(())
    }

    fn native_grpc(&self) -> bool {
        true
    }
}

/// Reads the browser's redirect, thanks it, and returns the query string.
fn answer_redirect(mut stream: TcpStream, expected_path: &str) -> Option<String> {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut buf = [0u8; 8192];
    let n = stream.read(&mut buf).ok()?;
    let head = String::from_utf8_lossy(&buf[..n]);
    let target = head.lines().next()?.split_whitespace().nth(1)?.to_string();
    let (path, query) = target.split_once('?').unwrap_or((&target, ""));
    if path != expected_path {
        let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        return None;
    }
    let body = "<!doctype html><meta charset=utf-8><title>Fizl ApiCraft</title><body style=\"font-family:system-ui;padding:3rem;text-align:center\"><h2>Signed in</h2><p>You can close this tab and return to Fizl ApiCraft.</p></body>";
    let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
    Some(query.to_string())
}

enum Outgoing {
    Text(String),
    Close,
}

struct NativeWs {
    tx: Sender<Outgoing>,
}

impl WsConn for NativeWs {
    fn send_text(&self, text: &str) -> Result<(), String> {
        self.tx.send(Outgoing::Text(text.to_string())).map_err(|_| "the connection is closed".to_string())
    }
    fn close(&self) {
        let _ = self.tx.send(Outgoing::Close);
    }
}

/// Connects IPv4 first (see `apicraft_agent::upstream`), with a timeout per address.
fn connect_tcp(host: &str, port: u16) -> Result<TcpStream, String> {
    let mut addrs: Vec<SocketAddr> = (host.trim_matches(['[', ']']), port).to_socket_addrs().map_err(|e| format!("resolve {host}: {e}"))?.collect();
    addrs.sort_by_key(|a| a.is_ipv6());
    let mut last = format!("no addresses for {host}");
    for a in addrs {
        match TcpStream::connect_timeout(&a, Duration::from_secs(10)) {
            Ok(s) => return Ok(s),
            Err(e) => last = format!("connect {a}: {e}"),
        }
    }
    Err(last)
}

fn ws_thread(req: tungstenite::handshake::client::Request, rx: mpsc::Receiver<Outgoing>, seq: u32, inbox: Inbox) {
    use tungstenite::stream::MaybeTlsStream;
    use tungstenite::Message;
    let post = |event| inbox.post(Msg::Ws { seq, event });
    let uri = req.uri().clone();
    let host = uri.host().unwrap_or("").to_string();
    let port = uri.port_u16().unwrap_or(if uri.scheme_str() == Some("wss") { 443 } else { 80 });
    let tcp = match connect_tcp(&host, port) {
        Ok(s) => s,
        Err(e) => {
            post(WsEvent::Error(e));
            post(WsEvent::Closed("Could not connect".into()));
            return;
        }
    };
    let _ = tcp.set_nodelay(true);
    let (mut sock, _resp) = match tungstenite::client_tls(req, tcp) {
        Ok(s) => s,
        Err(e) => {
            post(WsEvent::Error(format!("handshake failed: {e}")));
            post(WsEvent::Closed("Could not connect".into()));
            return;
        }
    };
    // Short read timeouts let one thread both read and send.
    let timeout = Some(Duration::from_millis(50));
    match sock.get_mut() {
        MaybeTlsStream::Plain(s) => {
            let _ = s.set_read_timeout(timeout);
        }
        MaybeTlsStream::Rustls(s) => {
            let _ = s.get_mut().set_read_timeout(timeout);
        }
        _ => {}
    }
    post(WsEvent::Open);
    loop {
        while let Ok(out) = rx.try_recv() {
            let r = match out {
                Outgoing::Text(t) => sock.send(Message::Text(t.into())),
                Outgoing::Close => sock.close(None),
            };
            if let Err(e) = r {
                post(WsEvent::Error(e.to_string()));
            }
        }
        match sock.read() {
            Ok(Message::Text(t)) => post(WsEvent::Text(t.to_string())),
            Ok(Message::Binary(b)) => post(WsEvent::Binary(b.to_vec())),
            Ok(Message::Close(frame)) => {
                let why = match frame {
                    Some(f) if f.reason.is_empty() => format!("Closed (code {})", u16::from(f.code)),
                    Some(f) => format!("Closed (code {}: {})", u16::from(f.code), f.reason),
                    None => "Closed".into(),
                };
                // Finish the closing handshake.
                let _ = sock.flush();
                post(WsEvent::Closed(why));
                return;
            }
            Ok(_) => {}
            Err(tungstenite::Error::Io(e)) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
            Err(tungstenite::Error::ConnectionClosed) | Err(tungstenite::Error::AlreadyClosed) => {
                post(WsEvent::Closed("Closed".into()));
                return;
            }
            Err(e) => {
                post(WsEvent::Error(e.to_string()));
                post(WsEvent::Closed("Connection lost".into()));
                return;
            }
        }
    }
}

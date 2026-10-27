//! Sending from the browser: straight `fetch` (subject to CORS), or through
//! the local agent's proxy, which has no such limits and speaks native gRPC.

use apicraft_core::http::{HttpRequest, HttpResponse, LocalBoxFuture, ProxyError, Transport};
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;
use web_sys::{Request, RequestInit, Response};

use crate::dom::{self, js_err};

/// Headers browsers refuse to let a page set.
const FORBIDDEN: [&str; 13] =
    ["host", "content-length", "connection", "keep-alive", "origin", "referer", "cookie", "cookie2", "te", "trailer", "transfer-encoding", "upgrade", "via"];

#[derive(Clone)]
pub struct WebTransport {
    /// Agent base URL, e.g. `http://127.0.0.1:8787`; `None` sends directly.
    pub agent: Option<String>,
    /// Set by the app so a Cancel button can abort direct fetches.
    pub abort: Option<web_sys::AbortSignal>,
}

impl Transport for WebTransport {
    fn send(&self, req: HttpRequest) -> LocalBoxFuture<'_, Result<HttpResponse, String>> {
        Box::pin(async move {
            match &self.agent {
                Some(base) => via_agent(base, req, self.abort.as_ref()).await,
                None => direct(req, self.abort.as_ref()).await,
            }
        })
    }
    fn native_grpc(&self) -> bool {
        self.agent.is_some()
    }
    fn honours_connect_options(&self) -> bool {
        self.agent.is_some()
    }
    fn sleep(&self, ms: u32) -> LocalBoxFuture<'_, ()> {
        Box::pin(dom::sleep_ms(ms))
    }
    fn now_ms(&self) -> u64 {
        dom::now_ms()
    }
    fn mcp_session(
        &self,
        launch: apicraft_core::mcp::McpLaunch,
        messages: Vec<serde_json::Value>,
    ) -> LocalBoxFuture<'_, Result<apicraft_core::mcp::McpExchange, String>> {
        Box::pin(async move {
            let Some(base) = &self.agent else {
                return Err("stdio and SSE MCP servers need the local agent in the browser (Streamable HTTP works without it)".to_string());
            };
            let body = serde_json::to_vec(&serde_json::json!({ "launch": launch, "messages": messages })).map_err(|e| e.to_string())?;
            let resp = agent_fetch(&format!("{base}/__apicraft/mcp-session"), "POST", Some(&body), "application/json", self.abort.as_ref()).await?;
            let bytes = response_bytes(&resp).await?;
            if resp.status() == 200 {
                serde_json::from_slice(&bytes).map_err(|e| format!("bad agent reply: {e}"))
            } else {
                match serde_json::from_slice::<ProxyError>(&bytes) {
                    Ok(e) => Err(e.error),
                    Err(_) => Err(format!("agent returned {}", resp.status())),
                }
            }
        })
    }
}

fn headers_of(resp: &Response) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if let Ok(Some(iter)) = js_sys::try_iter(resp.headers().as_ref()) {
        for entry in iter.flatten() {
            let pair: js_sys::Array = entry.unchecked_into();
            out.push((pair.get(0).as_string().unwrap_or_default(), pair.get(1).as_string().unwrap_or_default()));
        }
    }
    out
}

async fn direct(req: HttpRequest, abort: Option<&web_sys::AbortSignal>) -> Result<HttpResponse, String> {
    let init = RequestInit::new();
    init.set_method(&req.method);
    init.set_mode(web_sys::RequestMode::Cors);
    init.set_credentials(web_sys::RequestCredentials::Omit);
    init.set_redirect(web_sys::RequestRedirect::Follow);
    if let Some(sig) = abort {
        init.set_signal(Some(sig));
    }
    let headers = web_sys::Headers::new().map_err(|e| js_err(&e))?;
    for (k, v) in &req.headers {
        if FORBIDDEN.contains(&k.to_ascii_lowercase().as_str()) {
            continue;
        }
        headers.append(k, v).map_err(|e| format!("header {k}: {}", js_err(&e)))?;
    }
    init.set_headers(&headers);
    if !req.body.is_empty() && req.method != "GET" && req.method != "HEAD" {
        let arr = js_sys::Uint8Array::from(&req.body[..]);
        init.set_body(&arr);
    }
    let request = Request::new_with_str_and_init(&req.url, &init).map_err(|e| js_err(&e))?;
    let started = dom::perf_now();
    let resp_val = JsFuture::from(dom::window().fetch_with_request(&request)).await.map_err(|e| {
        let msg = js_err(&e);
        if msg.contains("abort") {
            "request cancelled".to_string()
        } else {
            format!(
                "network error: {msg}. The server may be unreachable, or it does not allow browser requests from this page (CORS). Start the local agent (apicraft-agent) and its proxy is used automatically."
            )
        }
    })?;
    let resp: Response = resp_val.dyn_into().map_err(|_| "fetch did not return a Response")?;
    let headers = headers_of(&resp);
    let buf = JsFuture::from(resp.array_buffer().map_err(|e| js_err(&e))?).await.map_err(|e| js_err(&e))?;
    let body = js_sys::Uint8Array::new(&buf).to_vec();
    Ok(HttpResponse { status: resp.status(), status_text: resp.status_text(), headers, body, trailers: Vec::new(), elapsed_ms: dom::perf_now() - started })
}

async fn agent_fetch(url: &str, method: &str, body: Option<&[u8]>, content_type: &str, abort: Option<&web_sys::AbortSignal>) -> Result<Response, String> {
    let init = RequestInit::new();
    init.set_method(method);
    let headers = web_sys::Headers::new().map_err(|e| js_err(&e))?;
    let _ = headers.append("X-ApiCraft-Agent", "1");
    let _ = headers.append("Content-Type", content_type);
    init.set_headers(&headers);
    if let Some(sig) = abort {
        init.set_signal(Some(sig));
    }
    if let Some(b) = body {
        let arr = js_sys::Uint8Array::from(b);
        init.set_body(&arr);
    }
    let request = Request::new_with_str_and_init(url, &init).map_err(|e| js_err(&e))?;
    let v = JsFuture::from(dom::window().fetch_with_request(&request)).await.map_err(|e| {
        let msg = js_err(&e);
        if msg.contains("abort") {
            "request cancelled".to_string()
        } else {
            format!("could not reach the local agent at {url}: {msg}")
        }
    })?;
    v.dyn_into().map_err(|_| "fetch did not return a Response".to_string())
}

async fn response_bytes(resp: &Response) -> Result<Vec<u8>, String> {
    let buf = JsFuture::from(resp.array_buffer().map_err(|e| js_err(&e))?).await.map_err(|e| js_err(&e))?;
    Ok(js_sys::Uint8Array::new(&buf).to_vec())
}

async fn via_agent(base: &str, req: HttpRequest, abort: Option<&web_sys::AbortSignal>) -> Result<HttpResponse, String> {
    let body = serde_json::to_vec(&req).map_err(|e| e.to_string())?;
    let resp = agent_fetch(&format!("{base}/__apicraft/proxy"), "POST", Some(&body), "application/json", abort).await?;
    let bytes = response_bytes(&resp).await?;
    if resp.status() == 200 {
        serde_json::from_slice::<HttpResponse>(&bytes).map_err(|e| format!("bad agent reply: {e}"))
    } else {
        match serde_json::from_slice::<ProxyError>(&bytes) {
            Ok(e) => Err(e.error),
            Err(_) => Err(format!("agent returned {}: {}", resp.status(), String::from_utf8_lossy(&bytes))),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AgentInfo {
    pub base: String,
    pub version: String,
    pub catalog_file: String,
}

/// Looks for the agent: first on this page's own origin (when it served the
/// page), then on the default port.
pub async fn detect_agent(extra: Option<String>) -> Option<AgentInfo> {
    let origin = dom::location_origin();
    let mut candidates = vec![];
    if let Some(e) = extra.filter(|e| !e.is_empty()) {
        candidates.push(e);
    }
    if origin.starts_with("http") {
        candidates.push(origin.clone());
    }
    for d in ["http://127.0.0.1:8787", "http://localhost:8787"] {
        if !candidates.iter().any(|c| c == d) {
            candidates.push(d.to_string());
        }
    }
    for base in candidates {
        let base = base.trim_end_matches('/').to_string();
        let Ok(resp) = agent_fetch(&format!("{base}/__apicraft/ping"), "GET", None, "application/json", None).await else { continue };
        if !resp.ok() {
            continue;
        }
        let Ok(bytes) = response_bytes(&resp).await else { continue };
        let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes) else { continue };
        if v.get("agent").and_then(|a| a.as_str()) == Some("fizl-apicraft-agent") {
            return Some(AgentInfo {
                base,
                version: v.get("version").and_then(|x| x.as_str()).unwrap_or("").into(),
                catalog_file: v.get("catalogFile").and_then(|x| x.as_str()).unwrap_or("").into(),
            });
        }
    }
    None
}

pub async fn agent_load_catalog(base: &str) -> Result<Option<Vec<u8>>, String> {
    let resp = agent_fetch(&format!("{base}/__apicraft/catalog"), "GET", None, "application/json", None).await?;
    if resp.status() == 404 {
        return Ok(None);
    }
    if !resp.ok() {
        return Err(format!("agent returned {}", resp.status()));
    }
    Ok(Some(response_bytes(&resp).await?))
}

/// Reads a collection folder through the agent (or, with `link`, checks
/// one can hold a collection).
pub async fn agent_folder(base: &str, path: &str, link: bool) -> Result<apicraft_ui::FolderContents, String> {
    let body = serde_json::json!({ "path": path, "link": link }).to_string();
    let resp = agent_fetch(&format!("{base}/__apicraft/folder"), "POST", Some(body.as_bytes()), "application/json", None).await?;
    let bytes = response_bytes(&resp).await?;
    if !resp.ok() {
        return Err(String::from_utf8_lossy(&bytes).into_owned());
    }
    let v: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| format!("bad agent reply: {e}"))?;
    Ok(apicraft_ui::FolderContents {
        collection: serde_json::from_value(v["collection"].clone()).map_err(|e| format!("bad agent reply: {e}"))?,
        workflows: serde_json::from_value(v["workflows"].clone()).unwrap_or_default(),
        warnings: serde_json::from_value(v["warnings"].clone()).unwrap_or_default(),
    })
}

pub async fn agent_save_catalog(base: &str, gz: &[u8]) -> Result<(), String> {
    let resp = agent_fetch(&format!("{base}/__apicraft/catalog"), "PUT", Some(gz), "application/gzip", None).await?;
    if resp.ok() {
        Ok(())
    } else {
        Err(format!("agent could not save the catalog ({})", resp.status()))
    }
}

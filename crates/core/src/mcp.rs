//! Model Context Protocol client: one short session per request
//! (initialize, `notifications/initialized`, then the operation).
//!
//! Streamable HTTP runs on the ordinary [`Transport`], so it works anywhere,
//! including the browser. stdio and the legacy SSE transport need a process
//! or a long-lived stream; the host provides them through
//! [`Transport::mcp_session`] (the desktop app and the agent do).

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::http::{HttpRequest, Transport};
use crate::model::{McpOperation, McpOptions, McpTransport};

pub const PROTOCOL_VERSION: &str = "2025-06-18";
pub const CLIENT_NAME: &str = "fizl-apicraft";

/// How to reach a server that isn't plain Streamable HTTP.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum McpLaunch {
    Stdio { command: String, args: Vec<String>, env: Vec<(String, String)> },
    Sse { url: String, headers: Vec<(String, String)> },
}

/// The messages of one session and everything that came back.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct McpExchange {
    /// Responses to the requests, in order.
    pub responses: Vec<Value>,
    /// Server-to-client notifications and requests seen along the way (logs, progress, ...).
    pub notifications: Vec<Value>,
    /// stdio: what the process wrote to stderr.
    #[serde(default)]
    pub stderr: String,
    /// Transport details worth showing (session id, endpoint, ...).
    #[serde(default)]
    pub info: Vec<(String, String)>,
}

pub fn request(id: u64, method: &str, params: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
}

pub fn notification(method: &str) -> Value {
    json!({ "jsonrpc": "2.0", "method": method })
}

pub fn initialize_request(id: u64) -> Value {
    request(
        id,
        "initialize",
        json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": { "name": CLIENT_NAME, "version": env!("CARGO_PKG_VERSION") }
        }),
    )
}

/// The JSON-RPC request for the configured operation. `body` is the request
/// body: tool arguments, prompt arguments, or `{method, params}` for raw calls.
pub fn operation_request(id: u64, opts: &McpOptions, body: &str) -> Result<Value, String> {
    let parsed = || -> Result<Value, String> {
        if body.trim().is_empty() {
            Ok(json!({}))
        } else {
            serde_json::from_str(body).map_err(|e| format!("the body must be JSON: {e}"))
        }
    };
    let target = opts.target.trim();
    Ok(match opts.operation {
        McpOperation::Initialize => return Err("initialize is the handshake itself".into()),
        McpOperation::CallTool => {
            if target.is_empty() {
                return Err("choose a tool to call".into());
            }
            request(id, "tools/call", json!({ "name": target, "arguments": parsed()? }))
        }
        McpOperation::ReadResource => {
            if target.is_empty() {
                return Err("enter the resource URI to read".into());
            }
            request(id, "resources/read", json!({ "uri": target }))
        }
        McpOperation::GetPrompt => {
            if target.is_empty() {
                return Err("choose a prompt".into());
            }
            request(id, "prompts/get", json!({ "name": target, "arguments": parsed()? }))
        }
        McpOperation::Raw => {
            let v = parsed()?;
            let method = v.get("method").and_then(|m| m.as_str()).ok_or("a raw call's body needs a \"method\"")?;
            request(id, method, v.get("params").cloned().unwrap_or(json!({})))
        }
        op => request(id, op.method(), json!({})),
    })
}

/// The messages of a session: handshake, then the operation (if any).
pub fn session_messages(opts: &McpOptions, body: &str) -> Result<Vec<Value>, String> {
    let mut msgs = vec![initialize_request(1), notification("notifications/initialized")];
    if opts.operation != McpOperation::Initialize {
        msgs.push(operation_request(2, opts, body)?);
    }
    Ok(msgs)
}

/// JSON-RPC messages from an HTTP response body: plain JSON (one message or
/// a batch) or a `text/event-stream` of `data:` lines.
pub fn parse_messages(content_type: &str, body: &[u8]) -> Vec<Value> {
    let text = String::from_utf8_lossy(body);
    if content_type.contains("event-stream") {
        return parse_sse(&text).into_iter().filter_map(|(_, data)| serde_json::from_str(&data).ok()).collect();
    }
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Array(a)) => a,
        Ok(v) => vec![v],
        Err(_) => Vec::new(),
    }
}

/// Server-sent events as (event name, data) pairs.
pub fn parse_sse(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut event = String::new();
    let mut data: Vec<String> = Vec::new();
    for line in text.lines().chain(std::iter::once("")) {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            if !data.is_empty() {
                out.push((if event.is_empty() { "message".to_string() } else { event.clone() }, data.join("\n")));
            }
            event.clear();
            data.clear();
        } else if let Some(v) = line.strip_prefix("data:") {
            data.push(v.strip_prefix(' ').unwrap_or(v).to_string());
        } else if let Some(v) = line.strip_prefix("event:") {
            event = v.trim().to_string();
        }
    }
    out
}

/// Runs the session over Streamable HTTP: each message is a POST, carrying
/// the session id and protocol version the server handed out.
pub async fn http_session(url: &str, headers: &[(String, String)], messages: Vec<Value>, transport: &dyn Transport) -> Result<McpExchange, String> {
    let mut ex = McpExchange::default();
    let mut session: Option<String> = None;
    let mut version = PROTOCOL_VERSION.to_string();
    for msg in messages {
        let is_request = msg.get("id").is_some();
        let mut h: Vec<(String, String)> = headers.to_vec();
        h.retain(|(k, _)| !k.eq_ignore_ascii_case("content-type") && !k.eq_ignore_ascii_case("accept"));
        h.push(("Content-Type".into(), "application/json".into()));
        h.push(("Accept".into(), "application/json, text/event-stream".into()));
        if msg.get("method").and_then(|m| m.as_str()) != Some("initialize") {
            h.push(("MCP-Protocol-Version".into(), version.clone()));
        }
        if let Some(s) = &session {
            h.push(("Mcp-Session-Id".into(), s.clone()));
        }
        let req = HttpRequest {
            method: "POST".into(),
            url: url.to_string(),
            headers: h,
            body: serde_json::to_vec(&msg).unwrap_or_default(),
            grpc: false,
            timeout_ms: 60_000,
        };
        let resp = transport.send(req).await?;
        if let Some(s) = resp.header("mcp-session-id") {
            if session.is_none() {
                ex.info.push(("Mcp-Session-Id".into(), s.to_string()));
            }
            session = Some(s.to_string());
        }
        if !(200..300).contains(&resp.status) {
            let detail = String::from_utf8_lossy(&resp.body).chars().take(300).collect::<String>();
            return Err(format!(
                "server answered {} {} to {}: {detail}",
                resp.status,
                resp.status_text,
                msg.get("method").and_then(|m| m.as_str()).unwrap_or("?")
            ));
        }
        if !is_request {
            continue; // notifications get 202 Accepted
        }
        let id = msg.get("id").cloned();
        let mut answer = None;
        for m in parse_messages(resp.content_type(), &resp.body) {
            if m.get("id") == id.as_ref() && (m.get("result").is_some() || m.get("error").is_some()) {
                answer = Some(m);
            } else {
                ex.notifications.push(m);
            }
        }
        let answer = answer.ok_or_else(|| {
            format!("no response to {} (status {}, {})", msg.get("method").and_then(|m| m.as_str()).unwrap_or("?"), resp.status, resp.content_type())
        })?;
        if let Some(v) = answer.pointer("/result/protocolVersion").and_then(|v| v.as_str()) {
            version = v.to_string();
        }
        ex.responses.push(answer);
    }
    Ok(ex)
}

/// Splits a command line on spaces, honouring "double" and 'single' quotes.
/// Backslashes are kept as they are, so Windows paths work.
pub fn split_command_line(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut started = false;
    for c in line.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => cur.push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                started = true;
            }
            (None, c) if c.is_whitespace() => {
                if started || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                    started = false;
                }
            }
            (None, c) => cur.push(c),
        }
    }
    if started || !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// The program and its arguments: the separate argument list when there is
/// one, otherwise the command line split up.
pub fn command_and_args(opts: &McpOptions) -> Result<(String, Vec<String>), String> {
    if !opts.args.is_empty() {
        return Ok((opts.command.trim().to_string(), opts.args.clone()));
    }
    let mut parts = split_command_line(&opts.command);
    if parts.is_empty() {
        return Err("enter the command that starts the MCP server".into());
    }
    let program = parts.remove(0);
    Ok((program, parts))
}

/// Runs a whole session for a request's MCP settings.
pub async fn run(
    opts: &McpOptions,
    url: &str,
    headers: &[(String, String)],
    body: &str,
    transport: &dyn Transport,
) -> Result<(Vec<Value>, McpExchange), String> {
    let messages = session_messages(opts, body)?;
    let ex = match opts.transport {
        McpTransport::Http => http_session(url, headers, messages.clone(), transport).await?,
        McpTransport::Sse => transport.mcp_session(McpLaunch::Sse { url: url.to_string(), headers: headers.to_vec() }, messages.clone()).await?,
        McpTransport::Stdio => {
            let (command, args) = command_and_args(opts)?;
            let env = opts.env.iter().filter(|kv| kv.enabled && !kv.key.is_empty()).map(|kv| (kv.key.clone(), kv.value.clone())).collect();
            transport.mcp_session(McpLaunch::Stdio { command, args, env }, messages.clone()).await?
        }
    };
    Ok((messages, ex))
}

/// The result to show for an exchange: the operation's result (or the
/// handshake's for Initialize), and an error message if it failed.
pub fn outcome_result(opts: &McpOptions, ex: &McpExchange) -> (Value, Option<String>) {
    let last = if opts.operation == McpOperation::Initialize { ex.responses.first() } else { ex.responses.get(1).or(ex.responses.first()) };
    let Some(resp) = last else { return (Value::Null, Some("the server sent no response".into())) };
    if let Some(err) = resp.get("error") {
        let msg = format!(
            "JSON-RPC error {}: {}",
            err.get("code").map(|c| c.to_string()).unwrap_or_default(),
            err.get("message").and_then(|m| m.as_str()).unwrap_or("")
        );
        return (resp.clone(), Some(msg));
    }
    let result = resp.get("result").cloned().unwrap_or(Value::Null);
    let tool_error = (opts.operation == McpOperation::CallTool && result.get("isError").and_then(|b| b.as_bool()).unwrap_or(false)).then(|| {
        let text = result.pointer("/content/0/text").and_then(|t| t.as_str()).unwrap_or("the tool reported an error");
        format!("tool error: {text}")
    });
    (result, tool_error)
}

// ---------------------------------------------------------------- discovery

#[derive(Debug, Clone, Default, PartialEq)]
pub struct McpCatalog {
    pub server: String,
    pub tools: Vec<(String, String, Value)>,
    pub resources: Vec<(String, String)>,
    pub prompts: Vec<(String, String, Vec<String>)>,
}

/// Messages that list everything a server offers.
pub fn discovery_messages() -> Vec<Value> {
    vec![
        initialize_request(1),
        notification("notifications/initialized"),
        request(2, "tools/list", json!({})),
        request(3, "resources/list", json!({})),
        request(4, "prompts/list", json!({})),
    ]
}

pub fn parse_catalog(ex: &McpExchange) -> McpCatalog {
    let mut c = McpCatalog::default();
    for r in &ex.responses {
        let Some(result) = r.get("result") else { continue };
        if let Some(info) = result.get("serverInfo") {
            c.server = format!("{} {}", info.get("name").and_then(|v| v.as_str()).unwrap_or(""), info.get("version").and_then(|v| v.as_str()).unwrap_or(""))
                .trim()
                .to_string();
        }
        for t in result.get("tools").and_then(|v| v.as_array()).into_iter().flatten() {
            let name = t.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let desc = t.get("description").and_then(|v| v.as_str()).unwrap_or("").to_string();
            c.tools.push((name, desc, t.get("inputSchema").cloned().unwrap_or(json!({}))));
        }
        for r in result.get("resources").and_then(|v| v.as_array()).into_iter().flatten() {
            c.resources
                .push((r.get("uri").and_then(|v| v.as_str()).unwrap_or("").to_string(), r.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string()));
        }
        for p in result.get("prompts").and_then(|v| v.as_array()).into_iter().flatten() {
            let args = p
                .get("arguments")
                .and_then(|a| a.as_array())
                .map(|a| a.iter().filter_map(|x| x.get("name").and_then(|n| n.as_str()).map(str::to_string)).collect())
                .unwrap_or_default();
            c.prompts.push((
                p.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                p.get("description").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                args,
            ));
        }
    }
    c
}

/// Discovery over HTTP, or through the host for stdio/SSE. Servers that
/// don't offer resources or prompts answer those lists with errors; that's fine.
pub async fn discover(opts: &McpOptions, url: &str, headers: &[(String, String)], transport: &dyn Transport) -> Result<McpCatalog, String> {
    let messages = discovery_messages();
    let ex = match opts.transport {
        McpTransport::Http => http_session_lenient(url, headers, messages, transport).await?,
        McpTransport::Sse => transport.mcp_session(McpLaunch::Sse { url: url.to_string(), headers: headers.to_vec() }, messages).await?,
        McpTransport::Stdio => {
            let (command, args) = command_and_args(opts)?;
            let env = opts.env.iter().filter(|kv| kv.enabled && !kv.key.is_empty()).map(|kv| (kv.key.clone(), kv.value.clone())).collect();
            transport.mcp_session(McpLaunch::Stdio { command, args, env }, messages).await?
        }
    };
    Ok(parse_catalog(&ex))
}

/// Like [`http_session`], but carries on past failed list calls.
async fn http_session_lenient(url: &str, headers: &[(String, String)], messages: Vec<Value>, transport: &dyn Transport) -> Result<McpExchange, String> {
    let (head, rest) = messages.split_at(2);
    let mut ex = http_session(url, headers, head.to_vec(), transport).await?;
    let session: Vec<(String, String)> = ex.info.iter().filter(|(k, _)| k == "Mcp-Session-Id").cloned().collect();
    let version = ex.responses.first().and_then(|r| r.pointer("/result/protocolVersion")).and_then(|v| v.as_str()).unwrap_or(PROTOCOL_VERSION).to_string();
    for msg in rest {
        let mut h = headers.to_vec();
        h.extend(session.iter().cloned());
        h.push(("MCP-Protocol-Version".into(), version.clone()));
        // Each list call on its own, skipping the handshake (already done).
        match http_session(url, &h, vec![msg.clone()], transport).await {
            Ok(more) => {
                ex.responses.extend(more.responses);
                ex.notifications.extend(more.notifications);
            }
            Err(_) => continue,
        }
    }
    Ok(ex)
}

/// An example value for a JSON Schema (tool `inputSchema`s).
pub fn schema_example(schema: &Value) -> Value {
    crate::openapi::example_for_schema(schema)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::tests::{reply, Scripted};
    use crate::model::McpOptions;

    #[test]
    fn builds_operation_messages() {
        let opts = McpOptions { operation: McpOperation::CallTool, target: "add".into(), ..Default::default() };
        let msgs = session_messages(&opts, r#"{"a":1,"b":2}"#).unwrap();
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[0]["method"], "initialize");
        assert!(msgs[1].get("id").is_none());
        assert_eq!(msgs[2]["params"]["name"], "add");
        assert_eq!(msgs[2]["params"]["arguments"]["b"], 2);
        let raw = McpOptions { operation: McpOperation::Raw, ..Default::default() };
        let m = operation_request(9, &raw, r#"{"method":"logging/setLevel","params":{"level":"debug"}}"#).unwrap();
        assert_eq!(m["method"], "logging/setLevel");
    }

    #[test]
    fn splits_command_lines() {
        assert_eq!(split_command_line(r#"npx -y "@scope/server name" C:\tools\x.exe ''"#), vec!["npx", "-y", "@scope/server name", "C:\\tools\\x.exe", ""]);
        let opts = McpOptions { command: "uvx  mcp-server-fetch".into(), ..Default::default() };
        assert_eq!(command_and_args(&opts).unwrap(), ("uvx".into(), vec!["mcp-server-fetch".to_string()]));
    }

    #[test]
    fn parses_sse_bodies() {
        let body = b"event: message\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/message\"}\n\nevent: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":2,\n\ndata: \"result\":{}}\n\n";
        let msgs = parse_messages("text/event-stream", body);
        assert_eq!(msgs.len(), 1, "the split second event isn't valid JSON on its own");
        assert_eq!(parse_sse("event: endpoint\ndata: /messages?session=1\n\n"), vec![("endpoint".into(), "/messages?session=1".into())]);
    }

    #[test]
    fn http_session_carries_session_id() {
        let init = reply(
            200,
            &[("content-type", "application/json"), ("mcp-session-id", "abc")],
            r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-03-26","serverInfo":{"name":"demo","version":"1"},"capabilities":{}}}"#,
        );
        let ack = reply(202, &[], "");
        let call = reply(
            200,
            &[("content-type", "text/event-stream")],
            "data: {\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"3\"}]}}\n\n",
        );
        let t = Scripted::new(vec![init, ack, call]);
        let opts = McpOptions { operation: McpOperation::CallTool, target: "add".into(), ..Default::default() };
        let (_, ex) = pollster::block_on(run(&opts, "http://x/mcp", &[], r#"{"a":1,"b":2}"#, &t)).unwrap();
        let sent = t.sent.borrow();
        assert!(sent[1].headers.iter().any(|(k, v)| k == "Mcp-Session-Id" && v == "abc"));
        assert!(sent[2].headers.iter().any(|(k, v)| k == "MCP-Protocol-Version" && v == "2025-03-26"));
        let (result, err) = outcome_result(&opts, &ex);
        assert!(err.is_none());
        assert_eq!(result["content"][0]["text"], "3");
    }
}

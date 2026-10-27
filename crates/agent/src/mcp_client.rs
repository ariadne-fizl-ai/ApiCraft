//! Native MCP sessions over stdio (a child process) and the legacy HTTP+SSE
//! transport. Streamable HTTP needs nothing special and lives in
//! `apicraft_core::mcp`.

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use apicraft_core::mcp::{McpExchange, McpLaunch};
use serde_json::{json, Value};

const RESPONSE_TIMEOUT: Duration = Duration::from_secs(60);

pub fn session(launch: McpLaunch, messages: Vec<Value>) -> Result<McpExchange, String> {
    match launch {
        McpLaunch::Stdio { command, args, env } => stdio_session(&command, &args, &env, messages),
        McpLaunch::Sse { url, headers } => sse_session(&url, &headers, messages),
    }
}

/// What to do with a message that isn't the response we're waiting for.
/// Server-to-client requests get an answer so the server doesn't stall.
fn reply_to_server_request(msg: &Value) -> Option<Value> {
    let id = msg.get("id")?;
    let method = msg.get("method")?.as_str()?;
    Some(if method == "ping" {
        json!({ "jsonrpc": "2.0", "id": id, "result": {} })
    } else {
        json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": format!("{method} is not supported by Fizl ApiCraft") } })
    })
}

// ---------------------------------------------------------------- stdio

/// A bare program name (no path) that exists next to this executable, such as
/// the bundled `apicraft-agent`, resolves to that copy.
fn resolve_command(command: &str) -> String {
    if command.contains(['/', '\\']) {
        return command.to_string();
    }
    let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf())) else { return command.to_string() };
    for candidate in [dir.join(command), dir.join(format!("{command}{}", std::env::consts::EXE_SUFFIX))] {
        if candidate.is_file() {
            return candidate.display().to_string();
        }
    }
    command.to_string()
}

fn spawn(command: &str, args: &[String], env: &[(String, String)]) -> Result<Child, String> {
    let resolved = resolve_command(command);
    let command = resolved.as_str();
    let build = |program: &str, extra: &[&str]| {
        let mut c = Command::new(program);
        c.args(extra).args(args).envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str()))).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        c
    };
    match build(command, &[]).spawn() {
        Ok(c) => Ok(c),
        // On Windows, `npx`, `uvx` and friends are .cmd shims that need cmd.exe.
        Err(e) if cfg!(windows) && e.kind() == std::io::ErrorKind::NotFound => {
            build("cmd", &["/C", command]).spawn().map_err(|e2| format!("could not start {command}: {e} ({e2})"))
        }
        Err(e) => Err(format!("could not start {command}: {e}")),
    }
}

fn stdio_session(command: &str, args: &[String], env: &[(String, String)], messages: Vec<Value>) -> Result<McpExchange, String> {
    let mut child = spawn(command, args, env)?;
    let mut stdin = child.stdin.take().ok_or("no stdin")?;
    let stdout = child.stdout.take().ok_or("no stdout")?;
    let stderr = child.stderr.take().ok_or("no stderr")?;

    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let err_buf = Arc::new(Mutex::new(String::new()));
    let eb = err_buf.clone();
    std::thread::spawn(move || {
        let mut r = BufReader::new(stderr);
        let mut chunk = [0u8; 4096];
        while let Ok(n) = r.read(&mut chunk) {
            if n == 0 {
                break;
            }
            if let Ok(mut b) = eb.lock() {
                if b.len() < 64 * 1024 {
                    b.push_str(&String::from_utf8_lossy(&chunk[..n]));
                }
            }
        }
    });

    let mut ex = McpExchange::default();
    ex.info.push(("process id".into(), child.id().to_string()));
    let result = (|| -> Result<(), String> {
        for msg in messages {
            let line = serde_json::to_string(&msg).map_err(|e| e.to_string())?;
            writeln!(stdin, "{line}").and_then(|_| stdin.flush()).map_err(|e| format!("writing to the server: {e}"))?;
            let Some(id) = msg.get("id").cloned() else { continue };
            let deadline = Instant::now() + RESPONSE_TIMEOUT;
            loop {
                let left = deadline.saturating_duration_since(Instant::now());
                let line = match rx.recv_timeout(left) {
                    Ok(l) => l,
                    Err(RecvTimeoutError::Timeout) => return Err(format!("no response to {} within {}s", msg["method"], RESPONSE_TIMEOUT.as_secs())),
                    Err(RecvTimeoutError::Disconnected) => return Err("the server exited".into()),
                };
                let Ok(v) = serde_json::from_str::<Value>(line.trim()) else {
                    ex.notifications.push(json!({ "method": "stdout", "params": line }));
                    continue;
                };
                if v.get("id") == Some(&id) && v.get("method").is_none() {
                    ex.responses.push(v);
                    break;
                }
                if let Some(reply) = reply_to_server_request(&v) {
                    let _ = writeln!(stdin, "{reply}").and_then(|_| stdin.flush());
                }
                ex.notifications.push(v);
            }
        }
        Ok(())
    })();

    // Closing stdin asks the server to exit; give it a moment, then make sure.
    drop(stdin);
    let deadline = Instant::now() + Duration::from_millis(1500);
    while Instant::now() < deadline {
        if let Ok(Some(_)) = child.try_wait() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    if let Ok(None) = child.try_wait() {
        let _ = child.kill();
        let _ = child.wait();
    }
    std::thread::sleep(Duration::from_millis(20));
    ex.stderr = err_buf.lock().map(|b| b.clone()).unwrap_or_default();
    result.map_err(|e| if ex.stderr.trim().is_empty() { e } else { format!("{e}\n\nstderr:\n{}", ex.stderr.trim()) })?;
    Ok(ex)
}

// ---------------------------------------------------------------- legacy SSE

fn sse_events(reader: impl Read + Send + 'static) -> Receiver<(String, String)> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut event = String::new();
        let mut data: Vec<String> = Vec::new();
        for line in BufReader::new(reader).lines() {
            let Ok(line) = line else { break };
            let line = line.trim_end_matches('\r');
            if line.is_empty() {
                if !data.is_empty() && tx.send((if event.is_empty() { "message".into() } else { event.clone() }, data.join("\n"))).is_err() {
                    break;
                }
                event.clear();
                data.clear();
            } else if let Some(v) = line.strip_prefix("data:") {
                data.push(v.strip_prefix(' ').unwrap_or(v).to_string());
            } else if let Some(v) = line.strip_prefix("event:") {
                event = v.trim().to_string();
            }
        }
    });
    rx
}

fn resolve(base: &str, endpoint: &str) -> String {
    if endpoint.starts_with("http://") || endpoint.starts_with("https://") {
        return endpoint.to_string();
    }
    match apicraft_core::url::Url::parse(base) {
        Ok(u) if endpoint.starts_with('/') => format!("{}://{}{endpoint}", u.scheme, u.host_header()),
        Ok(u) => {
            let dir = u.path.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
            format!("{}://{}{dir}/{endpoint}", u.scheme, u.host_header())
        }
        Err(_) => endpoint.to_string(),
    }
}

fn sse_session(url: &str, headers: &[(String, String)], messages: Vec<Value>) -> Result<McpExchange, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).timeout_global(Some(Duration::from_secs(180))).build().into();
    let mut get = agent.get(url).header("Accept", "text/event-stream");
    for (k, v) in headers {
        get = get.header(k.as_str(), v.as_str());
    }
    let resp = get.call().map_err(|e| format!("could not open the event stream: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("the event stream answered {}", resp.status()));
    }
    let events = sse_events(resp.into_body().into_reader());
    let mut ex = McpExchange::default();

    // The server first names the endpoint to POST messages to.
    let endpoint = loop {
        match events.recv_timeout(Duration::from_secs(15)) {
            Ok((ev, data)) if ev == "endpoint" => break resolve(url, data.trim()),
            Ok((_, data)) => ex.notifications.push(serde_json::from_str(&data).unwrap_or(json!({ "method": "event", "params": data }))),
            Err(_) => return Err("the server never sent its endpoint event (is this an SSE MCP server?)".into()),
        }
    };
    ex.info.push(("endpoint".into(), endpoint.clone()));

    for msg in messages {
        let mut post = agent.post(&endpoint).header("Content-Type", "application/json");
        for (k, v) in headers {
            post = post.header(k.as_str(), v.as_str());
        }
        let r = post.send(serde_json::to_vec(&msg).unwrap_or_default()).map_err(|e| format!("posting {}: {e}", msg["method"]))?;
        if !r.status().is_success() {
            return Err(format!("the server answered {} to {}", r.status(), msg["method"]));
        }
        let Some(id) = msg.get("id").cloned() else { continue };
        let deadline = Instant::now() + RESPONSE_TIMEOUT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let (_, data) = events.recv_timeout(left).map_err(|_| format!("no response to {} on the event stream", msg["method"]))?;
            let Ok(v) = serde_json::from_str::<Value>(&data) else { continue };
            if v.get("id") == Some(&id) && v.get("method").is_none() {
                ex.responses.push(v);
                break;
            }
            if let Some(reply) = reply_to_server_request(&v) {
                let _ = agent.post(&endpoint).header("Content-Type", "application/json").send(reply.to_string());
            }
            ex.notifications.push(v);
        }
    }
    Ok(ex)
}

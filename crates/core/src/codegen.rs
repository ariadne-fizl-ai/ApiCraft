//! "Save as" curl and PowerShell. Both start from the same prepared request
//! the engine sends, so variables, params and auth come out the same.

use crate::auth;
use crate::http::HttpRequest;
use crate::model::{Auth, Catalog, Protocol};
use crate::prepare::{prepare, Prepared, PreparedBody};
use crate::vars::Vars;

mod workflow;
pub use workflow::generate_workflow;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Curl,
    PowerShell,
}

impl Target {
    pub fn file_extension(self) -> &'static str {
        match self {
            Target::Curl => "sh",
            Target::PowerShell => "ps1",
        }
    }
}

/// What code generation works from: the prepared request with auth applied
/// where it can be written as headers.
struct Plan {
    p: Prepared,
    headers: Vec<(String, String)>,
    url: String,
    /// Extra curl options for auth curl does itself (`-u`, `--digest`, `--aws-sigv4`).
    curl_auth: Vec<String>,
    /// PowerShell lines to put before the call, and the extra parameters.
    ps_setup: Vec<String>,
    ps_params: Vec<String>,
    notes: Vec<String>,
}

pub fn generate(catalog: &Catalog, request_id: &str, now_ms: u64, target: Target) -> Result<String, String> {
    let col = catalog.collection_of(request_id).ok_or("request not found")?;
    let vars = Vars::for_request(catalog, Some(col), &[], now_ms);
    generate_with(catalog, request_id, &vars, now_ms, target)
}

/// [`generate`] with the variables supplied by the caller.
fn generate_with(catalog: &Catalog, request_id: &str, vars: &Vars, now_ms: u64, target: Target) -> Result<String, String> {
    let (plan, mcp) = request_plan(catalog, request_id, vars, now_ms, target)?;
    Ok(match (plan.p.protocol, target) {
        (Protocol::Mcp, _) => mcp_code(&plan, &mcp, target)?,
        (Protocol::Grpc, Target::Curl) => grpcurl_sh(&plan),
        (Protocol::Grpc, Target::PowerShell) => grpcurl_ps(&plan),
        (Protocol::WebSocket, Target::Curl) => ws_curl(&plan),
        (Protocol::WebSocket, Target::PowerShell) => ws_ps(&plan),
        (_, Target::Curl) => curl(&plan),
        (_, Target::PowerShell) => powershell(&plan),
    })
}

fn request_plan(catalog: &Catalog, request_id: &str, vars: &Vars, now_ms: u64, target: Target) -> Result<(Plan, crate::model::McpOptions), String> {
    let (_, req) = catalog.find_request(request_id).ok_or("request not found")?;
    let p = prepare(req, vars);
    let (auth, _) = catalog.effective_auth(request_id).unwrap_or((Auth::None, String::new()));
    let auth = auth::resolve(&auth, vars);
    let mut mcp = req.mcp.clone();
    mcp.command = vars.apply(&mcp.command);
    mcp.args = mcp.args.iter().map(|a| vars.apply(a)).collect();
    mcp.target = vars.apply(&mcp.target);
    let mut connect = catalog.network.connect_options(vars);
    if let Auth::ClientCert(c) = &auth {
        connect.client_cert = c.certificate.trim().to_string();
        connect.client_key = c.key.trim().to_string();
    }
    let mut plan = plan(p, &auth, now_ms, target)?;
    connect_options(&mut plan, &connect, target);
    if req.send.accept_compressed && target == Target::Curl {
        plan.curl_auth.push("--compressed".into());
    }
    if req.send.compress != crate::model::Compression::None {
        plan.notes.push(format!("Fizl ApiCraft sends this body {}-compressed; this command sends it as plain text.", req.send.compress.label()));
    }
    Ok((plan, mcp))
}

/// Proxy and certificate settings as curl options or PowerShell parameters.
/// Certificates given as PEM text (rather than a file) can't be passed on a
/// command line, so they become a note.
fn connect_options(plan: &mut Plan, c: &crate::http::ConnectOptions, target: Target) {
    let is_file = |s: &str| !s.contains("-----BEGIN");
    match (&c.proxy, target) {
        (Some(p), Target::Curl) if p.is_empty() => plan.curl_auth.push("--noproxy '*'".into()),
        (Some(p), Target::Curl) => plan.curl_auth.push(format!("--proxy {}", sh(p))),
        (Some(p), Target::PowerShell) if p.is_empty() => plan.ps_params.push("-NoProxy".into()),
        (Some(p), Target::PowerShell) => plan.ps_params.push(format!("-Proxy {}", ps(p))),
        (None, _) => {}
    }
    if !c.no_proxy.is_empty() && target == Target::Curl && c.proxy.as_deref() != Some("") {
        plan.curl_auth.push(format!("--noproxy {}", sh(&c.no_proxy)));
    }
    if c.insecure {
        match target {
            Target::Curl => plan.curl_auth.push("--insecure".into()),
            Target::PowerShell => plan.ps_params.push("-SkipCertificateCheck".into()),
        }
    }
    if !c.ca.is_empty() {
        match target {
            Target::Curl if is_file(&c.ca) => plan.curl_auth.push(format!("--cacert {}", sh(&c.ca))),
            Target::Curl => plan.notes.push("Custom CA certificates are pasted PEM in Settings; save them to a file and add --cacert FILE.".into()),
            Target::PowerShell => {
                plan.notes.push("PowerShell trusts the system's CA store; add the custom CA certificates there (or use -SkipCertificateCheck).".into())
            }
        }
    }
    if !c.client_cert.is_empty() {
        if is_file(&c.client_cert) && is_file(&c.client_key) {
            match target {
                Target::Curl => {
                    plan.curl_auth.push(format!("--cert {}", sh(&c.client_cert)));
                    plan.curl_auth.push(format!("--key {}", sh(&c.client_key)));
                }
                Target::PowerShell => {
                    plan.ps_setup.push(format!(
                        "$certificate = [System.Security.Cryptography.X509Certificates.X509Certificate2]::CreateFromPemFile({}, {})",
                        ps(&c.client_cert),
                        ps(&c.client_key)
                    ));
                    plan.ps_params.push("-Certificate $certificate".into());
                }
            }
        } else {
            plan.notes.push("Client certificate: it's pasted PEM in the Auth tab; save the certificate and key to files to use them here.".into());
        }
    }
}

fn plan(p: Prepared, auth: &Auth, now_ms: u64, target: Target) -> Result<Plan, String> {
    let mut plan =
        Plan { headers: p.headers.clone(), url: p.url.clone(), p, curl_auth: Vec::new(), ps_setup: Vec::new(), ps_params: Vec::new(), notes: Vec::new() };
    if !plan.p.missing_vars.is_empty() {
        plan.notes.push(format!("Undefined variables left as written: {}", plan.p.missing_vars.join(", ")));
    }
    match auth {
        Auth::Basic { username, password } if target == Target::Curl => plan.curl_auth.push(format!("--user {}", sh(&format!("{username}:{password}")))),
        Auth::Digest { username, password } => match target {
            Target::Curl => plan.curl_auth.push(format!("--digest --user {}", sh(&format!("{username}:{password}")))),
            Target::PowerShell => {
                plan.ps_setup.push(format!(
                    "$credential = [System.Management.Automation.PSCredential]::new({}, (ConvertTo-SecureString {} -AsPlainText -Force))",
                    ps(username),
                    ps(password)
                ));
                plan.ps_params.push("-Credential $credential".into());
                if plan.url.starts_with("http://") {
                    plan.ps_params.push("-AllowUnencryptedAuthentication".into());
                }
            }
        },
        Auth::AwsSigV4(a) if target == Target::Curl => {
            let region = if a.region.is_empty() { "us-east-1" } else { &a.region };
            let service = if a.service.is_empty() { "execute-api" } else { &a.service };
            plan.curl_auth.push(format!("--aws-sigv4 {}", sh(&format!("aws:amz:{region}:{service}"))));
            plan.curl_auth.push(format!("--user {}", sh(&format!("{}:{}", a.access_key, a.secret_key))));
            if !a.session_token.is_empty() {
                plan.headers.push(("x-amz-security-token".into(), a.session_token.clone()));
            }
        }
        Auth::OAuth2(o) if o.access_token.is_empty() => plan.notes.push("OAuth 2: no access token yet; get one in the Auth tab and generate again.".into()),
        _ => {
            // Compute headers (or query) for the rest.
            let body = match &plan.p.body {
                PreparedBody::Text(t) => t.clone().into_bytes(),
                _ => Vec::new(),
            };
            let mut tmp = HttpRequest {
                method: plan.p.method.clone(),
                url: plan.url.clone(),
                headers: plan.headers.clone(),
                body,
                grpc: false,
                timeout_ms: 0,
                ..Default::default()
            };
            auth::apply(auth, &mut tmp, now_ms)?;
            plan.headers = tmp.headers;
            plan.url = tmp.url;
            match auth {
                Auth::OAuth1(_) => {
                    plan.notes.push("OAuth 1.0a: the signature includes a timestamp and nonce, so this command works once and only for a few minutes.".into())
                }
                Auth::AwsSigV4(_) => plan.notes.push("AWS SigV4: the signature expires after about 15 minutes; generate the script again after that.".into()),
                Auth::Jwt(_) => plan.notes.push("JWT: the token was signed when this was generated.".into()),
                _ => {}
            }
        }
    }
    Ok(plan)
}

// ---------------------------------------------------------------- quoting

/// POSIX shell single-quoting.
pub fn sh(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// PowerShell single-quoting. PowerShell also treats the typographic
/// single quotes as quote characters, so those are doubled too.
pub fn ps(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        if matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '\u{201a}' | '\u{201b}') {
            out.push(c);
        }
        out.push(c);
    }
    out.push('\'');
    out
}

/// A PowerShell here-string when the text allows it, else a quoted string.
fn ps_text(s: &str) -> String {
    if s.contains("\n'@") || s.starts_with("'@") {
        ps(s)
    } else {
        format!("@'\n{s}\n'@")
    }
}

fn comment_lines(prefix: &str, notes: &[String]) -> String {
    notes.iter().map(|n| format!("{prefix} {n}\n")).collect()
}

// ---------------------------------------------------------------- curl

fn curl(plan: &Plan) -> String {
    format!("{}curl {}\n", comment_lines("#", &plan.notes), curl_parts(plan).join(" \\\n  "))
}

/// curl's options and arguments for an HTTP request.
fn curl_parts(plan: &Plan) -> Vec<String> {
    let p = &plan.p;
    let mut parts: Vec<String> = Vec::new();
    let has_body = !matches!(p.body, PreparedBody::None);
    if p.method == "HEAD" {
        parts.push("--head".into());
    } else if (p.method != "GET" || has_body) && !(p.method == "POST" && has_body) {
        parts.push(format!("--request {}", p.method));
    }
    // The app follows redirects, so the command does too.
    parts.push("--location".into());
    parts.push(format!("--url {}", sh(&plan.url)));
    let multipart = matches!(p.body, PreparedBody::Multipart(_));
    for (k, v) in &plan.headers {
        if multipart && k.eq_ignore_ascii_case("content-type") {
            continue;
        }
        parts.push(format!("--header {}", sh(&format!("{k}: {v}"))));
    }
    parts.extend(plan.curl_auth.iter().cloned());
    match &p.body {
        PreparedBody::None => {}
        PreparedBody::Text(t) => parts.push(format!("--data-raw {}", sh(t))),
        PreparedBody::Multipart(fields) => {
            for (k, v) in fields {
                parts.push(format!("--form-string {}", sh(&format!("{k}={v}"))));
            }
        }
    }
    parts
}

fn grpcurl_sh(plan: &Plan) -> String {
    let p = &plan.p;
    let t = p.grpc.as_ref().expect("grpc target");
    let mut parts = Vec::new();
    if !t.is_tls() {
        parts.push("-plaintext".to_string());
    }
    if !t.proto.trim().is_empty() {
        parts.push("-proto service.proto".into());
    }
    for (k, v) in plan.headers.iter().filter(|(k, _)| !k.eq_ignore_ascii_case("content-type")) {
        parts.push(format!("-H {}", sh(&format!("{k}: {v}"))));
    }
    if let PreparedBody::Text(body) = &p.body {
        parts.push(format!("-d {}", sh(body)));
    }
    parts.push(sh(&t.host_port()));
    parts.push(sh(&format!("{}/{}", t.service, t.method)));
    let mut notes = vec!["gRPC via grpcurl (https://github.com/fullstorydev/grpcurl).".to_string()];
    if !t.proto.trim().is_empty() {
        notes.push("Save the request's .proto definition as service.proto next to this script.".into());
    }
    notes.extend(plan.notes.iter().cloned());
    format!("{}grpcurl {}\n", comment_lines("#", &notes), parts.join(" \\\n  "))
}

fn ws_curl(plan: &Plan) -> String {
    let mut parts = vec!["--no-buffer".to_string(), "--include".to_string()];
    for (k, v) in &plan.headers {
        parts.push(format!("--header {}", sh(&format!("{k}: {v}"))));
    }
    if !plan.p.subprotocols.is_empty() {
        parts.push(format!("--header {}", sh(&format!("Sec-WebSocket-Protocol: {}", plan.p.subprotocols.join(", ")))));
    }
    parts.extend(plan.curl_auth.iter().cloned());
    parts.push(sh(&plan.url));
    let mut notes = vec![
        "WebSocket: needs curl 8.11+ built with WebSocket support; prints frames as they arrive.".to_string(),
        format!("To send messages interactively, use websocat: websocat {}", sh(&plan.url)),
    ];
    notes.extend(plan.notes.iter().cloned());
    format!("{}curl {}\n", comment_lines("#", &notes), parts.join(" \\\n  "))
}

// ---------------------------------------------------------------- MCP

fn mcp_code(plan: &Plan, mcp: &crate::model::McpOptions, target: Target) -> Result<String, String> {
    use crate::model::{McpOperation, McpTransport};
    let body = match &plan.p.body {
        PreparedBody::Text(t) => t.clone(),
        _ => String::new(),
    };
    let quote = |s: &str| if target == Target::Curl { sh(s) } else { ps(s) };
    if mcp.transport == McpTransport::Http {
        let messages = crate::mcp::session_messages(mcp, &body)?;
        let json = |v: &serde_json::Value| serde_json::to_string(v).unwrap_or_default();
        let headers: Vec<(String, String)> = plan.headers.iter().filter(|(k, _)| !k.eq_ignore_ascii_case("content-type")).cloned().collect();
        let mut notes = vec![
            format!(
                "MCP over Streamable HTTP: initialize, then {}.",
                if mcp.operation == McpOperation::Initialize { "stop".to_string() } else { mcp.operation.method().to_string() }
            ),
            "Responses may come back as JSON or as a text/event-stream.".to_string(),
        ];
        notes.extend(plan.notes.iter().cloned());
        if target == Target::Curl {
            let mut extra = String::new();
            for (k, v) in &headers {
                extra.push_str(&format!(" \\\n  --header {}", sh(&format!("{k}: {v}"))));
            }
            let base = format!(
                "curl -s --url {} \\\n  --header 'Content-Type: application/json' \\\n  --header 'Accept: application/json, text/event-stream'{extra}",
                sh(&plan.url)
            );
            let mut out = comment_lines("#", &notes);
            out.push_str(&format!("SESSION=$({base} \\\n  --dump-header - --output /dev/null \\\n  --data-raw {} \\\n  | grep -i '^mcp-session-id:' | cut -d' ' -f2 | tr -d '\\r')\n", sh(&json(&messages[0]))));
            for m in &messages[1..] {
                out.push_str(&format!(
                    "{base} \\\n  --header \"Mcp-Session-Id: $SESSION\" \\\n  --header 'MCP-Protocol-Version: {}' \\\n  --data-raw {}\n",
                    crate::mcp::PROTOCOL_VERSION,
                    sh(&json(m))
                ));
            }
            return Ok(out);
        }
        let mut lines: Vec<String> = notes.iter().map(|n| format!("# {n}")).collect();
        lines.push("$headers = @{".into());
        lines.push("    'Accept' = 'application/json, text/event-stream'".into());
        for (k, v) in &headers {
            lines.push(format!("    {} = {}", ps(k), ps(v)));
        }
        lines.push("}".into());
        lines.push(format!("$url = {}", ps(&plan.url)));
        lines.push(format!(
            "$init = Invoke-WebRequest -Uri $url -Method Post -ContentType 'application/json' -Headers $headers -Body {}",
            ps(&json(&messages[0]))
        ));
        lines.push("$headers['Mcp-Session-Id'] = [string]$init.Headers['Mcp-Session-Id']".into());
        lines.push(format!("$headers['MCP-Protocol-Version'] = '{}'", crate::mcp::PROTOCOL_VERSION));
        for m in &messages[1..] {
            lines
                .push(format!("$response = Invoke-WebRequest -Uri $url -Method Post -ContentType 'application/json' -Headers $headers -Body {}", ps(&json(m))));
        }
        lines.push("$response.Content".into());
        return Ok(lines.join("\n") + "\n");
    }
    // stdio and SSE: the official MCP Inspector CLI.
    let mut parts: Vec<String> = vec!["npx".into(), "@modelcontextprotocol/inspector".into(), "--cli".into()];
    match mcp.transport {
        McpTransport::Stdio => {
            for kv in mcp.env.iter().filter(|kv| kv.enabled && !kv.key.is_empty()) {
                parts.push("-e".into());
                parts.push(quote(&format!("{}={}", kv.key, kv.value)));
            }
            let (program, args) = crate::mcp::command_and_args(mcp)?;
            parts.push(quote(&program));
            parts.extend(args.iter().map(|a| quote(a)));
        }
        _ => {
            parts.push(quote(&plan.url));
            parts.push("--transport".into());
            parts.push("sse".into());
            for (k, v) in plan.headers.iter().filter(|(k, _)| !k.eq_ignore_ascii_case("content-type")) {
                parts.push("--header".into());
                parts.push(quote(&format!("{k}: {v}")));
            }
        }
    }
    let method = if mcp.operation == McpOperation::Initialize { "tools/list" } else { mcp.operation.method() };
    if mcp.operation == McpOperation::Raw {
        let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
        parts.push("--method".into());
        parts.push(quote(v.get("method").and_then(|m| m.as_str()).unwrap_or("ping")));
    } else {
        parts.push("--method".into());
        parts.push(method.into());
    }
    match mcp.operation {
        McpOperation::CallTool | McpOperation::GetPrompt => {
            parts.push(if mcp.operation == McpOperation::CallTool { "--tool-name".into() } else { "--prompt-name".into() });
            parts.push(quote(&mcp.target));
            if let Ok(serde_json::Value::Object(args)) = serde_json::from_str::<serde_json::Value>(&body) {
                for (k, v) in args {
                    let text = match v {
                        serde_json::Value::String(s) => s,
                        other => other.to_string(),
                    };
                    parts.push(if mcp.operation == McpOperation::CallTool { "--tool-arg".into() } else { "--prompt-args".into() });
                    parts.push(quote(&format!("{k}={text}")));
                }
            }
        }
        McpOperation::ReadResource => {
            parts.push("--uri".into());
            parts.push(quote(&mcp.target));
        }
        _ => {}
    }
    let mut notes = vec!["MCP via the MCP Inspector CLI (needs Node.js): https://github.com/modelcontextprotocol/inspector".to_string()];
    notes.extend(plan.notes.iter().cloned());
    let sep = if target == Target::Curl { " \\\n  " } else { " `\n  " };
    Ok(format!("{}{}\n", comment_lines("#", &notes), parts.join(sep)))
}

// ---------------------------------------------------------------- PowerShell

const PS_METHODS: [&str; 9] = ["GET", "HEAD", "POST", "PUT", "DELETE", "TRACE", "OPTIONS", "MERGE", "PATCH"];

fn ps_headers(headers: &[(String, String)], skip_content_type: bool) -> (Vec<String>, Option<String>) {
    let mut content_type = None;
    let mut merged: Vec<(String, String)> = Vec::new();
    for (k, v) in headers {
        if k.eq_ignore_ascii_case("content-type") {
            content_type = Some(v.clone());
            if skip_content_type {
                continue;
            }
            continue;
        }
        match merged.iter_mut().find(|(mk, _)| mk.eq_ignore_ascii_case(k)) {
            Some((_, mv)) => {
                mv.push_str(", ");
                mv.push_str(v);
            }
            None => merged.push((k.clone(), v.clone())),
        }
    }
    let mut lines = Vec::new();
    if !merged.is_empty() {
        lines.push("$headers = @{".to_string());
        for (k, v) in merged {
            lines.push(format!("    {} = {}", ps(&k), ps(&v)));
        }
        lines.push("}".to_string());
    }
    (lines, content_type)
}

fn powershell(plan: &Plan) -> String {
    let (mut lines, call) = ps_request(plan);
    lines.push(format!("$response = {}", call.join(" `\n    ")));
    lines.push("$response.StatusCode".into());
    lines.push("$response.Content".into());
    lines.join("\n") + "\n"
}

/// The lines that set up an HTTP request in PowerShell, and the parts of
/// the `Invoke-WebRequest` call.
fn ps_request(plan: &Plan) -> (Vec<String>, Vec<String>) {
    let p = &plan.p;
    let mut lines: Vec<String> = plan.notes.iter().map(|n| format!("# {n}")).collect();
    let multipart = matches!(p.body, PreparedBody::Multipart(_));
    if multipart {
        lines.push("# -Form needs PowerShell 7 or later.".into());
    }
    lines.extend(plan.ps_setup.iter().cloned());
    let (header_lines, content_type) = ps_headers(&plan.headers, true);
    let has_headers = !header_lines.is_empty();
    lines.extend(header_lines);

    let mut call = vec![format!("Invoke-WebRequest -Uri {}", ps(&plan.url))];
    if PS_METHODS.contains(&p.method.as_str()) {
        if p.method != "GET" {
            let m = format!("{}{}", &p.method[..1], p.method[1..].to_ascii_lowercase());
            call.push(format!("-Method {m}"));
        }
    } else {
        call.push(format!("-CustomMethod {}", ps(&p.method)));
    }
    if has_headers {
        call.push("-Headers $headers".into());
    }
    match &p.body {
        PreparedBody::None => {}
        PreparedBody::Text(t) => {
            lines.push(format!("$body = {}", ps_text(t)));
            if let Some(ct) = content_type.as_ref() {
                call.push(format!("-ContentType {}", ps(ct)));
            }
            if t.is_ascii() {
                call.push("-Body $body".into());
            } else {
                // Windows PowerShell 5.1 would otherwise send ISO-8859-1.
                call.push("-Body ([System.Text.Encoding]::UTF8.GetBytes($body))".into());
            }
        }
        PreparedBody::Multipart(fields) => {
            lines.push("$form = @{".into());
            for (k, v) in fields {
                lines.push(format!("    {} = {}", ps(k), ps(v)));
            }
            lines.push("}".into());
            call.push("-Form $form".into());
        }
    }
    call.extend(plan.ps_params.iter().cloned());
    (lines, call)
}

fn grpcurl_ps(plan: &Plan) -> String {
    let p = &plan.p;
    let t = p.grpc.as_ref().expect("grpc target");
    let mut lines = vec!["# gRPC via grpcurl (https://github.com/fullstorydev/grpcurl). PowerShell 7.3+ passes the JSON quotes through unchanged.".to_string()];
    if !t.proto.trim().is_empty() {
        lines.push("# Save the request's .proto definition as service.proto next to this script.".into());
    }
    lines.extend(plan.notes.iter().map(|n| format!("# {n}")));
    let mut args = Vec::new();
    if !t.is_tls() {
        args.push("'-plaintext'".to_string());
    }
    if !t.proto.trim().is_empty() {
        args.push("'-proto'".into());
        args.push("'service.proto'".into());
    }
    for (k, v) in plan.headers.iter().filter(|(k, _)| !k.eq_ignore_ascii_case("content-type")) {
        args.push("'-H'".into());
        args.push(ps(&format!("{k}: {v}")));
    }
    if let PreparedBody::Text(body) = &p.body {
        lines.push(format!("$body = {}", ps_text(body)));
        args.push("'-d'".into());
        args.push("$body".into());
    }
    args.push(ps(&t.host_port()));
    args.push(ps(&format!("{}/{}", t.service, t.method)));
    lines.push(format!("grpcurl {}", args.join(" ")));
    lines.join("\n") + "\n"
}

fn ws_ps(plan: &Plan) -> String {
    let p = &plan.p;
    let mut lines: Vec<String> = plan.notes.iter().map(|n| format!("# {n}")).collect();
    lines.push("$ws = [System.Net.WebSockets.ClientWebSocket]::new()".into());
    for (k, v) in &plan.headers {
        lines.push(format!("$ws.Options.SetRequestHeader({}, {})", ps(k), ps(v)));
    }
    for sp in &p.subprotocols {
        lines.push(format!("$ws.Options.AddSubProtocol({})", ps(sp)));
    }
    lines.push("$cancel = [System.Threading.CancellationToken]::None".into());
    lines.push(format!("$ws.ConnectAsync([Uri]{}, $cancel).GetAwaiter().GetResult()", ps(&plan.url)));
    lines.push(String::new());
    lines.push("function Send-Text([string]$text) {".into());
    lines.push("    $bytes = [System.Text.Encoding]::UTF8.GetBytes($text)".into());
    lines.push(
        "    $ws.SendAsync([ArraySegment[byte]]::new($bytes), [System.Net.WebSockets.WebSocketMessageType]::Text, $true, $cancel).GetAwaiter().GetResult()"
            .into(),
    );
    lines.push("}".into());
    lines.push("function Receive-Text {".into());
    lines.push("    $buffer = [byte[]]::new(65536)".into());
    lines.push("    $text = [System.Text.StringBuilder]::new()".into());
    lines.push("    do {".into());
    lines.push("        $result = $ws.ReceiveAsync([ArraySegment[byte]]::new($buffer), $cancel).GetAwaiter().GetResult()".into());
    lines.push("        [void]$text.Append([System.Text.Encoding]::UTF8.GetString($buffer, 0, $result.Count))".into());
    lines.push("    } until ($result.EndOfMessage)".into());
    lines.push("    $text.ToString()".into());
    lines.push("}".into());
    lines.push(String::new());
    for m in &p.ws_messages {
        lines.push(format!("Send-Text {}", ps_text(m)));
        lines.push("Receive-Text".into());
    }
    if p.ws_messages.is_empty() {
        lines.push("Receive-Text".into());
    }
    lines.push("$ws.CloseAsync([System.Net.WebSockets.WebSocketCloseStatus]::NormalClosure, 'done', $cancel).GetAwaiter().GetResult()".into());
    lines.join("\n") + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::*;

    fn catalog_with(r: Request) -> (Catalog, String) {
        let mut cat = Catalog::default();
        let mut c = Collection::new("c");
        let id = r.id.clone();
        c.items.push(Item::Request(r));
        cat.collections.push(c);
        (cat, id)
    }

    #[test]
    fn curl_and_powershell_for_json_post() {
        let mut r = Request::new("r", Protocol::Rest);
        r.method = "POST".into();
        r.url = "https://api.test/items".into();
        r.headers.push(KeyValue::new("X-Note", "it's"));
        r.body = Body::Raw { content_type: String::new(), text: "{\"a\":\"b\"}".into() };
        r.auth = Auth::Bearer { token: "tok".into() };
        let (cat, id) = catalog_with(r);
        let c = generate(&cat, &id, 0, Target::Curl).unwrap();
        assert_eq!(
            c,
            "curl --location \\\n  --url 'https://api.test/items' \\\n  --header 'X-Note: it'\\''s' \\\n  --header 'Content-Type: application/json' \\\n  --header 'Authorization: Bearer tok' \\\n  --data-raw '{\"a\":\"b\"}'\n"
        );
        let p = generate(&cat, &id, 0, Target::PowerShell).unwrap();
        assert!(p.contains("'X-Note' = 'it''s'"), "{p}");
        assert!(p.contains("-Method Post"));
        assert!(p.contains("-ContentType 'application/json'"));
        assert!(p.contains("$body = @'\n{\"a\":\"b\"}\n'@"));
    }

    #[test]
    fn digest_and_grpc() {
        let mut r = Request::new("r", Protocol::Rest);
        r.url = "http://h/x".into();
        r.auth = Auth::Digest { username: "u".into(), password: "p".into() };
        let (cat, id) = catalog_with(r);
        assert!(generate(&cat, &id, 0, Target::Curl).unwrap().contains("--digest --user 'u:p'"));
        assert!(generate(&cat, &id, 0, Target::PowerShell).unwrap().contains("-AllowUnencryptedAuthentication"));

        let mut g = Request::new("g", Protocol::Grpc);
        g.url = "https://grpc.test".into();
        g.grpc.service = "pkg.Svc".into();
        g.grpc.method = "Do".into();
        let (cat, id) = catalog_with(g);
        let s = generate(&cat, &id, 0, Target::Curl).unwrap();
        assert!(s.contains("grpcurl -d '{}' \\\n  'grpc.test:443' \\\n  'pkg.Svc/Do'"), "{s}");
    }

    #[test]
    fn ps_quotes_typographic() {
        assert_eq!(ps("it\u{2019}s"), "'it\u{2019}\u{2019}s'");
    }
}

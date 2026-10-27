//! Postman collection (v2.0 / v2.1) and environment import, and v2.1 export.
//!
//! Postman test scripts are JavaScript, which we don't run. Common patterns
//! (`pm.environment.set("x", pm.response.json().a.b)`,
//! `pm.response.to.have.status(200)`, ...) are turned into extraction rules
//! and checks; anything else is kept in the request description.

use serde_json::{json, Value};

use crate::model::*;

pub fn is_collection(v: &Value) -> bool {
    v.get("info").and_then(|i| i.get("schema")).and_then(|s| s.as_str()).map(|s| s.contains("getpostman.com") || s.contains("postman")).unwrap_or(false)
        || (v.get("info").is_some() && v.get("item").is_some())
}

pub fn is_environment(v: &Value) -> bool {
    v.get("values").map(|x| x.is_array()).unwrap_or(false) && v.get("name").is_some()
}

fn s(v: &Value, key: &str) -> String {
    match v.get(key) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

fn description(v: &Value) -> String {
    match v.get("description") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Object(o)) => o.get("content").and_then(|c| c.as_str()).unwrap_or("").to_string(),
        _ => String::new(),
    }
}

fn kv_list(v: Option<&Value>) -> Vec<KeyValue> {
    v.and_then(|x| x.as_array())
        .map(|arr| {
            arr.iter()
                .filter(|e| e.get("key").is_some())
                .map(|e| KeyValue { key: s(e, "key"), value: s(e, "value"), enabled: !e.get("disabled").and_then(|d| d.as_bool()).unwrap_or(false) })
                .collect()
        })
        .unwrap_or_default()
}

pub fn import_environment(v: &Value) -> Result<Environment, String> {
    let mut env = Environment::new(s(v, "name"));
    env.variables = v
        .get("values")
        .and_then(|x| x.as_array())
        .map(|arr| {
            arr.iter()
                .map(|e| KeyValue { key: s(e, "key"), value: s(e, "value"), enabled: e.get("enabled").and_then(|d| d.as_bool()).unwrap_or(true) })
                .collect()
        })
        .unwrap_or_default();
    Ok(env)
}

pub fn import_collection(v: &Value) -> Result<Collection, String> {
    let info = v.get("info").ok_or("not a Postman collection (no info)")?;
    let mut col = Collection::new(s(info, "name"));
    if col.name.is_empty() {
        col.name = "Imported collection".into();
    }
    col.description = description(info);
    col.variables = kv_list(v.get("variable"));
    col.auth = v.get("auth").map(import_auth).unwrap_or(Auth::None);
    if col.auth == Auth::Inherit {
        col.auth = Auth::None;
    }
    col.items = import_items(v.get("item"))?;
    Ok(col)
}

fn import_items(items: Option<&Value>) -> Result<Vec<Item>, String> {
    let Some(arr) = items.and_then(|i| i.as_array()) else { return Ok(Vec::new()) };
    let mut out = Vec::new();
    for it in arr {
        if it.get("item").is_some() && it.get("request").is_none() {
            let mut f = Folder::new(s(it, "name"));
            f.items = import_items(it.get("item"))?;
            out.push(Item::Folder(f));
        } else {
            out.push(Item::Request(import_request(it)));
        }
    }
    Ok(out)
}

fn import_request(it: &Value) -> Request {
    let mut r = Request::new(s(it, "name"), Protocol::Rest);
    let req = it.get("request").cloned().unwrap_or(Value::Null);
    if let Value::String(url) = &req {
        r.url = url.clone();
        return r;
    }
    r.method = match s(&req, "method") {
        m if m.is_empty() => "GET".into(),
        m => m.to_uppercase(),
    };
    r.description = description(&req);
    r.headers = kv_list(req.get("header"));
    import_url(&req, &mut r);
    import_body(&req, &mut r);
    r.auth = req.get("auth").map(import_auth).unwrap_or(Auth::Inherit);
    if let Some(Value::Object(a)) = req.get("auth") {
        if let Some(t) = a.get("type").and_then(|t| t.as_str()) {
            if Auth::KINDS.iter().all(|(k, _)| *k != map_auth_type(t)) {
                note(&mut r, &format!("Postman auth \u{201c}{t}\u{201d} is not supported; set auth manually."));
            }
        }
    }
    detect_soap(&mut r);
    import_scripts(it, &mut r);
    r
}

fn note(r: &mut Request, text: &str) {
    if !r.description.is_empty() {
        r.description.push_str("\n\n");
    }
    r.description.push_str(text);
}

fn import_url(req: &Value, r: &mut Request) {
    match req.get("url") {
        Some(Value::String(raw)) => split_url(raw, r),
        Some(u @ Value::Object(_)) => {
            let raw = s(u, "raw");
            if raw.is_empty() {
                // Rebuild from parts.
                let protocol = s(u, "protocol");
                let host = join_parts(u.get("host"), ".");
                let path = join_parts(u.get("path"), "/");
                let mut url = String::new();
                if !protocol.is_empty() {
                    url.push_str(&protocol);
                    url.push_str("://");
                }
                url.push_str(&host);
                if !path.is_empty() {
                    url.push('/');
                    url.push_str(&path);
                }
                r.url = url;
                r.params = kv_list(u.get("query"));
            } else {
                split_url(&raw, r);
                if u.get("query").is_some() {
                    r.params = kv_list(u.get("query"));
                }
            }
            // Path variables (`/users/:id`) become their values.
            for pv in kv_list(u.get("variable")) {
                if !pv.value.is_empty() {
                    r.url = r.url.replace(&format!(":{}", pv.key), &pv.value);
                }
            }
        }
        _ => {}
    }
}

fn join_parts(v: Option<&Value>, sep: &str) -> String {
    match v {
        Some(Value::Array(a)) => a.iter().map(|x| x.as_str().map(str::to_string).unwrap_or_else(|| s(x, "value"))).collect::<Vec<_>>().join(sep),
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    }
}

fn split_url(raw: &str, r: &mut Request) {
    match raw.split_once('?') {
        Some((base, q)) => {
            r.url = base.to_string();
            r.params = q
                .split('&')
                .filter(|p| !p.is_empty())
                .map(|p| {
                    let (k, v) = p.split_once('=').unwrap_or((p, ""));
                    // Keep {{vars}} and encoding exactly as written.
                    KeyValue::new(crate::prepare::percent_decode(k), crate::prepare::percent_decode(v))
                })
                .collect();
        }
        None => r.url = raw.to_string(),
    }
}

fn import_body(req: &Value, r: &mut Request) {
    let Some(body) = req.get("body") else { return };
    if body.get("disabled").and_then(|d| d.as_bool()).unwrap_or(false) {
        return;
    }
    r.body = match s(body, "mode").as_str() {
        "raw" => {
            let lang = body.get("options").and_then(|o| o.get("raw")).and_then(|o| o.get("language")).and_then(|l| l.as_str()).unwrap_or("");
            let ct = match lang {
                "json" => "application/json",
                "xml" => "application/xml",
                "html" => "text/html",
                "javascript" => "application/javascript",
                _ => "",
            };
            Body::Raw { content_type: ct.into(), text: s(body, "raw") }
        }
        "urlencoded" => Body::UrlEncoded { fields: kv_list(body.get("urlencoded")) },
        "formdata" => {
            let fields = kv_list(body.get("formdata"));
            let has_files = body.get("formdata").and_then(|f| f.as_array()).map(|a| a.iter().any(|e| s(e, "type") == "file")).unwrap_or(false);
            if has_files {
                note(r, "Postman file fields were imported as text fields; file uploads are not supported.");
            }
            Body::FormData { fields }
        }
        "graphql" => {
            let g = body.get("graphql").cloned().unwrap_or(Value::Null);
            r.protocol = Protocol::GraphQl;
            r.graphql.query = s(&g, "query");
            r.graphql.variables = s(&g, "variables");
            Body::None
        }
        "file" => {
            note(r, "Postman file body was not imported.");
            Body::None
        }
        _ => Body::None,
    };
}

fn detect_soap(r: &mut Request) {
    let soap_action = r.headers.iter().position(|h| h.key.eq_ignore_ascii_case("soapaction"));
    let ct = r.headers.iter().find(|h| h.key.eq_ignore_ascii_case("content-type")).map(|h| h.value.to_ascii_lowercase()).unwrap_or_default();
    let body = r.body.raw_text();
    let looks_soap = soap_action.is_some() || ct.contains("soap+xml") || (body.contains("Envelope") && body.contains("soap"));
    if !looks_soap {
        return;
    }
    r.protocol = Protocol::Soap;
    if ct.contains("soap+xml") {
        r.soap.version = SoapVersion::V12;
        if let Some(a) = ct.split(';').find_map(|p| p.trim().strip_prefix("action=")) {
            r.soap.action = a.trim_matches('"').to_string();
        }
    }
    if let Some(i) = soap_action {
        r.soap.action = r.headers[i].value.trim_matches('"').to_string();
        r.headers.remove(i);
    }
    r.headers.retain(|h| !h.key.eq_ignore_ascii_case("content-type"));
}

fn map_auth_type(t: &str) -> &str {
    match t {
        "noauth" => "none",
        other => other,
    }
}

/// Postman auth params are `[{key, value}]` (v2.1) or an object (v2.0).
fn auth_params(a: &Value, kind: &str) -> impl Fn(&str) -> String {
    let p = a.get(kind).cloned().unwrap_or(Value::Null);
    move |key: &str| match &p {
        Value::Array(arr) => arr.iter().find(|e| s(e, "key") == key).map(|e| s(e, "value")).unwrap_or_default(),
        Value::Object(_) => s(&p, key),
        _ => String::new(),
    }
}

pub fn import_auth(a: &Value) -> Auth {
    let t = s(a, "type");
    let get = auth_params(a, &t);
    match t.as_str() {
        "noauth" => Auth::None,
        "inherit" | "" => Auth::Inherit,
        "basic" => Auth::Basic { username: get("username"), password: get("password") },
        "digest" => Auth::Digest { username: get("username"), password: get("password") },
        "bearer" => Auth::Bearer { token: get("token") },
        "apikey" => Auth::ApiKey { key: get("key"), value: get("value"), in_header: get("in") != "query" },
        "oauth2" => {
            let grant = match get("grant_type").as_str() {
                "password_credentials" => GrantType::Password,
                "authorization_code" | "authorization_code_with_pkce" => GrantType::AuthorizationCode,
                "implicit" => GrantType::Manual,
                "client_credentials" => GrantType::ClientCredentials,
                _ if !get("accessToken").is_empty() => GrantType::Manual,
                _ => GrantType::ClientCredentials,
            };
            let prefix = get("headerPrefix");
            Auth::OAuth2(OAuth2 {
                grant,
                auth_url: get("authUrl"),
                token_url: get("accessTokenUrl"),
                client_id: get("clientId"),
                client_secret: get("clientSecret"),
                scope: get("scope"),
                audience: get("audience"),
                username: get("username"),
                password: get("password"),
                redirect_uri: get("redirect_uri"),
                use_pkce: get("grant_type") == "authorization_code_with_pkce" || grant == GrantType::AuthorizationCode,
                client_auth_basic: get("client_authentication") != "body",
                header_prefix: if prefix.is_empty() { "Bearer".into() } else { prefix },
                access_token: get("accessToken"),
                refresh_token: get("refreshToken"),
                expires_at: 0,
            })
        }
        "oauth1" => Auth::OAuth1(OAuth1 {
            consumer_key: get("consumerKey"),
            consumer_secret: get("consumerSecret"),
            token: get("token"),
            token_secret: get("tokenSecret"),
            signature_method: OAuth1Method::from_name(&get("signatureMethod")),
            realm: get("realm"),
        }),
        "awsv4" => Auth::AwsSigV4(AwsSigV4 {
            access_key: get("accessKey"),
            secret_key: get("secretKey"),
            session_token: get("sessionToken"),
            region: get("region"),
            service: get("service"),
        }),
        "jwt" => {
            let prefix = get("headerPrefix");
            let algorithm = get("algorithm");
            Auth::Jwt(JwtAuth {
                algorithm: if algorithm.is_empty() { "HS256".into() } else { algorithm },
                secret: get("secret"),
                payload: get("payload"),
                expires_in_secs: 0,
                header_prefix: if prefix.is_empty() { "Bearer".into() } else { prefix },
            })
        }
        _ => Auth::None,
    }
}

// ---------------------------------------------------------------- scripts

fn script_lines(it: &Value, listen: &str) -> Vec<String> {
    it.get("event")
        .and_then(|e| e.as_array())
        .map(|events| {
            events
                .iter()
                .filter(|e| s(e, "listen") == listen)
                .flat_map(|e| match e.get("script").and_then(|sc| sc.get("exec")) {
                    Some(Value::Array(lines)) => lines.iter().filter_map(|l| l.as_str().map(str::to_string)).collect::<Vec<_>>(),
                    Some(Value::String(s)) => s.lines().map(str::to_string).collect(),
                    _ => Vec::new(),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn import_scripts(it: &Value, r: &mut Request) {
    let lines = script_lines(it, "test");
    if lines.iter().all(|l| l.trim().is_empty()) {
        let pre = script_lines(it, "prerequest");
        if pre.iter().any(|l| !l.trim().is_empty()) {
            note(r, &format!("Postman pre-request script (not run):\n{}", pre.join("\n")));
        }
        return;
    }
    let script = lines.join("\n");
    // Names bound to the parsed body: `var jsonData = pm.response.json();`
    let json_var =
        regex_lite::Regex::new(r"(?:var|let|const)\s+(\w+)\s*=\s*(?:pm\.response\.json\(\)|JSON\.parse\(\s*(?:responseBody|pm\.response\.text\(\))\s*\))")
            .unwrap();
    let json_names: Vec<String> = json_var.captures_iter(&script).map(|c| c[1].to_string()).collect();
    let set = regex_lite::Regex::new(r#"(?:pm\.(environment|globals|collectionVariables|variables)\.set|postman\.set(Environment|Global)Variable)\(\s*["']([^"']+)["']\s*,\s*(.+?)\s*\)\s*;?\s*$"#).unwrap();
    let mut converted = 0;
    let mut leftover = Vec::new();

    for line in &lines {
        let t = line.trim();
        if let Some(c) = set.captures(t) {
            let scope = match c.get(1).map(|m| m.as_str()).or_else(|| c.get(2).map(|m| m.as_str())) {
                Some("environment") | Some("Environment") => VarScope::Environment,
                _ => VarScope::Runtime,
            };
            if let Some((source, expression)) = value_expression(&c[4], &json_names) {
                r.extract.push(Extraction { variable: c[3].to_string(), source, expression, scope, enabled: true });
                converted += 1;
                continue;
            }
        }
        if let Some(check) = check_from_line(t) {
            if !r.checks.contains(&check) {
                r.checks.push(check);
            }
            converted += 1;
            continue;
        }
        leftover.push(line.clone());
    }
    let meaningful = leftover.iter().any(|l| {
        let t = l.trim();
        !(t.is_empty() || t.starts_with("//") || t == "});" || t == "})" || t == "}" || json_var.is_match(t) || t.starts_with("pm.test("))
    });
    if meaningful || converted == 0 {
        note(r, &format!("Postman test script (only simple variable captures and checks were converted):\n{script}"));
    }
}

/// `pm.response.json().data.id`, `jsonData.token`, `pm.response.headers.get("X")`, `pm.response.code`.
fn value_expression(expr: &str, json_names: &[String]) -> Option<(ExtractSource, String)> {
    let e = expr.trim();
    for prefix in ["pm.response.json()", "JSON.parse(responseBody)", "JSON.parse(pm.response.text())"] {
        if let Some(path) = e.strip_prefix(prefix) {
            return Some((ExtractSource::Json, format!("${path}")));
        }
    }
    for name in json_names {
        if let Some(path) = e.strip_prefix(name.as_str()) {
            if path.is_empty() || path.starts_with('.') || path.starts_with('[') {
                return Some((ExtractSource::Json, format!("${path}")));
            }
        }
    }
    let header = regex_lite::Regex::new(r#"^(?:pm\.response\.headers\.get|postman\.getResponseHeader)\(\s*["']([^"']+)["']\s*\)$"#).unwrap();
    if let Some(c) = header.captures(e) {
        return Some((ExtractSource::Header, c[1].to_string()));
    }
    if e == "pm.response.code" || e == "responseCode.code" {
        return Some((ExtractSource::Status, String::new()));
    }
    None
}

fn check_from_line(t: &str) -> Option<Check> {
    let status = regex_lite::Regex::new(r"pm\.response\.to\.have\.status\(\s*(\d{3})\s*\)").unwrap();
    if let Some(c) = status.captures(t) {
        return Some(Check { kind: CheckKind::Status, target: String::new(), expected: c[1].to_string(), enabled: true });
    }
    if t.contains("pm.response.to.be.ok") || t.contains("pm.response.to.be.success") {
        return Some(Check { kind: CheckKind::Status, target: String::new(), expected: "2xx".into(), enabled: true });
    }
    let code = regex_lite::Regex::new(r"pm\.expect\(\s*pm\.response\.code\s*\)\.to\.(?:eql|equal|be\.equal)\(\s*(\d{3})\s*\)").unwrap();
    if let Some(c) = code.captures(t) {
        return Some(Check { kind: CheckKind::Status, target: String::new(), expected: c[1].to_string(), enabled: true });
    }
    let time = regex_lite::Regex::new(r"pm\.expect\(\s*pm\.response\.responseTime\s*\)\.to\.be\.below\(\s*(\d+)\s*\)").unwrap();
    if let Some(c) = time.captures(t) {
        return Some(Check { kind: CheckKind::TimeBelow, target: String::new(), expected: c[1].to_string(), enabled: true });
    }
    let body = regex_lite::Regex::new(r#"pm\.expect\(\s*pm\.response\.text\(\)\s*\)\.to\.include\(\s*["']([^"']*)["']\s*\)"#).unwrap();
    if let Some(c) = body.captures(t) {
        return Some(Check { kind: CheckKind::BodyContains, target: String::new(), expected: c[1].to_string(), enabled: true });
    }
    let header = regex_lite::Regex::new(r#"pm\.response\.to\.have\.header\(\s*["']([^"']+)["']\s*\)"#).unwrap();
    if let Some(c) = header.captures(t) {
        return Some(Check { kind: CheckKind::Header, target: c[1].to_string(), expected: String::new(), enabled: true });
    }
    None
}

// ---------------------------------------------------------------- export (v2.1)

pub fn export_collection(c: &Collection) -> Value {
    let mut out = json!({
        "info": {
            "_postman_id": c.id,
            "name": c.name,
            "description": c.description,
            "schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json"
        },
        "item": export_items(&c.items),
        "variable": c.variables.iter().map(export_kv).collect::<Vec<_>>(),
    });
    if let Some(a) = export_auth(&c.auth) {
        out["auth"] = a;
    }
    out
}

pub fn export_environment(e: &Environment) -> Value {
    json!({
        "id": e.id,
        "name": e.name,
        "values": e.variables.iter().map(|kv| json!({ "key": kv.key, "value": kv.value, "enabled": kv.enabled })).collect::<Vec<_>>(),
        "_postman_variable_scope": "environment"
    })
}

fn export_kv(kv: &KeyValue) -> Value {
    let mut v = json!({ "key": kv.key, "value": kv.value });
    if !kv.enabled {
        v["disabled"] = json!(true);
    }
    v
}

fn export_items(items: &[Item]) -> Vec<Value> {
    items
        .iter()
        .map(|i| match i {
            Item::Folder(f) => json!({ "name": f.name, "item": export_items(&f.items) }),
            Item::Request(r) => export_request(r),
        })
        .collect()
}

fn export_request(r: &Request) -> Value {
    let mut headers: Vec<Value> = r.headers.iter().map(export_kv).collect();
    let mut method = r.method.clone();
    let mut description = r.description.clone();
    match r.protocol {
        Protocol::Soap => {
            method = "POST".into();
            match r.soap.version {
                SoapVersion::V11 => {
                    headers.push(json!({ "key": "Content-Type", "value": "text/xml; charset=utf-8" }));
                    headers.push(json!({ "key": "SOAPAction", "value": format!("\"{}\"", r.soap.action) }));
                }
                SoapVersion::V12 => {
                    headers.push(json!({ "key": "Content-Type", "value": format!("application/soap+xml; charset=utf-8; action=\"{}\"", r.soap.action) }))
                }
            }
        }
        Protocol::Grpc => {
            description = format!(
                "gRPC {}/{} (exported from Fizl ApiCraft; Postman collections cannot hold gRPC requests).\n\n{description}",
                r.grpc.service, r.grpc.method
            );
        }
        Protocol::WebSocket => {
            description = format!("WebSocket request (exported from Fizl ApiCraft; Postman collections cannot hold WebSocket requests).\n\n{description}");
        }
        Protocol::Mcp => {
            description =
                format!("MCP {} (exported from Fizl ApiCraft; Postman collections cannot hold MCP requests).\n\n{description}", r.mcp.operation.label());
        }
        Protocol::OData => {
            for (k, v) in crate::odata::headers(r.odata.version) {
                headers.push(json!({ "key": k, "value": v }));
            }
        }
        Protocol::Rest | Protocol::GraphQl => {}
    }
    let (url, mut query): (String, Vec<Value>) = match r.protocol {
        Protocol::OData => (
            crate::odata::resource_url(&r.url, &r.odata.resource),
            crate::odata::query_options(&r.odata).into_iter().map(|(k, v)| json!({ "key": k, "value": v })).collect(),
        ),
        _ => (r.url.clone(), Vec::new()),
    };
    query.extend(r.params.iter().map(export_kv));
    let raw_query: Vec<String> = query
        .iter()
        .filter(|p| p.get("disabled").is_none())
        .map(|p| format!("{}={}", p["key"].as_str().unwrap_or(""), p["value"].as_str().unwrap_or("")))
        .collect();
    let raw = if raw_query.is_empty() { url } else { format!("{url}?{}", raw_query.join("&")) };
    let mut req = json!({
        "method": method,
        "header": headers,
        "url": { "raw": raw, "query": query },
        "description": description.trim(),
    });
    match &r.body {
        _ if r.protocol == Protocol::GraphQl => {
            req["body"] = json!({ "mode": "graphql", "graphql": { "query": r.graphql.query, "variables": r.graphql.variables } });
        }
        Body::None => {}
        Body::Raw { content_type, text } => {
            let lang = if content_type.contains("json") || r.protocol == Protocol::Grpc {
                "json"
            } else if content_type.contains("xml") || r.protocol == Protocol::Soap {
                "xml"
            } else {
                "text"
            };
            req["body"] = json!({ "mode": "raw", "raw": text, "options": { "raw": { "language": lang } } });
        }
        Body::UrlEncoded { fields } => req["body"] = json!({ "mode": "urlencoded", "urlencoded": fields.iter().map(export_kv).collect::<Vec<_>>() }),
        Body::FormData { fields } => {
            req["body"] = json!({ "mode": "formdata", "formdata": fields.iter().map(|kv| { let mut v = export_kv(kv); v["type"] = json!("text"); v }).collect::<Vec<_>>() })
        }
    }
    if let Some(a) = export_auth(&r.auth) {
        req["auth"] = a;
    }
    let mut item = json!({ "name": r.name, "request": req });
    let script = export_script(r);
    if !script.is_empty() {
        item["event"] = json!([{ "listen": "test", "script": { "type": "text/javascript", "exec": script } }]);
    }
    item
}

fn export_script(r: &Request) -> Vec<String> {
    let mut lines = Vec::new();
    for e in r.extract.iter().filter(|e| e.enabled) {
        let target = match e.scope {
            VarScope::Environment => "pm.environment",
            VarScope::Runtime => "pm.collectionVariables",
        };
        let value = match e.source {
            ExtractSource::Json => {
                let path = e.expression.trim().trim_start_matches('$');
                let path = if path.is_empty() || path.starts_with('.') || path.starts_with('[') { path.to_string() } else { format!(".{path}") };
                format!("pm.response.json(){path}")
            }
            ExtractSource::Header => format!("pm.response.headers.get({})", serde_json::to_string(&e.expression).unwrap_or_default()),
            ExtractSource::Status => "pm.response.code".into(),
            ExtractSource::Regex => format!("(pm.response.text().match(new RegExp({})) || [])[1]", serde_json::to_string(&e.expression).unwrap_or_default()),
            ExtractSource::Xml => format!("xml2Json(pm.response.text()) /* XML path: {} */", e.expression),
        };
        lines.push(format!("{target}.set({}, {value});", serde_json::to_string(&e.variable).unwrap_or_default()));
    }
    for c in r.checks.iter().filter(|c| c.enabled) {
        let name = serde_json::to_string(&crate::extract::describe(c)).unwrap_or_default();
        let body = match c.kind {
            CheckKind::Status => match c.expected.trim().parse::<u16>() {
                Ok(code) => format!("pm.response.to.have.status({code});"),
                Err(_) => "pm.response.to.be.success;".into(),
            },
            CheckKind::BodyContains => format!("pm.expect(pm.response.text()).to.include({});", serde_json::to_string(&c.expected).unwrap_or_default()),
            CheckKind::TimeBelow => format!("pm.expect(pm.response.responseTime).to.be.below({});", c.expected.trim()),
            CheckKind::Header => format!("pm.response.to.have.header({});", serde_json::to_string(&c.target).unwrap_or_default()),
            CheckKind::JsonEquals => format!(
                "pm.expect(String(pm.response.json(){})).to.eql({});",
                if c.target.trim_start_matches('$').starts_with(['.', '[']) {
                    c.target.trim_start_matches('$').to_string()
                } else {
                    format!(".{}", c.target.trim_start_matches('$'))
                },
                serde_json::to_string(&c.expected).unwrap_or_default()
            ),
        };
        lines.push(format!("pm.test({name}, function () {{ {body} }});"));
    }
    lines
}

fn pm_params(pairs: &[(&str, String)]) -> Value {
    Value::Array(pairs.iter().map(|(k, v)| json!({ "key": k, "value": v, "type": "string" })).collect())
}

fn export_auth(a: &Auth) -> Option<Value> {
    Some(match a {
        Auth::Inherit => return None,
        Auth::None => json!({ "type": "noauth" }),
        Auth::Basic { username, password } => json!({ "type": "basic", "basic": pm_params(&[("username", username.clone()), ("password", password.clone())]) }),
        Auth::Digest { username, password } => {
            json!({ "type": "digest", "digest": pm_params(&[("username", username.clone()), ("password", password.clone())]) })
        }
        Auth::Bearer { token } => json!({ "type": "bearer", "bearer": pm_params(&[("token", token.clone())]) }),
        Auth::ApiKey { key, value, in_header } => {
            json!({ "type": "apikey", "apikey": pm_params(&[("key", key.clone()), ("value", value.clone()), ("in", if *in_header { "header" } else { "query" }.to_string())]) })
        }
        Auth::OAuth2(o) => {
            let grant = match o.grant {
                GrantType::ClientCredentials => "client_credentials",
                GrantType::Password => "password_credentials",
                GrantType::AuthorizationCode if o.use_pkce => "authorization_code_with_pkce",
                GrantType::AuthorizationCode => "authorization_code",
                GrantType::Manual => "implicit",
            };
            json!({ "type": "oauth2", "oauth2": pm_params(&[
                ("grant_type", grant.to_string()),
                ("accessTokenUrl", o.token_url.clone()),
                ("authUrl", o.auth_url.clone()),
                ("clientId", o.client_id.clone()),
                ("clientSecret", o.client_secret.clone()),
                ("scope", o.scope.clone()),
                ("username", o.username.clone()),
                ("password", o.password.clone()),
                ("redirect_uri", o.redirect_uri.clone()),
                ("client_authentication", if o.client_auth_basic { "header" } else { "body" }.to_string()),
                ("headerPrefix", o.header_prefix.clone()),
                ("accessToken", o.access_token.clone()),
                ("addTokenTo", "header".to_string()),
            ]) })
        }
        Auth::OAuth1(o) => json!({ "type": "oauth1", "oauth1": pm_params(&[
            ("consumerKey", o.consumer_key.clone()),
            ("consumerSecret", o.consumer_secret.clone()),
            ("token", o.token.clone()),
            ("tokenSecret", o.token_secret.clone()),
            ("signatureMethod", o.signature_method.name().to_string()),
            ("realm", o.realm.clone()),
            ("addParamsToHeader", "true".to_string()),
        ]) }),
        Auth::AwsSigV4(a) => json!({ "type": "awsv4", "awsv4": pm_params(&[
            ("accessKey", a.access_key.clone()),
            ("secretKey", a.secret_key.clone()),
            ("sessionToken", a.session_token.clone()),
            ("region", a.region.clone()),
            ("service", a.service.clone()),
        ]) }),
        Auth::Jwt(j) => json!({ "type": "jwt", "jwt": pm_params(&[
            ("algorithm", j.algorithm.clone()),
            ("secret", j.secret.clone()),
            ("payload", j.payload.clone()),
            ("headerPrefix", j.header_prefix.clone()),
            ("addTokenTo", "header".to_string()),
        ]) }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
      "info": { "name": "Sample", "schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json" },
      "variable": [{ "key": "base", "value": "https://api.test" }],
      "auth": { "type": "bearer", "bearer": [{ "key": "token", "value": "{{token}}", "type": "string" }] },
      "item": [
        { "name": "Auth", "item": [
          { "name": "Login",
            "event": [{ "listen": "test", "script": { "exec": [
              "pm.test(\"ok\", function () {",
              "    pm.response.to.have.status(200);",
              "});",
              "var jsonData = pm.response.json();",
              "pm.environment.set(\"token\", jsonData.data.token);",
              "pm.collectionVariables.set('userId', pm.response.json().user.id);"
            ] } }],
            "request": {
              "method": "POST",
              "header": [{ "key": "Accept", "value": "application/json" }],
              "body": { "mode": "raw", "raw": "{\"user\":\"a\"}", "options": { "raw": { "language": "json" } } },
              "url": { "raw": "{{base}}/login?x=1", "host": ["{{base}}"], "path": ["login"], "query": [{ "key": "x", "value": "1" }] },
              "auth": { "type": "noauth" }
            } }
        ] },
        { "name": "Get user", "request": { "method": "GET", "url": "{{base}}/users/:id", "auth": { "type": "basic", "basic": { "username": "u", "password": "p" } } } },
        { "name": "Soap", "request": { "method": "POST",
            "header": [{ "key": "SOAPAction", "value": "\"urn:Add\"" }, { "key": "Content-Type", "value": "text/xml" }],
            "body": { "mode": "raw", "raw": "<soap:Envelope xmlns:soap=\"http://schemas.xmlsoap.org/soap/envelope/\"><soap:Body/></soap:Envelope>" },
            "url": "http://svc/calc.asmx" } }
      ]
    }"#;

    #[test]
    fn imports_folders_auth_scripts_and_soap() {
        let v: Value = serde_json::from_str(SAMPLE).unwrap();
        assert!(is_collection(&v));
        let c = import_collection(&v).unwrap();
        assert_eq!(c.name, "Sample");
        assert_eq!(c.variables[0].key, "base");
        assert_eq!(c.auth, Auth::Bearer { token: "{{token}}".into() });
        let Item::Folder(f) = &c.items[0] else { panic!() };
        let Item::Request(login) = &f.items[0] else { panic!() };
        assert_eq!(login.method, "POST");
        assert_eq!(login.url, "{{base}}/login");
        assert_eq!(login.params, vec![KeyValue::new("x", "1")]);
        assert_eq!(login.auth, Auth::None);
        assert_eq!(login.extract.len(), 2);
        assert_eq!(login.extract[0].expression, "$.data.token");
        assert_eq!(login.extract[0].scope, VarScope::Environment);
        assert_eq!(login.extract[1].expression, "$.user.id");
        assert_eq!(login.checks[0].expected, "200");
        assert!(login.description.is_empty(), "fully converted: {}", login.description);

        let Item::Request(get) = &c.items[1] else { panic!() };
        assert_eq!(get.auth, Auth::Basic { username: "u".into(), password: "p".into() });
        let Item::Request(soap) = &c.items[2] else { panic!() };
        assert_eq!(soap.protocol, Protocol::Soap);
        assert_eq!(soap.soap.action, "urn:Add");
        assert!(soap.headers.is_empty());
    }

    #[test]
    fn export_round_trips() {
        let v: Value = serde_json::from_str(SAMPLE).unwrap();
        let c = import_collection(&v).unwrap();
        let back = import_collection(&export_collection(&c)).unwrap();
        let Item::Folder(f) = &back.items[0] else { panic!() };
        let Item::Request(login) = &f.items[0] else { panic!() };
        assert_eq!(login.extract.len(), 2);
        assert_eq!(login.checks.len(), 1);
        assert_eq!(back.auth, c.auth);
        let Item::Request(soap) = &back.items[2] else { panic!() };
        assert_eq!(soap.protocol, Protocol::Soap);
        assert_eq!(soap.soap.action, "urn:Add");
    }

    #[test]
    fn environment() {
        let v: Value =
            serde_json::from_str(r#"{"name":"Dev","values":[{"key":"a","value":"1","enabled":true},{"key":"b","value":"2","enabled":false}]}"#).unwrap();
        assert!(is_environment(&v));
        let e = import_environment(&v).unwrap();
        assert_eq!(e.variables.len(), 2);
        assert!(!e.variables[1].enabled);
    }
}

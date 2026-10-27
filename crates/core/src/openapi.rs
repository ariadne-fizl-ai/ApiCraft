//! OpenAPI 3.x / Swagger 2.0 import, with generated test artifacts:
//!
//! - one request per operation, grouped in folders by tag, with example
//!   parameters and bodies built from the schemas;
//! - collection variables for the server URL and path parameters, and the
//!   security scheme mapped onto collection auth;
//! - checks on every request (documented success status, required fields);
//! - a smoke-test workflow over the safe GET operations, and a CRUD workflow
//!   per resource where "create" captures the new id for the steps after it.

use std::collections::HashSet;

use serde_json::{json, Map, Value};

use crate::model::*;

pub fn is_openapi(v: &Value) -> bool {
    v.get("openapi").and_then(|x| x.as_str()).is_some() || v.get("swagger").and_then(|x| x.as_str()).is_some()
}

/// Parses JSON or YAML into a JSON value. YAML keys are turned into strings
/// (OpenAPI files often write status codes as bare integers).
pub fn parse_document(text: &str) -> Result<Value, String> {
    if let Ok(v) = serde_json::from_str::<Value>(text) {
        return Ok(v);
    }
    let y: serde_yaml_ng::Value = serde_yaml_ng::from_str(text).map_err(|e| format!("not valid JSON or YAML: {e}"))?;
    Ok(yaml_to_json(y))
}

fn yaml_to_json(y: serde_yaml_ng::Value) -> Value {
    use serde_yaml_ng::Value as Y;
    match y {
        Y::Null => Value::Null,
        Y::Bool(b) => Value::Bool(b),
        Y::Number(n) => {
            if let Some(i) = n.as_i64() {
                json!(i)
            } else if let Some(u) = n.as_u64() {
                json!(u)
            } else {
                json!(n.as_f64().unwrap_or(0.0))
            }
        }
        Y::String(s) => Value::String(s),
        Y::Sequence(seq) => Value::Array(seq.into_iter().map(yaml_to_json).collect()),
        Y::Mapping(m) => {
            let mut out = Map::new();
            for (k, v) in m {
                let key = match k {
                    Y::String(s) => s,
                    Y::Number(n) => n.to_string(),
                    Y::Bool(b) => b.to_string(),
                    other => serde_yaml_ng::to_string(&other).unwrap_or_default().trim().to_string(),
                };
                out.insert(key, yaml_to_json(v));
            }
            Value::Object(out)
        }
        Y::Tagged(t) => yaml_to_json(t.value),
    }
}

/// What an OpenAPI import produces.
#[derive(Debug)]
pub struct OpenApiImport {
    pub collection: Collection,
    pub workflows: Vec<Workflow>,
    pub operations: usize,
}

struct Spec<'a> {
    root: &'a Value,
    v2: bool,
    /// Where the document was fetched from, for relative server URLs.
    source: Option<&'a str>,
}

impl<'a> Spec<'a> {
    /// Follows a local `$ref` (`#/components/schemas/Pet`, `#/definitions/Pet`, ...).
    fn deref(&self, v: &'a Value) -> &'a Value {
        let mut cur = v;
        for _ in 0..16 {
            match cur.get("$ref").and_then(|r| r.as_str()) {
                Some(r) => match self.pointer(r) {
                    Some(t) => cur = t,
                    None => return cur,
                },
                None => return cur,
            }
        }
        cur
    }

    fn pointer(&self, r: &str) -> Option<&'a Value> {
        let path = r.strip_prefix('#')?;
        let decoded: String = path.split('/').map(|p| p.replace("~1", "/").replace("~0", "~")).collect::<Vec<_>>().join("/");
        self.root.pointer(&decoded)
    }
}

fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string()
}

/// An example value for a standalone JSON Schema (e.g. an MCP tool's
/// `inputSchema`); local `$ref`s resolve within the schema itself.
pub fn example_for_schema(schema: &Value) -> Value {
    let spec = Spec { root: schema, v2: false, source: None };
    example(&spec, schema, 0, &mut HashSet::new())
}

/// An example value for a schema.
fn example(spec: &Spec, schema: &Value, depth: usize, seen: &mut HashSet<String>) -> Value {
    if depth > 6 {
        return Value::Null;
    }
    if let Some(r) = schema.get("$ref").and_then(|r| r.as_str()) {
        if !seen.insert(r.to_string()) {
            return json!({});
        }
        let out = example(spec, spec.deref(schema), depth + 1, seen);
        seen.remove(r);
        return out;
    }
    for key in ["example", "default"] {
        if let Some(v) = schema.get(key) {
            return v.clone();
        }
    }
    if let Some(v) = schema.get("examples").and_then(|e| e.as_array()).and_then(|a| a.first()) {
        return v.clone();
    }
    if let Some(v) = schema.get("const") {
        return v.clone();
    }
    if let Some(v) = schema.get("enum").and_then(|e| e.as_array()).and_then(|a| a.first()) {
        return v.clone();
    }
    if let Some(all) = schema.get("allOf").and_then(|a| a.as_array()) {
        let mut merged = Map::new();
        for part in all {
            if let Value::Object(o) = example(spec, part, depth + 1, seen) {
                merged.extend(o);
            }
        }
        return Value::Object(merged);
    }
    for key in ["oneOf", "anyOf"] {
        if let Some(first) = schema.get(key).and_then(|a| a.as_array()).and_then(|a| a.first()) {
            return example(spec, first, depth + 1, seen);
        }
    }
    let ty = match schema.get("type") {
        Some(Value::String(t)) => t.clone(),
        // 3.1 allows ["string", "null"]
        Some(Value::Array(a)) => a.iter().filter_map(|t| t.as_str()).find(|t| *t != "null").unwrap_or("string").to_string(),
        _ if schema.get("properties").is_some() => "object".into(),
        _ if schema.get("items").is_some() => "array".into(),
        _ => String::new(),
    };
    match ty.as_str() {
        "object" | "" => {
            let mut out = Map::new();
            if let Some(props) = schema.get("properties").and_then(|p| p.as_object()) {
                for (k, v) in props {
                    if v.get("readOnly").and_then(|r| r.as_bool()).unwrap_or(false) && depth == 0 {
                        continue;
                    }
                    out.insert(k.clone(), example(spec, v, depth + 1, seen));
                }
            } else if let Some(ap) = schema.get("additionalProperties").filter(|a| a.is_object()) {
                out.insert("key".into(), example(spec, ap, depth + 1, seen));
            }
            Value::Object(out)
        }
        "array" => {
            let item = schema.get("items").map(|i| example(spec, i, depth + 1, seen)).unwrap_or(Value::Null);
            Value::Array(vec![item])
        }
        "integer" => json!(schema.get("minimum").and_then(|m| m.as_i64()).unwrap_or(1)),
        "number" => json!(schema.get("minimum").and_then(|m| m.as_f64()).unwrap_or(1.5)),
        "boolean" => json!(true),
        "string" => json!(match s(schema, "format").as_str() {
            "date-time" => "2024-01-01T12:00:00Z",
            "date" => "2024-01-01",
            "time" => "12:00:00",
            "email" => "user@example.com",
            "uuid" => "3fa85f64-5717-4562-b3fc-2c963f66afa6",
            "uri" | "url" => "https://example.com",
            "hostname" => "example.com",
            "ipv4" => "192.0.2.1",
            "ipv6" => "2001:db8::1",
            "byte" => "ZXhhbXBsZQ==",
            "password" => "secret",
            _ => "string",
        }),
        _ => Value::Null,
    }
}

fn value_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        Value::Array(a) => a.iter().map(value_text).collect::<Vec<_>>().join(","),
        other => other.to_string(),
    }
}

fn param_example(spec: &Spec, p: &Value) -> String {
    if let Some(v) = p.get("example") {
        return value_text(v);
    }
    if let Some(v) = p.get("examples").and_then(|e| e.as_object()).and_then(|o| o.values().next()) {
        return value_text(spec.deref(v).get("value").unwrap_or(&Value::Null));
    }
    // 2.0 keeps type/format on the parameter itself.
    let schema = p.get("schema").cloned().unwrap_or_else(|| p.clone());
    value_text(&example(spec, &schema, 1, &mut HashSet::new()))
}

/// Server URL with its variables filled in.
fn base_url(spec: &Spec) -> String {
    if spec.v2 {
        let scheme = spec.root.get("schemes").and_then(|s| s.as_array()).and_then(|a| a.first()).and_then(|s| s.as_str()).unwrap_or("https");
        let host = s(spec.root, "host");
        let base = s(spec.root, "basePath");
        if host.is_empty() {
            let origin = spec
                .source
                .and_then(|s| crate::url::Url::parse(s).ok())
                .map(|u| format!("{}://{}", u.scheme, u.host_header()))
                .unwrap_or_else(|| "http://localhost".into());
            return format!("{origin}{}", base.trim_end_matches('/'));
        }
        return format!("{scheme}://{host}{}", base.trim_end_matches('/'));
    }
    let Some(server) = spec.root.get("servers").and_then(|s| s.as_array()).and_then(|a| a.first()) else {
        return "http://localhost".into();
    };
    let mut url = s(server, "url");
    if let Some(vars) = server.get("variables").and_then(|v| v.as_object()) {
        for (k, v) in vars {
            url = url.replace(&format!("{{{k}}}"), &s(v, "default"));
        }
    }
    if url.starts_with('/') {
        // Relative to wherever the definition came from.
        let origin = spec
            .source
            .and_then(|s| crate::url::Url::parse(s).ok())
            .map(|u| format!("{}://{}", u.scheme, u.host_header()))
            .unwrap_or_else(|| "http://localhost".into());
        url = format!("{origin}{url}");
    }
    url.trim_end_matches('/').to_string()
}

/// `{petId}` path segments become `{{petId}}`.
fn template_path(path: &str) -> String {
    let mut out = String::new();
    let mut rest = path;
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        match rest[i..].find('}') {
            Some(j) => {
                out.push_str("{{");
                out.push_str(&rest[i + 1..i + j]);
                out.push_str("}}");
                rest = &rest[i + j + 1..];
            }
            None => {
                out.push_str(&rest[i..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

fn path_params(path: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = path;
    while let Some(i) = rest.find('{') {
        let Some(j) = rest[i..].find('}') else { break };
        out.push(rest[i + 1..i + j].to_string());
        rest = &rest[i + j + 1..];
    }
    out
}

fn scheme_auth(scheme: &Value) -> Option<Auth> {
    match s(scheme, "type").as_str() {
        "http" => match s(scheme, "scheme").to_ascii_lowercase().as_str() {
            "basic" => Some(Auth::Basic { username: "{{username}}".into(), password: "{{password}}".into() }),
            "digest" => Some(Auth::Digest { username: "{{username}}".into(), password: "{{password}}".into() }),
            _ => Some(Auth::Bearer { token: "{{token}}".into() }),
        },
        "basic" => Some(Auth::Basic { username: "{{username}}".into(), password: "{{password}}".into() }),
        "apiKey" => {
            let location = s(scheme, "in");
            if location == "cookie" {
                return None;
            }
            Some(Auth::ApiKey { key: s(scheme, "name"), value: "{{apiKey}}".into(), in_header: location != "query" })
        }
        "oauth2" => {
            let mut o = OAuth2 { client_id: "{{clientId}}".into(), client_secret: "{{clientSecret}}".into(), ..OAuth2::default() };
            // 3.x: flows.{clientCredentials,password,authorizationCode,implicit}; 2.0: flow + urls.
            let flows = scheme.get("flows");
            let pick = |name: &str| flows.and_then(|f| f.get(name));
            let (grant, flow) = if let Some(f) = pick("clientCredentials") {
                (GrantType::ClientCredentials, f.clone())
            } else if let Some(f) = pick("authorizationCode") {
                (GrantType::AuthorizationCode, f.clone())
            } else if let Some(f) = pick("password") {
                (GrantType::Password, f.clone())
            } else if let Some(f) = pick("implicit") {
                (GrantType::Manual, f.clone())
            } else {
                let g = match s(scheme, "flow").as_str() {
                    "application" => GrantType::ClientCredentials,
                    "accessCode" => GrantType::AuthorizationCode,
                    "password" => GrantType::Password,
                    _ => GrantType::Manual,
                };
                (g, scheme.clone())
            };
            o.grant = grant;
            o.token_url = s(&flow, "tokenUrl");
            o.auth_url = s(&flow, "authorizationUrl");
            if let Some(scopes) = flow.get("scopes").and_then(|x| x.as_object()) {
                o.scope = scopes.keys().cloned().collect::<Vec<_>>().join(" ");
            }
            if grant == GrantType::Password {
                o.username = "{{username}}".into();
                o.password = "{{password}}".into();
            }
            Some(Auth::OAuth2(o))
        }
        "openIdConnect" => Some(Auth::OAuth2(OAuth2 { grant: GrantType::AuthorizationCode, ..OAuth2::default() })),
        _ => None,
    }
}

fn security_schemes<'a>(spec: &Spec<'a>) -> Option<&'a Map<String, Value>> {
    if spec.v2 {
        spec.root.get("securityDefinitions").and_then(|x| x.as_object())
    } else {
        spec.root.get("components").and_then(|c| c.get("securitySchemes")).and_then(|x| x.as_object())
    }
}

/// The auth for a `security` requirement list (first usable scheme).
fn auth_for(spec: &Spec, requirements: &Value) -> Option<Auth> {
    let schemes = security_schemes(spec)?;
    for req in requirements.as_array()? {
        for name in req.as_object()?.keys() {
            if let Some(a) = schemes.get(name).map(|sc| spec.deref(sc)).and_then(scheme_auth) {
                return Some(a);
            }
        }
    }
    None
}

/// The first documented 2xx code, the schema of its JSON body, and whether it's JSON.
fn success_response<'a>(spec: &Spec<'a>, op: &'a Value) -> (Option<String>, Option<&'a Value>) {
    let Some(responses) = op.get("responses").and_then(|r| r.as_object()) else { return (None, None) };
    let mut codes: Vec<&String> = responses.keys().filter(|k| k.starts_with('2')).collect();
    codes.sort();
    let Some(code) = codes.first() else { return (None, None) };
    let resp = spec.deref(&responses[*code]);
    let schema = if spec.v2 {
        resp.get("schema")
    } else {
        resp.get("content").and_then(|c| c.as_object()).and_then(|c| c.iter().find(|(k, _)| k.contains("json")).map(|(_, v)| v)).and_then(|m| m.get("schema"))
    };
    (Some(code.to_string()), schema)
}

fn checks_for(spec: &Spec, op: &Value) -> Vec<Check> {
    let (code, schema) = success_response(spec, op);
    let mut out = vec![Check { kind: CheckKind::Status, target: String::new(), expected: code.clone().unwrap_or_else(|| "2xx".into()), enabled: true }];
    if let Some(schema) = schema {
        let (_, required) = object_shape(spec, schema, 0);
        // Required fields first, then id-like ones, at most three checks.
        let mut fields = required.clone();
        fields.sort_by_key(|f| !f.to_ascii_lowercase().ends_with("id"));
        for field in fields.iter().take(3) {
            out.push(Check { kind: CheckKind::JsonEquals, target: format!("$.{field}"), expected: String::new(), enabled: true });
        }
    }
    out
}

/// Property names and required names of an object schema, following
/// `$ref` and merging `allOf` parts.
fn object_shape(spec: &Spec, schema: &Value, depth: usize) -> (Vec<String>, Vec<String>) {
    let schema = spec.deref(schema);
    let mut props: Vec<String> = schema.get("properties").and_then(|p| p.as_object()).map(|p| p.keys().cloned().collect()).unwrap_or_default();
    let mut required: Vec<String> =
        schema.get("required").and_then(|r| r.as_array()).map(|r| r.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default();
    if depth < 6 {
        for part in schema.get("allOf").and_then(|a| a.as_array()).into_iter().flatten() {
            let (p, r) = object_shape(spec, part, depth + 1);
            props.extend(p);
            required.extend(r);
        }
    }
    props.dedup();
    required.dedup();
    (props, required)
}

/// The id property a "create" response carries, if the schema shows one.
fn id_field(spec: &Spec, op: &Value, param: &str) -> String {
    let (_, schema) = success_response(spec, op);
    let props = schema.map(|sc| object_shape(spec, sc, 0).0).unwrap_or_default();
    if props.iter().any(|p| p == param) {
        param.to_string()
    } else if props.iter().any(|p| p == "id") || props.is_empty() {
        "id".into()
    } else {
        props.iter().find(|k| k.to_ascii_lowercase().ends_with("id")).cloned().unwrap_or_else(|| "id".into())
    }
}

struct OpInfo {
    request_id: String,
    method: String,
    path: String,
}

pub fn import(doc: &Value) -> Result<OpenApiImport, String> {
    import_from(doc, None)
}

/// Like [`import`]; `source` is the URL the document was fetched from, so
/// relative server URLs (`servers: [{url: /api/v3}]`) resolve against it.
pub fn import_from(doc: &Value, source: Option<&str>) -> Result<OpenApiImport, String> {
    if !is_openapi(doc) {
        return Err("not an OpenAPI or Swagger document".into());
    }
    let spec = Spec { root: doc, v2: doc.get("swagger").is_some(), source };
    let info = doc.get("info").cloned().unwrap_or(Value::Null);
    let title = match s(&info, "title") {
        t if t.is_empty() => "Imported API".to_string(),
        t => t,
    };
    let mut col = Collection::new(&title);
    col.description = s(&info, "description");
    col.variables.push(KeyValue::new("baseUrl", base_url(&spec)));

    // Collection-wide auth from the top-level security requirement.
    if let Some(a) = doc.get("security").and_then(|req| auth_for(&spec, req)) {
        col.auth = a;
    } else if let Some(schemes) = security_schemes(&spec) {
        if schemes.len() == 1 {
            if let Some(a) = schemes.values().next().map(|sc| spec.deref(sc)).and_then(scheme_auth) {
                col.auth = a;
            }
        }
    }
    add_auth_variables(&mut col);

    let paths = doc.get("paths").and_then(|p| p.as_object()).ok_or("the document has no paths")?;
    let mut folders: Vec<Folder> = Vec::new();
    let mut loose: Vec<Item> = Vec::new();
    let mut ops: Vec<OpInfo> = Vec::new();
    let mut path_vars: Vec<(String, String)> = Vec::new();
    let consumes_default = doc.get("consumes").cloned();

    for (path, item) in paths {
        let item = spec.deref(item);
        let shared_params = item.get("parameters").and_then(|p| p.as_array()).cloned().unwrap_or_default();
        for method in ["get", "post", "put", "patch", "delete", "head", "options"] {
            let Some(op) = item.get(method) else { continue };
            let name = {
                let summary = s(op, "summary");
                if !summary.is_empty() {
                    summary
                } else if !s(op, "operationId").is_empty() {
                    s(op, "operationId")
                } else {
                    format!("{} {path}", method.to_uppercase())
                }
            };
            let mut r = Request::new(name, Protocol::Rest);
            r.method = method.to_uppercase();
            r.url = format!("{{{{baseUrl}}}}{}", template_path(path));
            r.description =
                [s(op, "description"), format!("{} {}", method.to_uppercase(), path)].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join("\n\n");

            // parameters: path-level first, operation-level overrides
            let mut params: Vec<&Value> = shared_params.iter().map(|p| spec.deref(p)).collect();
            if let Some(list) = op.get("parameters").and_then(|p| p.as_array()) {
                for p in list.iter().map(|p| spec.deref(p)) {
                    params.retain(|q| !(s(q, "name") == s(p, "name") && s(q, "in") == s(p, "in")));
                    params.push(p);
                }
            }
            let mut form_fields = Vec::new();
            let mut body_schema: Option<Value> = None;
            for p in &params {
                let pname = s(p, "name");
                let required = p.get("required").and_then(|x| x.as_bool()).unwrap_or(false);
                match s(p, "in").as_str() {
                    "query" => r.params.push(KeyValue { key: pname, value: param_example(&spec, p), enabled: required }),
                    "header" => r.headers.push(KeyValue { key: pname, value: param_example(&spec, p), enabled: required }),
                    "path" if !path_vars.iter().any(|(k, _)| *k == pname) => path_vars.push((pname.clone(), param_example(&spec, p))),
                    "formData" => form_fields.push(KeyValue { key: pname, value: param_example(&spec, p), enabled: true }),
                    "body" => body_schema = p.get("schema").cloned(),
                    _ => {}
                }
            }

            // body
            if spec.v2 {
                let consumes = op
                    .get("consumes")
                    .or(consumes_default.as_ref())
                    .and_then(|c| c.as_array())
                    .and_then(|a| a.first())
                    .and_then(|c| c.as_str())
                    .unwrap_or("application/json")
                    .to_string();
                if let Some(sc) = body_schema {
                    let ex = example(&spec, &sc, 0, &mut HashSet::new());
                    r.body = Body::Raw { content_type: consumes, text: serde_json::to_string_pretty(&ex).unwrap_or_default() };
                } else if !form_fields.is_empty() {
                    r.body = if consumes.contains("multipart") { Body::FormData { fields: form_fields } } else { Body::UrlEncoded { fields: form_fields } };
                }
            } else if let Some(rb) = op.get("requestBody").map(|b| spec.deref(b)) {
                if let Some(content) = rb.get("content").and_then(|c| c.as_object()) {
                    let (ct, media) =
                        content.iter().find(|(k, _)| k.contains("json")).or_else(|| content.iter().next()).map(|(k, v)| (k.clone(), v)).unwrap_or_default();
                    let ex = media
                        .get("example")
                        .cloned()
                        .or_else(|| {
                            media
                                .get("examples")
                                .and_then(|e| e.as_object())
                                .and_then(|o| o.values().next())
                                .map(|e| spec.deref(e).get("value").cloned().unwrap_or(Value::Null))
                        })
                        .unwrap_or_else(|| media.get("schema").map(|sc| example(&spec, sc, 0, &mut HashSet::new())).unwrap_or(json!({})));
                    r.body = if ct.contains("x-www-form-urlencoded") || ct.contains("multipart") {
                        let fields = ex.as_object().map(|o| o.iter().map(|(k, v)| KeyValue::new(k, value_text(v))).collect()).unwrap_or_default();
                        if ct.contains("multipart") {
                            Body::FormData { fields }
                        } else {
                            Body::UrlEncoded { fields }
                        }
                    } else if ct.contains("json") {
                        Body::Raw { content_type: ct, text: serde_json::to_string_pretty(&ex).unwrap_or_default() }
                    } else {
                        Body::Raw { content_type: ct, text: value_text(&ex) }
                    };
                }
            }

            // per-operation security
            if let Some(sec) = op.get("security") {
                if sec.as_array().map(|a| a.is_empty()).unwrap_or(false) {
                    r.auth = Auth::None;
                } else if let Some(a) = auth_for(&spec, sec) {
                    if a != col.auth {
                        r.auth = a;
                    }
                }
            }

            r.checks = checks_for(&spec, op);
            ops.push(OpInfo { request_id: r.id.clone(), method: method.to_string(), path: path.clone() });

            let tag = op.get("tags").and_then(|t| t.as_array()).and_then(|a| a.first()).and_then(|t| t.as_str()).map(str::to_string);
            match tag {
                Some(t) => match folders.iter_mut().find(|f| f.name == t) {
                    Some(f) => f.items.push(Item::Request(r)),
                    None => {
                        let mut f = Folder::new(t);
                        f.items.push(Item::Request(r));
                        folders.push(f);
                    }
                },
                None => loose.push(Item::Request(r)),
            }
        }
    }
    for (k, v) in &path_vars {
        let value = if v.is_empty() || v == "string" { "1".to_string() } else { v.clone() };
        col.variables.push(KeyValue::new(k, value));
    }
    col.items = folders.into_iter().map(Item::Folder).chain(loose).collect();
    let operations = ops.len();

    // Captures for "create" operations, then the workflows.
    let mut workflows = Vec::new();
    let mut smoke = Workflow::new(format!("{title}: smoke test"));
    smoke.stop_on_failure = false;
    for op in &ops {
        if op.method == "get" && path_params(&op.path).is_empty() {
            smoke.steps.push(WorkflowStep::new(op.request_id.clone()));
        }
    }
    if !smoke.steps.is_empty() {
        workflows.push(smoke);
    }

    // CRUD: POST /things  +  GET|PUT|PATCH|DELETE /things/{thingId}
    for create in ops.iter().filter(|o| o.method == "post" && path_params(&o.path).is_empty()) {
        let base = create.path.trim_end_matches('/');
        let item_ops: Vec<&OpInfo> = ops
            .iter()
            .filter(|o| {
                let params = path_params(&o.path);
                params.len() == 1 && o.path.starts_with(base) && o.path[base.len()..].trim_start_matches('/') == format!("{{{}}}", params[0])
            })
            .collect();
        if item_ops.is_empty() {
            continue;
        }
        let param = path_params(&item_ops[0].path).remove(0);
        let create_op = doc.pointer(&format!("/paths/{}/post", base.replace('~', "~0").replace('/', "~1"))).unwrap_or(&Value::Null);
        let field = id_field(&spec, create_op, &param);
        if let Some(r) = find_request_in_mut(&mut col.items, &create.request_id) {
            r.extract.push(Extraction {
                variable: param.clone(),
                source: ExtractSource::Json,
                expression: format!("$.{field}"),
                scope: VarScope::Runtime,
                enabled: true,
            });
        }
        let resource = base.rsplit('/').next().unwrap_or(base);
        let mut wf = Workflow::new(format!("{title}: {resource} CRUD"));
        wf.steps.push(WorkflowStep::new(create.request_id.clone()));
        for m in ["get", "put", "patch", "delete"] {
            if let Some(o) = item_ops.iter().find(|o| o.method == m) {
                wf.steps.push(WorkflowStep::new(o.request_id.clone()));
            }
        }
        workflows.push(wf);
    }

    Ok(OpenApiImport { collection: col, workflows, operations })
}

fn find_request_in_mut<'a>(items: &'a mut [Item], id: &str) -> Option<&'a mut Request> {
    items.iter_mut().find_map(|i| match i {
        Item::Request(r) if r.id == id => Some(r),
        Item::Folder(f) => find_request_in_mut(&mut f.items, id),
        _ => None,
    })
}

/// Placeholders the generated auth refers to become collection variables.
fn add_auth_variables(col: &mut Collection) {
    let names: &[&str] = match &col.auth {
        Auth::Basic { .. } | Auth::Digest { .. } => &["username", "password"],
        Auth::Bearer { .. } => &["token"],
        Auth::ApiKey { .. } => &["apiKey"],
        Auth::OAuth2(o) if o.grant == GrantType::Password => &["clientId", "clientSecret", "username", "password"],
        Auth::OAuth2(o) if o.client_id.starts_with("{{") => &["clientId", "clientSecret"],
        _ => &[],
    };
    for n in names {
        col.variables.push(KeyValue::new(*n, ""));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PETSTORE_V3: &str = r#"
openapi: 3.0.3
info:
  title: Petstore
servers:
  - url: https://{env}.example.com/v1
    variables:
      env:
        default: api
security:
  - apiKeyAuth: []
components:
  securitySchemes:
    apiKeyAuth:
      type: apiKey
      in: header
      name: X-API-Key
  schemas:
    NewPet:
      type: object
      required: [name]
      properties:
        name: { type: string, example: Rex }
        tag: { type: string }
    Pet:
      allOf:
        - $ref: '#/components/schemas/NewPet'
        - type: object
          required: [id]
          properties:
            id: { type: integer, format: int64 }
paths:
  /pets:
    get:
      tags: [pets]
      summary: List pets
      parameters:
        - name: limit
          in: query
          schema: { type: integer, minimum: 5 }
      responses:
        200:
          description: ok
          content:
            application/json:
              schema:
                type: array
                items: { $ref: '#/components/schemas/Pet' }
    post:
      tags: [pets]
      summary: Create a pet
      requestBody:
        content:
          application/json:
            schema: { $ref: '#/components/schemas/NewPet' }
      responses:
        '201':
          description: created
          content:
            application/json:
              schema: { $ref: '#/components/schemas/Pet' }
  /pets/{petId}:
    parameters:
      - name: petId
        in: path
        required: true
        schema: { type: integer, example: 7 }
    get:
      tags: [pets]
      summary: Get a pet
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema: { $ref: '#/components/schemas/Pet' }
    delete:
      tags: [pets]
      summary: Delete a pet
      security: []
      responses:
        '204': { description: gone }
"#;

    #[test]
    fn imports_openapi3_yaml_with_tests() {
        let doc = parse_document(PETSTORE_V3).unwrap();
        assert!(is_openapi(&doc));
        let imp = import(&doc).unwrap();
        let c = &imp.collection;
        assert_eq!(c.name, "Petstore");
        assert_eq!(imp.operations, 4);
        assert!(c.variables.contains(&KeyValue::new("baseUrl", "https://api.example.com/v1")));
        assert!(c.variables.contains(&KeyValue::new("petId", "7")));
        assert_eq!(c.auth, Auth::ApiKey { key: "X-API-Key".into(), value: "{{apiKey}}".into(), in_header: true });
        let Item::Folder(f) = &c.items[0] else { panic!() };
        assert_eq!(f.name, "pets");
        let reqs: Vec<&Request> = f.items.iter().filter_map(|i| if let Item::Request(r) = i { Some(r) } else { None }).collect();
        let list = reqs.iter().find(|r| r.name == "List pets").unwrap();
        assert_eq!(list.url, "{{baseUrl}}/pets");
        assert_eq!(list.params[0], KeyValue { key: "limit".into(), value: "5".into(), enabled: false });
        assert_eq!(list.checks[0].expected, "200");
        let create = reqs.iter().find(|r| r.name == "Create a pet").unwrap();
        assert!(create.body.raw_text().contains("\"name\": \"Rex\""));
        assert_eq!(create.checks[0].expected, "201");
        assert!(create.checks.iter().any(|ch| ch.target == "$.id"));
        assert_eq!(create.extract[0].variable, "petId");
        assert_eq!(create.extract[0].expression, "$.id");
        let get = reqs.iter().find(|r| r.name == "Get a pet").unwrap();
        assert_eq!(get.url, "{{baseUrl}}/pets/{{petId}}");
        let del = reqs.iter().find(|r| r.name == "Delete a pet").unwrap();
        assert_eq!(del.auth, Auth::None);

        assert_eq!(imp.workflows.len(), 2);
        assert_eq!(imp.workflows[0].steps.len(), 1, "smoke test: GET /pets only");
        let crud = &imp.workflows[1];
        assert_eq!(crud.steps.len(), 3, "create, get, delete");
        assert_eq!(crud.steps[0].request_id, create.id);
    }

    #[test]
    fn imports_swagger2_json() {
        let doc: Value = serde_json::from_str(
            r#"{"swagger":"2.0","info":{"title":"Legacy"},"host":"legacy.test","basePath":"/api","schemes":["https"],
               "securityDefinitions":{"basic":{"type":"basic"}},
               "paths":{"/login":{"post":{"consumes":["application/x-www-form-urlencoded"],
                 "parameters":[{"name":"user","in":"formData","type":"string"},{"name":"pass","in":"formData","type":"string","format":"password"}],
                 "responses":{"200":{"description":"ok","schema":{"type":"object","required":["token"],"properties":{"token":{"type":"string"}}}}}}}}}"#,
        )
        .unwrap();
        let imp = import(&doc).unwrap();
        let c = imp.collection;
        assert!(c.variables.contains(&KeyValue::new("baseUrl", "https://legacy.test/api")));
        assert!(matches!(c.auth, Auth::Basic { .. }));
        let Item::Request(r) = &c.items[0] else { panic!() };
        assert_eq!(r.body, Body::UrlEncoded { fields: vec![KeyValue::new("user", "string"), KeyValue::new("pass", "secret")] });
        assert!(r.checks.iter().any(|ch| ch.target == "$.token"));
    }
}

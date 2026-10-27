//! Turns a saved [`Request`] into the concrete call: variables substituted,
//! query parameters and auth applied, protocol headers added. Sending and
//! code generation both start from a [`Prepared`] request, so a copied curl
//! command does exactly what the Send button does.

use crate::model::{Body, KeyValue, ODataOptions, Protocol, Request, SoapVersion};
use crate::vars::Vars;
use crate::{graphql, odata, soap};

#[derive(Debug, Clone, PartialEq)]
pub enum PreparedBody {
    None,
    Text(String),
    /// multipart/form-data text fields; the transport picks the boundary.
    Multipart(Vec<(String, String)>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct GrpcTarget {
    /// `scheme://host:port` with no path.
    pub base: String,
    pub service: String,
    pub method: String,
    pub proto: String,
}

impl GrpcTarget {
    pub fn path(&self) -> String {
        format!("/{}/{}", self.service, self.method)
    }
    pub fn host_port(&self) -> String {
        let without_scheme = self.base.split_once("://").map(|(_, r)| r).unwrap_or(&self.base);
        let host = without_scheme.trim_end_matches('/');
        if host.contains(':') {
            host.to_string()
        } else if self.is_tls() {
            format!("{host}:443")
        } else {
            format!("{host}:80")
        }
    }
    pub fn is_tls(&self) -> bool {
        self.base.starts_with("https://") || self.base.starts_with("grpcs://")
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Prepared {
    pub protocol: Protocol,
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: PreparedBody,
    /// Set for gRPC; the body then holds the request message as JSON.
    pub grpc: Option<GrpcTarget>,
    pub subprotocols: Vec<String>,
    pub ws_messages: Vec<String>,
    /// `{{names}}` that had no value.
    pub missing_vars: Vec<String>,
}

impl Prepared {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
    pub fn content_type(&self) -> Option<&str> {
        self.header("content-type")
    }
}

pub fn prepare(req: &Request, vars: &Vars) -> Prepared {
    let mut missing = Vec::new();
    let mut sub = |s: &str| {
        for m in vars.missing(s) {
            if !missing.contains(&m) {
                missing.push(m);
            }
        }
        vars.apply(s)
    };

    let mut url = sub(req.url.trim());
    let query: Vec<(String, String)> = enabled(&req.params).map(|kv| (sub(&kv.key), sub(&kv.value))).collect();
    let mut headers: Vec<(String, String)> = enabled(&req.headers).map(|kv| (sub(&kv.key), sub(&kv.value))).collect();

    let mut body = match &req.body {
        Body::None => PreparedBody::None,
        Body::Raw { text, .. } => PreparedBody::Text(sub(text)),
        Body::UrlEncoded { fields } => {
            let pairs: Vec<(String, String)> = enabled(fields).map(|kv| (sub(&kv.key), sub(&kv.value))).collect();
            PreparedBody::Text(form_urlencode(&pairs))
        }
        Body::FormData { fields } => PreparedBody::Multipart(enabled(fields).map(|kv| (sub(&kv.key), sub(&kv.value))).collect()),
    };

    let raw_ct = match &req.body {
        Body::Raw { content_type, .. } => sub(content_type),
        _ => String::new(),
    };

    let mut grpc = None;
    let mut method = req.method.trim().to_uppercase();
    if method.is_empty() {
        method = "GET".into();
    }

    match req.protocol {
        Protocol::Rest => match &req.body {
            Body::Raw { text, .. } => {
                let ct = if !raw_ct.is_empty() {
                    raw_ct
                } else if serde_json::from_str::<serde_json::Value>(text).is_ok() {
                    "application/json".into()
                } else if text.trim_start().starts_with('<') {
                    "application/xml".into()
                } else {
                    "text/plain".into()
                };
                set_default(&mut headers, "Content-Type", &ct);
            }
            Body::UrlEncoded { .. } => set_default(&mut headers, "Content-Type", "application/x-www-form-urlencoded"),
            _ => {}
        },
        Protocol::Soap => {
            method = "POST".into();
            let action = sub(&req.soap.action);
            let text = match &body {
                PreparedBody::Text(t) => t.clone(),
                _ => String::new(),
            };
            body = PreparedBody::Text(soap::ensure_envelope(&text, req.soap.version));
            match req.soap.version {
                SoapVersion::V11 => {
                    set_default(&mut headers, "Content-Type", "text/xml; charset=utf-8");
                    set_default(&mut headers, "SOAPAction", &format!("\"{action}\""));
                }
                SoapVersion::V12 => {
                    let ct = if action.is_empty() {
                        "application/soap+xml; charset=utf-8".to_string()
                    } else {
                        format!("application/soap+xml; charset=utf-8; action=\"{action}\"")
                    };
                    set_default(&mut headers, "Content-Type", &ct);
                }
            }
        }
        Protocol::Grpc => {
            method = "POST".into();
            let base = url.trim_end_matches('/').to_string();
            let target = GrpcTarget {
                base: base.clone(),
                service: req.grpc.service.trim().to_string(),
                method: req.grpc.method.trim().to_string(),
                proto: req.grpc.proto.clone(),
            };
            url = format!("{base}{}", target.path());
            if matches!(body, PreparedBody::None) {
                body = PreparedBody::Text("{}".into());
            }
            grpc = Some(target);
        }
        Protocol::WebSocket => {
            method = "GET".into();
            body = PreparedBody::None;
        }
        Protocol::Mcp => method = "POST".into(),
        Protocol::GraphQl => {
            let g = &req.graphql;
            let (query, variables, op) = (sub(&g.query), sub(&g.variables), sub(&g.operation_name));
            if method == "GET" {
                // A bad variables object is sent as written; the server will say why.
                let params = graphql::get_params(&query, &variables, &op)
                    .unwrap_or_else(|_| vec![("query".into(), query.clone()), ("variables".into(), variables.clone())]);
                url = append_query(&url, &params);
                body = PreparedBody::None;
            } else {
                method = "POST".into();
                let text = graphql::request_body(&query, &variables, &op)
                    .unwrap_or_else(|_| format!("{{\"query\":{},\"variables\":{}}}", serde_json::Value::String(query.clone()), variables.trim()));
                body = PreparedBody::Text(text);
                set_default(&mut headers, "Content-Type", "application/json");
            }
            set_default(&mut headers, "Accept", "application/graphql-response+json, application/json");
        }
        Protocol::OData => {
            let o = &req.odata;
            let opts = ODataOptions {
                resource: sub(&o.resource),
                select: sub(&o.select),
                expand: sub(&o.expand),
                filter: sub(&o.filter),
                orderby: sub(&o.orderby),
                top: sub(&o.top),
                skip: sub(&o.skip),
                search: sub(&o.search),
                ..o.clone()
            };
            url = odata::resource_url(&url, &opts.resource);
            let qs = odata::encode_query(&odata::query_options(&opts));
            if !qs.is_empty() {
                url = format!("{url}{}{qs}", if url.contains('?') { "&" } else { "?" });
            }
            for (k, v) in odata::headers(o.version) {
                set_default(&mut headers, k, v);
            }
            if matches!(method.as_str(), "GET" | "HEAD" | "DELETE") {
                body = PreparedBody::None;
            } else if matches!(req.body, Body::Raw { .. }) {
                set_default(&mut headers, "Content-Type", if raw_ct.is_empty() { "application/json" } else { &raw_ct });
            } else if matches!(req.body, Body::UrlEncoded { .. }) {
                set_default(&mut headers, "Content-Type", "application/x-www-form-urlencoded");
            }
        }
    }

    url = append_query(&url, &query);

    let ws_messages = if req.protocol == Protocol::WebSocket { req.websocket.messages.iter().map(|m| sub(m)).collect() } else { Vec::new() };
    let subprotocols = req.websocket.subprotocols.iter().map(|s| sub(s)).filter(|s| !s.is_empty()).collect();

    Prepared { protocol: req.protocol, method, url, headers, body, grpc, subprotocols, ws_messages, missing_vars: missing }
}

fn enabled(kvs: &[KeyValue]) -> impl Iterator<Item = &KeyValue> {
    kvs.iter().filter(|kv| kv.enabled && !kv.key.trim().is_empty())
}

pub fn append_query(url: &str, query: &[(String, String)]) -> String {
    if query.is_empty() {
        return url.to_string();
    }
    let qs = form_urlencode(query);
    match url.find('#') {
        Some(hash) => {
            let (head, frag) = url.split_at(hash);
            format!("{head}{}{qs}{frag}", if head.contains('?') { "&" } else { "?" })
        }
        None => format!("{url}{}{qs}", if url.contains('?') { "&" } else { "?" }),
    }
}

/// Adds a header unless the user already set one with that name.
pub fn set_default(headers: &mut Vec<(String, String)>, name: &str, value: &str) {
    if !headers.iter().any(|(k, _)| k.eq_ignore_ascii_case(name)) {
        headers.push((name.to_string(), value.to_string()));
    }
}

pub fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

pub fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => match std::str::from_utf8(&bytes[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()).ok_or(()) {
                Ok(v) => {
                    out.push(v);
                    i += 2;
                }
                Err(_) => out.push(b'%'),
            },
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn form_urlencode(pairs: &[(String, String)]) -> String {
    pairs.iter().map(|(k, v)| format!("{}={}", percent_encode(k), percent_encode(v))).collect::<Vec<_>>().join("&")
}

/// Builds a multipart/form-data body. Returns `(content_type, bytes)`.
pub fn multipart(fields: &[(String, String)], boundary: &str) -> (String, Vec<u8>) {
    let mut out = Vec::new();
    for (k, v) in fields {
        out.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{}\"\r\n\r\n", k.replace('"', "%22")).as_bytes());
        out.extend_from_slice(v.as_bytes());
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::*;

    #[test]
    fn rest_with_params_auth_and_json() {
        let mut r = Request::new("x", Protocol::Rest);
        r.method = "post".into();
        r.url = "https://{{host}}/items?a=1".into();
        r.params.push(KeyValue::new("q", "a b"));
        r.body = Body::Raw { content_type: String::new(), text: "{\"x\":1}".into() };
        let mut v = Vars::default();
        v.set("host", "api.test");
        let p = prepare(&r, &v);
        assert_eq!(p.method, "POST");
        assert_eq!(p.url, "https://api.test/items?a=1&q=a%20b");
        assert_eq!(p.content_type(), Some("application/json"));
        assert!(p.missing_vars.is_empty());
    }

    #[test]
    fn soap_headers() {
        let mut r = Request::new("s", Protocol::Soap);
        r.url = "http://svc/x.asmx".into();
        r.soap.action = "urn:Add".into();
        let p = prepare(&r, &Vars::default());
        assert_eq!(p.header("SOAPAction"), Some("\"urn:Add\""));
        assert!(p.content_type().unwrap().starts_with("text/xml"));
    }

    #[test]
    fn grpc_target() {
        let mut r = Request::new("g", Protocol::Grpc);
        r.url = "http://localhost:50051/".into();
        r.grpc.service = "helloworld.Greeter".into();
        r.grpc.method = "SayHello".into();
        let p = prepare(&r, &Vars::default());
        assert_eq!(p.url, "http://localhost:50051/helloworld.Greeter/SayHello");
        let t = p.grpc.unwrap();
        assert_eq!(t.host_port(), "localhost:50051");
        assert!(!t.is_tls());
    }

    #[test]
    fn graphql_post_and_get() {
        let mut r = Request::new("g", Protocol::GraphQl);
        r.url = "https://api.test/graphql".into();
        r.graphql.query = "query Q($id: ID!) { user(id: $id) { name } }".into();
        r.graphql.variables = "{\"id\": \"{{uid}}\"}".into();
        let mut v = Vars::default();
        v.set("uid", "42");
        let p = prepare(&r, &v);
        assert_eq!(p.method, "POST");
        assert_eq!(p.content_type(), Some("application/json"));
        let PreparedBody::Text(body) = &p.body else { panic!() };
        let body: serde_json::Value = serde_json::from_str(body).unwrap();
        assert_eq!(body["variables"]["id"], "42");
        r.method = "GET".into();
        let p = prepare(&r, &v);
        assert_eq!(p.body, PreparedBody::None);
        assert!(p.url.starts_with("https://api.test/graphql?query=query%20Q"));
        assert!(p.url.ends_with("&variables=%7B%22id%22%3A%2242%22%7D"));
    }

    #[test]
    fn odata_url_and_headers() {
        let mut r = Request::new("o", Protocol::OData);
        r.url = "https://h/svc/".into();
        r.odata.resource = "People('{{who}}')".into();
        r.odata.select = "FirstName,LastName".into();
        r.odata.expand = "Friends".into();
        r.params.push(KeyValue::new("sap-client", "100"));
        let mut v = Vars::default();
        v.set("who", "russell");
        let p = prepare(&r, &v);
        assert_eq!(p.url, "https://h/svc/People('russell')?$select=FirstName,LastName&$expand=Friends&sap-client=100");
        assert_eq!(p.header("OData-Version"), Some("4.0"));
        assert_eq!(p.header("Accept"), Some("application/json"));
        r.odata.version = ODataVersion::V2;
        r.method = "POST".into();
        r.body = Body::Raw { content_type: String::new(), text: "{}".into() };
        let p = prepare(&r, &v);
        assert_eq!(p.header("DataServiceVersion"), Some("2.0"));
        assert_eq!(p.content_type(), Some("application/json"));
    }

    #[test]
    fn decode_roundtrip() {
        assert_eq!(percent_decode(&percent_encode("a b&c=d/é")), "a b&c=d/é");
        assert_eq!(percent_decode("a+b%2"), "a b%2");
    }
}

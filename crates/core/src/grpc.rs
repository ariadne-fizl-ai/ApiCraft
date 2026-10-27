//! gRPC without generated code: the `.proto` is compiled at runtime, request
//! JSON becomes a dynamic protobuf message, and responses come back as JSON.
//! The same framing serves native gRPC (via the agent) and gRPC-Web.

use base64::Engine;
use prost::Message;
use prost_reflect::{DescriptorPool, DynamicMessage, Kind, MessageDescriptor, MethodDescriptor};
use protox::file::{ChainFileResolver, File, FileResolver, GoogleFileResolver};

const MAIN_FILE: &str = "request.proto";

struct SourceResolver(String);

impl FileResolver for SourceResolver {
    fn open_file(&self, name: &str) -> Result<File, protox::Error> {
        if name == MAIN_FILE {
            File::from_source(name, &self.0)
        } else {
            Err(protox::Error::file_not_found(name))
        }
    }
}

/// Compiles proto source (well-known `google/protobuf/*` imports work).
pub fn compile(source: &str) -> Result<DescriptorPool, String> {
    if source.trim().is_empty() {
        return Err("paste the service's .proto definition first".into());
    }
    let mut chain = ChainFileResolver::new();
    chain.add(SourceResolver(source.to_string()));
    chain.add(GoogleFileResolver::new());
    let mut compiler = protox::Compiler::with_file_resolver(chain);
    compiler.include_imports(true);
    compiler.open_file(MAIN_FILE).map_err(|e| format!("proto error: {e}"))?;
    Ok(compiler.descriptor_pool())
}

#[derive(Debug, Clone, PartialEq)]
pub struct MethodInfo {
    pub name: String,
    pub input: String,
    pub output: String,
    pub client_streaming: bool,
    pub server_streaming: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ServiceInfo {
    pub name: String,
    pub methods: Vec<MethodInfo>,
}

pub fn services(pool: &DescriptorPool) -> Vec<ServiceInfo> {
    pool.services()
        .filter(|s| !s.full_name().starts_with("google.protobuf"))
        .map(|s| ServiceInfo {
            name: s.full_name().to_string(),
            methods: s
                .methods()
                .map(|m| MethodInfo {
                    name: m.name().to_string(),
                    input: m.input().full_name().to_string(),
                    output: m.output().full_name().to_string(),
                    client_streaming: m.is_client_streaming(),
                    server_streaming: m.is_server_streaming(),
                })
                .collect(),
        })
        .collect()
}

pub fn method(pool: &DescriptorPool, service: &str, method: &str) -> Result<MethodDescriptor, String> {
    let svc = pool.get_service_by_name(service).ok_or_else(|| format!("service {service} is not in the proto"))?;
    let m = svc.methods().find(|m| m.name() == method).ok_or_else(|| format!("method {method} is not in {service}"))?;
    if m.is_client_streaming() {
        return Err(format!("{method} is client-streaming; only unary and server-streaming calls are supported"));
    }
    Ok(m)
}

/// JSON (proto3 JSON mapping) to protobuf bytes.
pub fn encode_json(desc: &MessageDescriptor, json: &str) -> Result<Vec<u8>, String> {
    let json = if json.trim().is_empty() { "{}" } else { json };
    let mut de = serde_json::Deserializer::from_str(json);
    let msg = DynamicMessage::deserialize(desc.clone(), &mut de).map_err(|e| format!("request does not match {}: {e}", desc.full_name()))?;
    Ok(msg.encode_to_vec())
}

pub fn decode_to_json(desc: &MessageDescriptor, bytes: &[u8]) -> Result<String, String> {
    let msg = DynamicMessage::decode(desc.clone(), bytes).map_err(|e| format!("could not decode {}: {e}", desc.full_name()))?;
    serde_json::to_string_pretty(&msg).map_err(|e| e.to_string())
}

/// Length-prefixed message frame (flag 0 = uncompressed data).
pub fn frame(msg: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(msg.len() + 5);
    out.push(0);
    out.extend_from_slice(&(msg.len() as u32).to_be_bytes());
    out.extend_from_slice(msg);
    out
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct GrpcResult {
    /// Each response message as JSON.
    pub messages: Vec<String>,
    /// `grpc-status`, if the server sent one.
    pub status: Option<i32>,
    pub message: String,
    pub trailers: Vec<(String, String)>,
}

impl GrpcResult {
    pub fn ok(&self) -> bool {
        self.status.unwrap_or(0) == 0
    }
    /// One message as-is, several as a JSON array.
    pub fn body_json(&self) -> String {
        match self.messages.len() {
            0 => "{}".into(),
            1 => self.messages[0].clone(),
            _ => format!("[\n{}\n]", self.messages.join(",\n")),
        }
    }
    pub fn status_name(&self) -> &'static str {
        status_name(self.status.unwrap_or(0))
    }
}

pub fn status_name(code: i32) -> &'static str {
    match code {
        0 => "OK",
        1 => "CANCELLED",
        2 => "UNKNOWN",
        3 => "INVALID_ARGUMENT",
        4 => "DEADLINE_EXCEEDED",
        5 => "NOT_FOUND",
        6 => "ALREADY_EXISTS",
        7 => "PERMISSION_DENIED",
        8 => "RESOURCE_EXHAUSTED",
        9 => "FAILED_PRECONDITION",
        10 => "ABORTED",
        11 => "OUT_OF_RANGE",
        12 => "UNIMPLEMENTED",
        13 => "INTERNAL",
        14 => "UNAVAILABLE",
        15 => "DATA_LOSS",
        16 => "UNAUTHENTICATED",
        _ => "UNKNOWN_CODE",
    }
}

/// Parses a gRPC or gRPC-Web response body. Trailers can come from HTTP/2
/// trailers, headers (trailers-only responses) or a gRPC-Web trailer frame.
pub fn decode_response(
    output: &MessageDescriptor,
    content_type: &str,
    body: &[u8],
    headers: &[(String, String)],
    trailers: &[(String, String)],
) -> Result<GrpcResult, String> {
    let decoded;
    let body = if content_type.contains("grpc-web-text") {
        decoded = base64::engine::general_purpose::STANDARD
            .decode(body.iter().copied().filter(|b| !b.is_ascii_whitespace()).collect::<Vec<u8>>())
            .map_err(|e| format!("bad grpc-web-text body: {e}"))?;
        &decoded[..]
    } else {
        body
    };
    let mut result = GrpcResult::default();
    let mut all_trailers: Vec<(String, String)> = trailers.to_vec();
    let mut i = 0;
    while i + 5 <= body.len() {
        let flag = body[i];
        let len = u32::from_be_bytes([body[i + 1], body[i + 2], body[i + 3], body[i + 4]]) as usize;
        let start = i + 5;
        let end = start.checked_add(len).filter(|e| *e <= body.len()).ok_or("truncated gRPC frame")?;
        let payload = &body[start..end];
        if flag & 0x80 != 0 {
            for line in String::from_utf8_lossy(payload).split("\r\n") {
                if let Some((k, v)) = line.split_once(':') {
                    all_trailers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
                }
            }
        } else if flag & 0x01 != 0 {
            return Err("server sent a compressed message; compression is not supported".into());
        } else {
            result.messages.push(decode_to_json(output, payload)?);
        }
        i = end;
    }
    let find = |name: &str| all_trailers.iter().chain(headers.iter()).find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.clone());
    result.status = find("grpc-status").and_then(|s| s.trim().parse().ok());
    result.message = find("grpc-message").map(|m| crate::prepare::percent_decode(&m.replace('+', "%2B"))).unwrap_or_default();
    result.trailers = all_trailers;
    Ok(result)
}

/// A JSON skeleton of a message, for a new request body.
pub fn example_json(desc: &MessageDescriptor) -> String {
    serde_json::to_string_pretty(&example_value(desc, 0)).unwrap_or_else(|_| "{}".into())
}

fn example_value(desc: &MessageDescriptor, depth: usize) -> serde_json::Value {
    let mut obj = serde_json::Map::new();
    for f in desc.fields() {
        let v = if f.is_map() {
            serde_json::json!({})
        } else if f.is_list() {
            serde_json::json!([])
        } else {
            match f.kind() {
                Kind::String => serde_json::json!(""),
                Kind::Bytes => serde_json::json!(""),
                Kind::Bool => serde_json::json!(false),
                Kind::Double | Kind::Float => serde_json::json!(0.0),
                Kind::Int64 | Kind::Uint64 | Kind::Sint64 | Kind::Fixed64 | Kind::Sfixed64 => serde_json::json!("0"),
                Kind::Enum(e) => serde_json::json!(e.values().next().map(|v| v.name().to_string()).unwrap_or_default()),
                Kind::Message(m) if m.full_name().starts_with("google.protobuf.") => serde_json::Value::Null,
                Kind::Message(m) if depth < 3 => example_value(&m, depth + 1),
                Kind::Message(_) => serde_json::json!({}),
                _ => serde_json::json!(0),
            }
        };
        obj.insert(f.json_name().to_string(), v);
    }
    serde_json::Value::Object(obj)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROTO: &str = r#"
        syntax = "proto3";
        package helloworld;
        import "google/protobuf/timestamp.proto";
        service Greeter { rpc SayHello (HelloRequest) returns (HelloReply); }
        message HelloRequest { string name = 1; int32 times = 2; google.protobuf.Timestamp at = 3; }
        message HelloReply { string message = 1; }
    "#;

    #[test]
    fn compile_encode_decode() {
        let pool = compile(PROTO).unwrap();
        let svcs = services(&pool);
        assert_eq!(svcs[0].name, "helloworld.Greeter");
        assert_eq!(svcs[0].methods[0].name, "SayHello");
        let m = method(&pool, "helloworld.Greeter", "SayHello").unwrap();
        let bytes = encode_json(&m.input(), r#"{"name":"world","times":2}"#).unwrap();
        assert_eq!(bytes, vec![0x0a, 5, b'w', b'o', b'r', b'l', b'd', 0x10, 2]);
        assert!(example_json(&m.input()).contains("\"times\": 0"));

        // gRPC-Web style response: one data frame then a trailer frame.
        let reply = encode_json(&m.output(), r#"{"message":"Hello world"}"#).unwrap();
        let mut body = frame(&reply);
        let trailer = b"grpc-status: 0\r\ngrpc-message: \r\n";
        body.push(0x80);
        body.extend_from_slice(&(trailer.len() as u32).to_be_bytes());
        body.extend_from_slice(trailer);
        let r = decode_response(&m.output(), "application/grpc-web+proto", &body, &[], &[]).unwrap();
        assert!(r.ok());
        assert!(r.body_json().contains("Hello world"));
    }

    #[test]
    fn bad_proto_reports() {
        assert!(compile("syntax = \"proto3\"; message {").unwrap_err().contains("proto error"));
    }
}

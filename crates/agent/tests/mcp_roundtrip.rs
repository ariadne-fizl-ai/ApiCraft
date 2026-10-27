//! Fizl ApiCraft's MCP client against its own MCP server, over stdio (a real
//! `apicraft-agent mcp` process), Streamable HTTP and legacy SSE (a real
//! `serve` on a free port).

use std::net::TcpListener;
use std::path::PathBuf;

use apicraft_agent::NativeTransport;
use apicraft_core::mcp;
use apicraft_core::model::*;
use apicraft_core::store;

fn catalog_file() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("apicraft-mcp-e2e-{}", apicraft_core::ids::new_id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("catalog.json.gz");
    let mut cat = Catalog::default();
    let mut col = Collection::new("Self test");
    let mut r = Request::new("Find me", Protocol::Rest);
    r.url = "https://example.com/found".into();
    col.items.push(Item::Request(r));
    cat.collections.push(col);
    std::fs::write(&path, store::to_gzip(&cat)).unwrap();
    path
}

fn call_list_requests() -> McpOptions {
    McpOptions { operation: McpOperation::CallTool, target: "list_requests".into(), ..Default::default() }
}

fn assert_found(opts: &McpOptions, ex: &mcp::McpExchange) {
    let (result, err) = mcp::outcome_result(opts, ex);
    assert!(err.is_none(), "{err:?}");
    let text = result.pointer("/content/0/text").and_then(|t| t.as_str()).unwrap_or("");
    assert!(text.contains("Self test / Find me"), "{result}");
    let server = ex.responses[0].pointer("/result/serverInfo/name").and_then(|n| n.as_str());
    assert_eq!(server, Some("fizl-apicraft"));
}

#[test]
fn stdio_round_trip() {
    let catalog = catalog_file();
    let opts = McpOptions {
        transport: McpTransport::Stdio,
        command: env!("CARGO_BIN_EXE_apicraft-agent").into(),
        args: vec!["mcp".into(), "--catalog".into(), catalog.display().to_string()],
        ..call_list_requests()
    };
    let (_, ex) = pollster::block_on(mcp::run(&opts, "", &[], "{}", &NativeTransport)).unwrap();
    assert_found(&opts, &ex);
    assert!(ex.stderr.contains("MCP server on stdio"), "stderr: {}", ex.stderr);

    // Discovery lists the tools with their input schemas.
    let cat = pollster::block_on(mcp::discover(&opts, "", &[], &NativeTransport)).unwrap();
    assert!(cat.tools.iter().any(|(n, _, schema)| n == "send_request" && schema["required"][0] == "request"));
}

#[test]
fn http_and_sse_round_trips() {
    let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let catalog = catalog_file();
    std::thread::spawn(move || {
        let _ = apicraft_agent::server::serve(apicraft_agent::server::Config { port, web_dir: None, data_file: catalog, allow_origins: Vec::new() });
    });
    std::thread::sleep(std::time::Duration::from_millis(300));

    let http = McpOptions { transport: McpTransport::Http, ..call_list_requests() };
    let (_, ex) = pollster::block_on(mcp::run(&http, &format!("http://127.0.0.1:{port}/mcp"), &[], "{}", &NativeTransport)).unwrap();
    assert_found(&http, &ex);
    assert!(ex.info.iter().any(|(k, _)| k == "Mcp-Session-Id"));

    let sse = McpOptions { transport: McpTransport::Sse, ..call_list_requests() };
    let (_, ex) = pollster::block_on(mcp::run(&sse, &format!("http://127.0.0.1:{port}/sse"), &[], "{}", &NativeTransport)).unwrap();
    assert_found(&sse, &ex);
    assert!(ex.info.iter().any(|(k, v)| k == "endpoint" && v.contains("/messages?sessionId=")));
}

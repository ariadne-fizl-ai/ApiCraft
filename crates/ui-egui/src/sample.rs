//! The starter catalog shown on first launch: one request per protocol,
//! the main auth styles, and a workflow that passes a token along.

use apicraft_core::model::*;

fn kv(k: &str, v: &str) -> KeyValue {
    KeyValue::new(k, v)
}

pub fn catalog() -> Catalog {
    let mut cat = Catalog::default();
    let mut env = Environment::new("httpbin");
    env.variables.push(kv("base_url", "https://httpbin.org"));
    env.variables.push(kv("user", "demo"));
    env.variables.push(kv("password", "s3cret"));
    cat.active_environment = Some(env.id.clone());
    cat.environments.push(env);

    let mut col = Collection::new("Examples");
    col.description = "Starter requests for each protocol. Edit or delete them freely.".into();

    let mut rest = Folder::new("REST");
    let mut get = Request::new("Get with query", Protocol::Rest);
    get.url = "{{base_url}}/get".into();
    get.params.push(kv("search", "fizl"));
    get.checks.push(Check { kind: CheckKind::Status, target: String::new(), expected: "200".into(), enabled: true });
    rest.items.push(Item::Request(get));

    let mut post = Request::new("Post JSON", Protocol::Rest);
    post.method = "POST".into();
    post.url = "{{base_url}}/post".into();
    post.body = Body::Raw { content_type: "application/json".into(), text: "{\n  \"name\": \"Ada\",\n  \"id\": \"{{$guid}}\"\n}".into() };
    post.extract.push(Extraction {
        variable: "echoed_name".into(),
        source: ExtractSource::Json,
        expression: "$.json.name".into(),
        scope: VarScope::Runtime,
        enabled: true,
    });
    rest.items.push(Item::Request(post));

    let mut basic = Request::new("Basic auth", Protocol::Rest);
    basic.url = "{{base_url}}/basic-auth/{{user}}/{{password}}".into();
    basic.auth = Auth::Basic { username: "{{user}}".into(), password: "{{password}}".into() };
    rest.items.push(Item::Request(basic));

    let mut digest = Request::new("Digest auth", Protocol::Rest);
    digest.url = "{{base_url}}/digest-auth/auth/{{user}}/{{password}}".into();
    digest.auth = Auth::Digest { username: "{{user}}".into(), password: "{{password}}".into() };
    digest.description = "Digest needs the server's WWW-Authenticate challenge, which browsers only expose to pages when the server allows it; run the local agent for this one.".into();
    rest.items.push(Item::Request(digest));
    col.items.push(Item::Folder(rest));

    let mut soap = Request::new("Number to words (SOAP)", Protocol::Soap);
    soap.url = "https://www.dataaccess.com/webservicesserver/NumberConversion.wso".into();
    soap.body = Body::Raw {
        content_type: String::new(),
        text: "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<soap:Envelope xmlns:soap=\"http://schemas.xmlsoap.org/soap/envelope/\">\n  <soap:Body>\n    <NumberToWords xmlns=\"http://www.dataaccess.com/webservicesserver/\">\n      <ubiNum>42</ubiNum>\n    </NumberToWords>\n  </soap:Body>\n</soap:Envelope>".into(),
    };
    soap.extract.push(Extraction {
        variable: "words".into(),
        source: ExtractSource::Xml,
        expression: "Envelope/Body/NumberToWordsResponse/NumberToWordsResult".into(),
        scope: VarScope::Runtime,
        enabled: true,
    });
    col.items.push(Item::Request(soap));

    let mut grpc = Request::new("Say hello (gRPC)", Protocol::Grpc);
    grpc.url = "https://grpcb.in:9001".into();
    grpc.grpc.proto = "syntax = \"proto3\";\n\npackage hello;\n\nservice HelloService {\n  rpc SayHello (HelloRequest) returns (HelloResponse);\n}\n\nmessage HelloRequest {\n  string greeting = 1;\n}\n\nmessage HelloResponse {\n  string reply = 1;\n}\n".into();
    grpc.grpc.service = "hello.HelloService".into();
    grpc.grpc.method = "SayHello".into();
    grpc.body = Body::Raw { content_type: "application/json".into(), text: "{\n  \"greeting\": \"Fizl\"\n}".into() };
    grpc.description = "grpcb.in speaks native gRPC, so this needs the local agent (browsers can only do gRPC-Web).".into();
    col.items.push(Item::Request(grpc));

    let mut ws = Request::new("Echo (WebSocket)", Protocol::WebSocket);
    ws.url = "wss://echo.websocket.org".into();
    ws.websocket.messages = vec!["Hello from Fizl ApiCraft".into(), "{\"type\":\"ping\",\"at\":\"{{$isoTimestamp}}\"}".into()];
    col.items.push(Item::Request(ws));

    // A workflow: get a "token" from one response and use it in the next request.
    let mut flow = Folder::new("Token flow");
    let mut login = Request::new("Log in", Protocol::Rest);
    login.method = "POST".into();
    login.url = "{{base_url}}/anything/login".into();
    login.body = Body::Raw { content_type: "application/json".into(), text: "{\n  \"username\": \"{{user}}\",\n  \"token\": \"tok-{{$randomInt}}\"\n}".into() };
    login.extract.push(Extraction {
        variable: "token".into(),
        source: ExtractSource::Json,
        expression: "$.json.token".into(),
        scope: VarScope::Runtime,
        enabled: true,
    });
    login.checks.push(Check { kind: CheckKind::Status, target: String::new(), expected: "2xx".into(), enabled: true });
    let mut me = Request::new("Use the token", Protocol::Rest);
    me.url = "{{base_url}}/bearer".into();
    me.auth = Auth::Bearer { token: "{{token}}".into() };
    me.checks.push(Check { kind: CheckKind::Status, target: String::new(), expected: "200".into(), enabled: true });
    me.checks.push(Check { kind: CheckKind::JsonEquals, target: "$.token".into(), expected: "{{token}}".into(), enabled: true });
    let mut wf = Workflow::new("Token flow");
    wf.steps.push(WorkflowStep::new(login.id.clone()));
    wf.steps.push(WorkflowStep::new(me.id.clone()));
    flow.items.push(Item::Request(login));
    flow.items.push(Item::Request(me));
    col.items.push(Item::Folder(flow));

    col.items.push(Item::Folder(mcp_folder()));
    cat.collections.push(col);
    cat.workflows.push(wf);
    cat
}

/// MCP test mode: Fizl ApiCraft's own MCP server over all three transports,
/// plus the reference "everything" server (needs Node.js).
fn mcp_folder() -> Folder {
    let mut f = Folder::new("MCP");
    let mk = |name: &str, transport: McpTransport, url: &str, command: &str, op: McpOperation, target: &str, body: &str, desc: &str| {
        let mut r = Request::new(name, Protocol::Mcp);
        r.url = url.into();
        r.mcp = McpOptions { transport, command: command.into(), args: Vec::new(), env: Vec::new(), operation: op, target: target.into() };
        r.body = Body::Raw { content_type: "application/json".into(), text: body.into() };
        r.description = desc.into();
        r.checks.push(Check { kind: CheckKind::Status, target: String::new(), expected: "200".into(), enabled: true });
        r
    };
    let mut stdio = mk(
        "ApiCraft MCP over stdio: list requests",
        McpTransport::Stdio,
        "",
        "apicraft-agent mcp",
        McpOperation::CallTool,
        "list_requests",
        "{}",
        "Starts Fizl ApiCraft's own MCP server (`apicraft-agent mcp`, shipped next to the app) and calls its list_requests tool.",
    );
    stdio.checks.push(Check { kind: CheckKind::BodyContains, target: String::new(), expected: "Examples".into(), enabled: true });
    f.items.push(Item::Request(stdio));
    f.items.push(Item::Request(mk(
        "ApiCraft MCP over Streamable HTTP: list tools",
        McpTransport::Http,
        "http://127.0.0.1:8787/mcp",
        "",
        McpOperation::ListTools,
        "",
        "{}",
        "Needs `apicraft-agent serve` running (or `cargo xtask serve`).",
    )));
    f.items.push(Item::Request(mk(
        "ApiCraft MCP over SSE: list tools",
        McpTransport::Sse,
        "http://127.0.0.1:8787/sse",
        "",
        McpOperation::ListTools,
        "",
        "{}",
        "The legacy HTTP+SSE transport. Needs `apicraft-agent serve` running.",
    )));
    f.items.push(Item::Request(mk(
        "Everything server (npx): echo",
        McpTransport::Stdio,
        "",
        "npx -y @modelcontextprotocol/server-everything",
        McpOperation::CallTool,
        "echo",
        "{\n  \"message\": \"Hello from Fizl ApiCraft\"\n}",
        "The MCP reference server with every feature. Needs Node.js; the first run downloads it.",
    )));
    f
}

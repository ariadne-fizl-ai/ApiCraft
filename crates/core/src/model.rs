//! The catalog: collections of saved requests, plus environments.
//!
//! Everything here is plain serde data, so the whole catalog round-trips
//! through JSON (and the gzip-compressed form in [`crate::store`]).

use serde::{Deserialize, Serialize};

use crate::ids::new_id;

pub const CATALOG_FORMAT: &str = "fizl-apicraft";
/// The format tag written before the rename to ApiCraft; still read.
pub const LEGACY_CATALOG_FORMAT: &str = "fizl-apipost";
pub const CATALOG_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Catalog {
    #[serde(default = "default_format")]
    pub format: String,
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub collections: Vec<Collection>,
    #[serde(default)]
    pub environments: Vec<Environment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_environment: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workflows: Vec<Workflow>,
    /// Values captured from responses. They override environment and
    /// collection variables, and persist until cleared.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runtime: Vec<KeyValue>,
    /// Proxy and TLS settings for every request.
    #[serde(default, skip_serializing_if = "NetworkSettings::is_default")]
    pub network: NetworkSettings,
}

fn default_format() -> String {
    CATALOG_FORMAT.into()
}
fn default_version() -> u32 {
    CATALOG_VERSION
}

impl Default for Catalog {
    fn default() -> Self {
        Catalog {
            format: default_format(),
            version: CATALOG_VERSION,
            collections: Vec::new(),
            environments: Vec::new(),
            active_environment: None,
            workflows: Vec::new(),
            runtime: Vec::new(),
            network: NetworkSettings::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Collection {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub variables: Vec<KeyValue>,
    /// Used by requests whose auth is `Inherit`.
    #[serde(default = "no_auth")]
    pub auth: Auth,
    #[serde(default)]
    pub items: Vec<Item>,
    /// A folder on disk that holds this collection as one text file per
    /// request and workflow (see [`crate::folder`]). The catalog then keeps
    /// only this link; the files are the source of truth.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub directory: Option<String>,
}

fn no_auth() -> Auth {
    Auth::None
}

impl Collection {
    pub fn new(name: impl Into<String>) -> Self {
        Collection { id: new_id(), name: name.into(), description: String::new(), variables: Vec::new(), auth: Auth::None, items: Vec::new(), directory: None }
    }
}

// Requests are much bigger than folders; catalogs are small, so boxing isn't worth the churn.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Item {
    Folder(Folder),
    Request(Request),
}

impl Item {
    pub fn id(&self) -> &str {
        match self {
            Item::Folder(f) => &f.id,
            Item::Request(r) => &r.id,
        }
    }
    pub fn name(&self) -> &str {
        match self {
            Item::Folder(f) => &f.name,
            Item::Request(r) => &r.name,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Folder {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub items: Vec<Item>,
}

/// Where [`Catalog::move_item`] puts an item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MoveTo {
    /// At the end of a collection or folder.
    Into(String),
    /// Next to another request or folder, in its container.
    Before(String),
    After(String),
}

impl Folder {
    pub fn new(name: impl Into<String>) -> Self {
        Folder { id: new_id(), name: name.into(), items: Vec::new() }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    #[default]
    Rest,
    Soap,
    Grpc,
    #[serde(rename = "websocket")]
    WebSocket,
    /// Model Context Protocol servers (stdio, SSE or Streamable HTTP).
    Mcp,
    /// GraphQL over HTTP: a query, variables and an operation name.
    #[serde(rename = "graphql")]
    GraphQl,
    /// OData v2-v4 services: a resource path under the service root plus
    /// system query options.
    #[serde(rename = "odata")]
    OData,
}

impl Protocol {
    pub const ALL: [Protocol; 7] = [Protocol::Rest, Protocol::GraphQl, Protocol::OData, Protocol::Soap, Protocol::Grpc, Protocol::WebSocket, Protocol::Mcp];

    pub fn label(self) -> &'static str {
        match self {
            Protocol::Rest => "REST",
            Protocol::Soap => "SOAP",
            Protocol::Grpc => "gRPC",
            Protocol::WebSocket => "WebSocket",
            Protocol::Mcp => "MCP",
            Protocol::GraphQl => "GraphQL",
            Protocol::OData => "OData",
        }
    }
    pub fn key(self) -> &'static str {
        match self {
            Protocol::Rest => "rest",
            Protocol::Soap => "soap",
            Protocol::Grpc => "grpc",
            Protocol::WebSocket => "websocket",
            Protocol::Mcp => "mcp",
            Protocol::GraphQl => "graphql",
            Protocol::OData => "odata",
        }
    }
    pub fn from_key(s: &str) -> Option<Protocol> {
        Protocol::ALL.into_iter().find(|p| p.key() == s)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub protocol: Protocol,
    #[serde(default = "default_method")]
    pub method: String,
    #[serde(default)]
    pub url: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub params: Vec<KeyValue>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<KeyValue>,
    #[serde(default)]
    pub body: Body,
    #[serde(default)]
    pub auth: Auth,
    #[serde(default, skip_serializing_if = "SoapOptions::is_default")]
    pub soap: SoapOptions,
    #[serde(default, skip_serializing_if = "GrpcOptions::is_default")]
    pub grpc: GrpcOptions,
    #[serde(default, skip_serializing_if = "WsOptions::is_default")]
    pub websocket: WsOptions,
    #[serde(default, skip_serializing_if = "McpOptions::is_default")]
    pub mcp: McpOptions,
    #[serde(default, skip_serializing_if = "GraphQlOptions::is_default")]
    pub graphql: GraphQlOptions,
    #[serde(default, skip_serializing_if = "ODataOptions::is_default")]
    pub odata: ODataOptions,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extract: Vec<Extraction>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checks: Vec<Check>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}

fn default_method() -> String {
    "GET".into()
}

impl Request {
    pub fn new(name: impl Into<String>, protocol: Protocol) -> Self {
        let mut r = Request {
            id: new_id(),
            name: name.into(),
            protocol,
            method: default_method(),
            url: String::new(),
            params: Vec::new(),
            headers: Vec::new(),
            body: Body::None,
            auth: Auth::Inherit,
            soap: SoapOptions::default(),
            grpc: GrpcOptions::default(),
            websocket: WsOptions::default(),
            mcp: McpOptions::default(),
            graphql: GraphQlOptions::default(),
            odata: ODataOptions::default(),
            extract: Vec::new(),
            checks: Vec::new(),
            description: String::new(),
        };
        match protocol {
            Protocol::Rest => {}
            Protocol::Soap => {
                r.method = "POST".into();
                r.body = Body::Raw { content_type: String::new(), text: crate::soap::envelope_template(SoapVersion::V11) };
            }
            Protocol::Grpc => {
                r.method = "POST".into();
                r.body = Body::Raw { content_type: "application/json".into(), text: "{}".into() };
            }
            Protocol::WebSocket => {
                r.url = "wss://".into();
            }
            Protocol::Mcp => {
                r.method = "POST".into();
                r.url = "http://127.0.0.1:8787/mcp".into();
                r.body = Body::Raw { content_type: "application/json".into(), text: "{}".into() };
            }
            Protocol::GraphQl => {
                r.method = "POST".into();
                r.graphql.query = "query {
  __typename
}
"
                .into();
            }
            Protocol::OData => {}
        }
        r
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyValue {
    pub key: String,
    #[serde(default)]
    pub value: String,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub enabled: bool,
}

fn yes() -> bool {
    true
}
fn is_true(b: &bool) -> bool {
    *b
}

impl KeyValue {
    pub fn new(key: impl Into<String>, value: impl Into<String>) -> Self {
        KeyValue { key: key.into(), value: value.into(), enabled: true }
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum Body {
    #[default]
    None,
    /// Raw text. An empty `content_type` means "pick one from the protocol".
    Raw {
        #[serde(default)]
        content_type: String,
        #[serde(default)]
        text: String,
    },
    #[serde(rename = "urlencoded")]
    UrlEncoded { fields: Vec<KeyValue> },
    /// multipart/form-data with text fields.
    #[serde(rename = "formdata")]
    FormData { fields: Vec<KeyValue> },
}

impl Body {
    pub fn key(&self) -> &'static str {
        match self {
            Body::None => "none",
            Body::Raw { .. } => "raw",
            Body::UrlEncoded { .. } => "urlencoded",
            Body::FormData { .. } => "formdata",
        }
    }
    pub fn raw_text(&self) -> &str {
        match self {
            Body::Raw { text, .. } => text,
            _ => "",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Auth {
    /// Use the collection's auth.
    #[default]
    Inherit,
    None,
    Basic {
        username: String,
        password: String,
    },
    /// RFC 7616 digest: answered after the server's 401 challenge.
    Digest {
        username: String,
        password: String,
    },
    Bearer {
        token: String,
    },
    #[serde(rename = "apikey")]
    ApiKey {
        key: String,
        value: String,
        /// `true` puts it in a header, otherwise in the query string.
        #[serde(default = "yes")]
        in_header: bool,
    },
    #[serde(rename = "oauth2")]
    OAuth2(OAuth2),
    #[serde(rename = "oauth1")]
    OAuth1(OAuth1),
    #[serde(rename = "awsv4")]
    AwsSigV4(AwsSigV4),
    Jwt(JwtAuth),
    /// Mutual TLS: the client presents a certificate during the handshake.
    #[serde(rename = "clientcert")]
    ClientCert(ClientCertAuth),
}

/// PEM text, or the path of a PEM file, for each part.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ClientCertAuth {
    /// The certificate (and any intermediates).
    pub certificate: String,
    /// The unencrypted private key (PKCS#8, PKCS#1 or SEC1).
    pub key: String,
}

impl Auth {
    pub const KINDS: [(&'static str, &'static str); 11] = [
        ("inherit", "Inherit from collection"),
        ("none", "No auth"),
        ("basic", "Basic"),
        ("digest", "Digest"),
        ("bearer", "Bearer token"),
        ("apikey", "API key"),
        ("oauth2", "OAuth 2.0"),
        ("oauth1", "OAuth 1.0a"),
        ("awsv4", "AWS Signature v4"),
        ("jwt", "JWT (signed here)"),
        ("clientcert", "Client certificate (mTLS)"),
    ];

    pub fn key(&self) -> &'static str {
        match self {
            Auth::Inherit => "inherit",
            Auth::None => "none",
            Auth::Basic { .. } => "basic",
            Auth::Digest { .. } => "digest",
            Auth::Bearer { .. } => "bearer",
            Auth::ApiKey { .. } => "apikey",
            Auth::OAuth2(_) => "oauth2",
            Auth::OAuth1(_) => "oauth1",
            Auth::AwsSigV4(_) => "awsv4",
            Auth::Jwt(_) => "jwt",
            Auth::ClientCert(_) => "clientcert",
        }
    }

    /// A blank auth of the given kind, keeping username/password where both have them.
    pub fn of_kind(kind: &str, current: &Auth) -> Auth {
        let (user, pass) = match current {
            Auth::Basic { username, password } | Auth::Digest { username, password } => (username.clone(), password.clone()),
            _ => (String::new(), String::new()),
        };
        match kind {
            "inherit" => Auth::Inherit,
            "basic" => Auth::Basic { username: user, password: pass },
            "digest" => Auth::Digest { username: user, password: pass },
            "bearer" => Auth::Bearer { token: String::new() },
            "apikey" => Auth::ApiKey { key: "X-API-Key".into(), value: String::new(), in_header: true },
            "oauth2" => Auth::OAuth2(OAuth2::default()),
            "oauth1" => Auth::OAuth1(OAuth1::default()),
            "awsv4" => Auth::AwsSigV4(AwsSigV4::default()),
            "jwt" => Auth::Jwt(JwtAuth::default()),
            "clientcert" => Auth::ClientCert(ClientCertAuth::default()),
            _ => Auth::None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrantType {
    #[default]
    ClientCredentials,
    Password,
    AuthorizationCode,
    /// Token pasted or obtained elsewhere; refreshed with the refresh token if one is set.
    Manual,
}

impl GrantType {
    pub const ALL: [(GrantType, &'static str, &'static str); 4] = [
        (GrantType::ClientCredentials, "client_credentials", "Client credentials"),
        (GrantType::Password, "password", "Password"),
        (GrantType::AuthorizationCode, "authorization_code", "Authorization code (PKCE)"),
        (GrantType::Manual, "manual", "Manual token"),
    ];
    pub fn key(self) -> &'static str {
        GrantType::ALL.iter().find(|g| g.0 == self).map(|g| g.1).unwrap_or("manual")
    }
    pub fn from_key(s: &str) -> GrantType {
        GrantType::ALL.iter().find(|g| g.1 == s).map(|g| g.0).unwrap_or_default()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OAuth2 {
    pub grant: GrantType,
    pub auth_url: String,
    pub token_url: String,
    pub client_id: String,
    pub client_secret: String,
    pub scope: String,
    pub audience: String,
    pub username: String,
    pub password: String,
    /// Empty means the app's own callback page.
    pub redirect_uri: String,
    pub use_pkce: bool,
    /// Send client id/secret as HTTP Basic (otherwise in the form body).
    pub client_auth_basic: bool,
    pub header_prefix: String,
    // Token state, filled in when a token is obtained.
    pub access_token: String,
    pub refresh_token: String,
    /// Unix time in ms; 0 means unknown/never.
    pub expires_at: u64,
}

impl Default for OAuth2 {
    fn default() -> Self {
        OAuth2 {
            grant: GrantType::ClientCredentials,
            auth_url: String::new(),
            token_url: String::new(),
            client_id: String::new(),
            client_secret: String::new(),
            scope: String::new(),
            audience: String::new(),
            username: String::new(),
            password: String::new(),
            redirect_uri: String::new(),
            use_pkce: true,
            client_auth_basic: true,
            header_prefix: "Bearer".into(),
            access_token: String::new(),
            refresh_token: String::new(),
            expires_at: 0,
        }
    }
}

impl OAuth2 {
    /// Usable now, with a 30 s margin.
    pub fn token_valid(&self, now_ms: u64) -> bool {
        !self.access_token.is_empty() && (self.expires_at == 0 || self.expires_at > now_ms + 30_000)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum OAuth1Method {
    #[default]
    #[serde(rename = "HMAC-SHA1")]
    HmacSha1,
    #[serde(rename = "HMAC-SHA256")]
    HmacSha256,
    #[serde(rename = "PLAINTEXT")]
    Plaintext,
}

impl OAuth1Method {
    pub fn name(self) -> &'static str {
        match self {
            OAuth1Method::HmacSha1 => "HMAC-SHA1",
            OAuth1Method::HmacSha256 => "HMAC-SHA256",
            OAuth1Method::Plaintext => "PLAINTEXT",
        }
    }
    pub fn from_name(s: &str) -> OAuth1Method {
        match s.to_ascii_uppercase().as_str() {
            "HMAC-SHA256" => OAuth1Method::HmacSha256,
            "PLAINTEXT" => OAuth1Method::Plaintext,
            _ => OAuth1Method::HmacSha1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct OAuth1 {
    pub consumer_key: String,
    pub consumer_secret: String,
    pub token: String,
    pub token_secret: String,
    pub signature_method: OAuth1Method,
    pub realm: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AwsSigV4 {
    pub access_key: String,
    pub secret_key: String,
    pub session_token: String,
    pub region: String,
    pub service: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct JwtAuth {
    /// HS256, HS384 or HS512.
    pub algorithm: String,
    pub secret: String,
    /// JSON claims. `iat`/`exp` are added if missing and `expires_in` is set.
    pub payload: String,
    pub expires_in_secs: u64,
    pub header_prefix: String,
}

impl Default for JwtAuth {
    fn default() -> Self {
        JwtAuth {
            algorithm: "HS256".into(),
            secret: String::new(),
            payload: "{\n  \"sub\": \"user\"\n}".into(),
            expires_in_secs: 3600,
            header_prefix: "Bearer".into(),
        }
    }
}

// ---------------------------------------------------------------- extraction and checks

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExtractSource {
    /// JSON path into the body (`$.data.items[0].id`); also works on gRPC responses.
    #[default]
    Json,
    /// Element path into an XML/SOAP body (`Envelope/Body/AddResponse/AddResult`).
    Xml,
    Header,
    Status,
    /// Regex over the body text; the first capture group (or whole match) is used.
    Regex,
}

impl ExtractSource {
    pub const ALL: [(ExtractSource, &'static str, &'static str); 5] = [
        (ExtractSource::Json, "json", "JSON path"),
        (ExtractSource::Xml, "xml", "XML path"),
        (ExtractSource::Header, "header", "Header"),
        (ExtractSource::Status, "status", "Status code"),
        (ExtractSource::Regex, "regex", "Regex"),
    ];
    pub fn key(self) -> &'static str {
        ExtractSource::ALL.iter().find(|s| s.0 == self).map(|s| s.1).unwrap()
    }
    pub fn from_key(k: &str) -> ExtractSource {
        ExtractSource::ALL.iter().find(|s| s.1 == k).map(|s| s.0).unwrap_or_default()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VarScope {
    /// Runtime variables: captured values that override everything else.
    #[default]
    Runtime,
    /// Written into the active environment.
    Environment,
}

/// "Take this from the response and store it as `{{variable}}`."
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Extraction {
    pub variable: String,
    #[serde(default)]
    pub source: ExtractSource,
    #[serde(default)]
    pub expression: String,
    #[serde(default)]
    pub scope: VarScope,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckKind {
    /// `expected` is a status code, a range like `2xx`, or a list `200,201`.
    #[default]
    Status,
    BodyContains,
    /// `target` is a JSON path; `expected` the value (compared as text). Empty `expected` = "exists".
    JsonEquals,
    /// `target` is a header name; empty `expected` = "present".
    Header,
    /// `expected` is a limit in milliseconds.
    TimeBelow,
}

impl CheckKind {
    pub const ALL: [(CheckKind, &'static str, &'static str); 5] = [
        (CheckKind::Status, "status", "Status is"),
        (CheckKind::BodyContains, "body_contains", "Body contains"),
        (CheckKind::JsonEquals, "json_equals", "JSON value"),
        (CheckKind::Header, "header", "Header"),
        (CheckKind::TimeBelow, "time_below", "Time below (ms)"),
    ];
    pub fn key(self) -> &'static str {
        CheckKind::ALL.iter().find(|s| s.0 == self).map(|s| s.1).unwrap()
    }
    pub fn from_key(k: &str) -> CheckKind {
        CheckKind::ALL.iter().find(|s| s.1 == k).map(|s| s.0).unwrap_or_default()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Check {
    pub kind: CheckKind,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub target: String,
    #[serde(default)]
    pub expected: String,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub enabled: bool,
}

// ---------------------------------------------------------------- workflows

/// Requests run in order, passing captured variables along.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Workflow {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub steps: Vec<WorkflowStep>,
    #[serde(default = "yes")]
    pub stop_on_failure: bool,
    /// Variables set before the first step (lowest precedence after collection vars).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub variables: Vec<KeyValue>,
    #[serde(default = "one")]
    pub iterations: u32,
}

fn one() -> u32 {
    1
}

impl Workflow {
    pub fn new(name: impl Into<String>) -> Self {
        Workflow { id: new_id(), name: name.into(), steps: Vec::new(), stop_on_failure: true, variables: Vec::new(), iterations: 1 }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkflowStep {
    pub id: String,
    pub request_id: String,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub enabled: bool,
    /// Pause before the step, in ms.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub delay_ms: u32,
    /// Extra captures for this step, on top of the request's own.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extract: Vec<Extraction>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checks: Vec<Check>,
    /// Repeat this step (and the next `steps - 1`) once per item of a list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub for_each: Option<ForEach>,
}

/// A loop over a list held in a variable, usually captured from an earlier
/// response. Each item is available as `{{item}}` (named by `item`), its
/// fields as `{{item.field}}`, and its position as `{{$index}}` (from 0).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForEach {
    /// The variable holding the list: a JSON array, or lines of text.
    pub list: String,
    /// A JSON path into the variable, e.g. `$.value` or `$.items[*].id`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub path: String,
    #[serde(default = "default_item")]
    pub item: String,
    /// How many steps, starting with this one, repeat for each item.
    #[serde(default = "one")]
    pub steps: u32,
}

fn default_item() -> String {
    "item".into()
}

impl Default for ForEach {
    fn default() -> Self {
        ForEach { list: String::new(), path: String::new(), item: default_item(), steps: 1 }
    }
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

impl WorkflowStep {
    pub fn new(request_id: impl Into<String>) -> Self {
        WorkflowStep { id: new_id(), request_id: request_id.into(), enabled: true, delay_ms: 0, extract: Vec::new(), checks: Vec::new(), for_each: None }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProxyMode {
    /// The `HTTPS_PROXY` / `HTTP_PROXY` / `ALL_PROXY` and `NO_PROXY` environment variables.
    #[default]
    System,
    None,
    Manual,
}

/// Proxy and TLS settings, kept with the catalog so the desktop app, the
/// agent and the CLI all send the same way. `{{variables}}` work in them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct NetworkSettings {
    pub proxy: ProxyMode,
    /// `http://host:port`, `https://...` or `socks5://...`.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub proxy_url: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub proxy_username: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub proxy_password: String,
    /// Hosts that skip the proxy, comma separated (`localhost,.internal`).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub no_proxy: String,
    /// Check server certificates. Turning this off accepts any certificate.
    pub verify_tls: bool,
    /// CA certificates to trust instead of the built-in roots: PEM text or
    /// the path of a PEM file.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub ca_certificates: String,
}

impl Default for NetworkSettings {
    fn default() -> Self {
        NetworkSettings {
            proxy: ProxyMode::System,
            proxy_url: String::new(),
            proxy_username: String::new(),
            proxy_password: String::new(),
            no_proxy: String::new(),
            verify_tls: true,
            ca_certificates: String::new(),
        }
    }
}

impl NetworkSettings {
    pub fn is_default(&self) -> bool {
        *self == NetworkSettings::default()
    }

    /// The options a request is sent with, `{{variables}}` substituted.
    pub fn connect_options(&self, vars: &crate::vars::Vars) -> crate::http::ConnectOptions {
        let proxy = match self.proxy {
            ProxyMode::System => None,
            ProxyMode::None => Some(String::new()),
            ProxyMode::Manual => {
                let url = vars.apply(self.proxy_url.trim());
                let user = vars.apply(&self.proxy_username);
                let url = if user.is_empty() || url.is_empty() {
                    url
                } else {
                    // user:password@ goes after the scheme.
                    let (scheme, rest) = url.split_once("://").unwrap_or(("http", url.as_str()));
                    let enc = crate::prepare::percent_encode;
                    format!("{scheme}://{}:{}@{rest}", enc(&user), enc(&vars.apply(&self.proxy_password)))
                };
                Some(url)
            }
        };
        crate::http::ConnectOptions {
            proxy,
            no_proxy: vars.apply(&self.no_proxy),
            insecure: !self.verify_tls,
            ca: vars.apply(self.ca_certificates.trim()),
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SoapVersion {
    #[default]
    #[serde(rename = "1.1")]
    V11,
    #[serde(rename = "1.2")]
    V12,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SoapOptions {
    #[serde(default)]
    pub version: SoapVersion,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub action: String,
}

impl SoapOptions {
    fn is_default(&self) -> bool {
        *self == SoapOptions::default()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct GraphQlOptions {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub query: String,
    /// A JSON object; empty sends no variables.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub variables: String,
    /// Which operation to run when the document has several.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub operation_name: String,
}

impl GraphQlOptions {
    fn is_default(&self) -> bool {
        *self == GraphQlOptions::default()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ODataVersion {
    #[serde(rename = "2")]
    V2,
    #[serde(rename = "3")]
    V3,
    #[default]
    #[serde(rename = "4")]
    V4,
}

impl ODataVersion {
    pub const ALL: [(ODataVersion, &'static str); 3] = [(ODataVersion::V4, "v4"), (ODataVersion::V3, "v3"), (ODataVersion::V2, "v2")];
}

/// An OData request: the URL is the service root and `resource` the path
/// under it (`People('russell')/Friends`). Empty options are not sent.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ODataOptions {
    pub version: ODataVersion,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub resource: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub select: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub expand: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub filter: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub orderby: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub top: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub skip: String,
    /// v4 `$search`.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub search: String,
    /// `$count=true` (v4) or `$inlinecount=allpages` (v2/v3).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub count: bool,
}

impl ODataOptions {
    fn is_default(&self) -> bool {
        *self == ODataOptions::default()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct GrpcOptions {
    /// The `.proto` source that describes the service.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub proto: String,
    /// Fully qualified service name, e.g. `helloworld.Greeter`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub service: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub method: String,
}

impl GrpcOptions {
    fn is_default(&self) -> bool {
        *self == GrpcOptions::default()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct WsOptions {
    /// Saved messages that can be sent once connected.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub messages: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subprotocols: Vec<String>,
}

impl WsOptions {
    fn is_default(&self) -> bool {
        *self == WsOptions::default()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum McpTransport {
    /// Streamable HTTP (the current spec): JSON-RPC over POST to the URL.
    #[default]
    Http,
    /// The older HTTP+SSE transport: an event stream at the URL, POSTs to the endpoint it names.
    Sse,
    /// A local process speaking JSON-RPC over stdin/stdout.
    Stdio,
}

impl McpTransport {
    pub const ALL: [(McpTransport, &'static str, &'static str); 3] =
        [(McpTransport::Http, "http", "Streamable HTTP"), (McpTransport::Sse, "sse", "SSE (legacy)"), (McpTransport::Stdio, "stdio", "stdio (local process)")];
    pub fn label(self) -> &'static str {
        McpTransport::ALL.iter().find(|t| t.0 == self).map(|t| t.2).unwrap_or("")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpOperation {
    /// Just the handshake: shows the server's name, version and capabilities.
    #[default]
    Initialize,
    ListTools,
    CallTool,
    ListResources,
    ReadResource,
    ListPrompts,
    GetPrompt,
    Ping,
    /// Any method: the body is `{"method": ..., "params": ...}`.
    Raw,
}

impl McpOperation {
    pub const ALL: [(McpOperation, &'static str, &'static str); 9] = [
        (McpOperation::Initialize, "initialize", "Initialize (server info)"),
        (McpOperation::ListTools, "tools/list", "List tools"),
        (McpOperation::CallTool, "tools/call", "Call tool"),
        (McpOperation::ListResources, "resources/list", "List resources"),
        (McpOperation::ReadResource, "resources/read", "Read resource"),
        (McpOperation::ListPrompts, "prompts/list", "List prompts"),
        (McpOperation::GetPrompt, "prompts/get", "Get prompt"),
        (McpOperation::Ping, "ping", "Ping"),
        (McpOperation::Raw, "raw", "Raw JSON-RPC"),
    ];
    pub fn method(self) -> &'static str {
        McpOperation::ALL.iter().find(|o| o.0 == self).map(|o| o.1).unwrap_or("")
    }
    pub fn label(self) -> &'static str {
        McpOperation::ALL.iter().find(|o| o.0 == self).map(|o| o.2).unwrap_or("")
    }
    /// Whether the operation needs a tool name, resource URI or prompt name.
    pub fn target_label(self) -> Option<&'static str> {
        match self {
            McpOperation::CallTool => Some("Tool"),
            McpOperation::ReadResource => Some("Resource URI"),
            McpOperation::GetPrompt => Some("Prompt"),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct McpOptions {
    pub transport: McpTransport,
    /// stdio: the program to start, its arguments and extra environment.
    pub command: String,
    pub args: Vec<String>,
    pub env: Vec<KeyValue>,
    pub operation: McpOperation,
    /// Tool name, resource URI or prompt name.
    pub target: String,
}

impl McpOptions {
    fn is_default(&self) -> bool {
        *self == McpOptions::default()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Environment {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub variables: Vec<KeyValue>,
}

impl Environment {
    pub fn new(name: impl Into<String>) -> Self {
        Environment { id: new_id(), name: name.into(), variables: Vec::new() }
    }
}

// ---------------------------------------------------------------- tree helpers

impl Catalog {
    pub fn collection(&self, id: &str) -> Option<&Collection> {
        self.collections.iter().find(|c| c.id == id)
    }
    pub fn collection_mut(&mut self, id: &str) -> Option<&mut Collection> {
        self.collections.iter_mut().find(|c| c.id == id)
    }
    pub fn active_env(&self) -> Option<&Environment> {
        let id = self.active_environment.as_deref()?;
        self.environments.iter().find(|e| e.id == id)
    }

    /// Finds a request anywhere in the catalog, returning its collection id too.
    pub fn find_request(&self, id: &str) -> Option<(&Collection, &Request)> {
        self.collections.iter().find_map(|c| find_request_in(&c.items, id).map(|r| (c, r)))
    }
    pub fn find_request_mut(&mut self, id: &str) -> Option<&mut Request> {
        self.collections.iter_mut().find_map(|c| find_request_in_mut(&mut c.items, id))
    }

    /// Returns the collection that holds the item (request or folder) with this id.
    pub fn collection_of(&self, item_id: &str) -> Option<&Collection> {
        self.collections.iter().find(|c| c.id == item_id || contains_item(&c.items, item_id))
    }

    /// Item list for a container id: a collection or a folder.
    pub fn container_items_mut(&mut self, container_id: &str) -> Option<&mut Vec<Item>> {
        for c in &mut self.collections {
            if c.id == container_id {
                return Some(&mut c.items);
            }
            if let Some(items) = folder_items_mut(&mut c.items, container_id) {
                return Some(items);
            }
        }
        None
    }

    /// Removes an item (request or folder) by id and returns it.
    pub fn remove_item(&mut self, id: &str) -> Option<Item> {
        self.collections.iter_mut().find_map(|c| remove_from(&mut c.items, id))
    }

    /// The collection or folder holding an item, and the item's index in it.
    pub fn parent_of(&self, id: &str) -> Option<(String, usize)> {
        self.collections.iter().find_map(|c| parent_in(&c.id, &c.items, id))
    }

    /// Moves a request or folder. Refuses (returning false, catalog unchanged)
    /// when the destination is missing or would put a folder inside itself.
    pub fn move_item(&mut self, id: &str, to: &MoveTo) -> bool {
        let Some(moving) = self.find_item(id) else { return false };
        let anchor = match to {
            MoveTo::Into(container) => container.as_str(),
            MoveTo::Before(sibling) | MoveTo::After(sibling) => sibling.as_str(),
        };
        if anchor == id {
            return false;
        }
        if let Item::Folder(f) = moving {
            if contains_item(&f.items, anchor) {
                return false;
            }
        }
        let target_ok = match to {
            MoveTo::Into(container) => self.collection(container).is_some() || self.find_item(container).is_some_and(|i| matches!(i, Item::Folder(_))),
            _ => self.parent_of(anchor).is_some(),
        };
        if !target_ok {
            return false;
        }
        let Some(item) = self.remove_item(id) else { return false };
        let (container, index) = match to {
            MoveTo::Into(container) => (container.clone(), usize::MAX),
            MoveTo::Before(sibling) => self.parent_of(sibling).expect("checked above"),
            MoveTo::After(sibling) => self.parent_of(sibling).map(|(c, i)| (c, i + 1)).expect("checked above"),
        };
        let items = self.container_items_mut(&container).expect("checked above");
        items.insert(index.min(items.len()), item);
        true
    }

    /// A request or folder by id.
    pub fn find_item(&self, id: &str) -> Option<&Item> {
        self.collections.iter().find_map(|c| find_item_in(&c.items, id))
    }

    /// Merges an import into an existing collection instead of adding new
    /// collections: each imported collection's items are appended (as a folder
    /// named after it when there are several), and its variables are added
    /// where the target doesn't already define them. Environments and
    /// workflows merge as usual.
    pub fn merge_into(&mut self, mut other: Catalog, collection_id: &str) {
        let incoming = std::mem::take(&mut other.collections);
        let several = incoming.len() > 1;
        if let Some(target) = self.collection_mut(collection_id) {
            for c in incoming {
                for v in c.variables {
                    if !target.variables.iter().any(|x| x.key == v.key) {
                        target.variables.push(v);
                    }
                }
                if several {
                    target.items.push(Item::Folder(Folder { id: c.id, name: c.name, items: c.items }));
                } else {
                    target.items.extend(c.items);
                }
            }
        } else {
            other.collections = incoming;
        }
        self.merge(other);
    }

    pub fn rename_item(&mut self, id: &str, name: &str) -> bool {
        for c in &mut self.collections {
            if c.id == id {
                c.name = name.into();
                return true;
            }
            if rename_in(&mut c.items, id, name) {
                return true;
            }
        }
        false
    }

    /// Adds a copy of the request next to the original, with a fresh id.
    pub fn duplicate_request(&mut self, id: &str) -> Option<String> {
        for c in &mut self.collections {
            if let Some(new_id) = duplicate_in(&mut c.items, id) {
                return Some(new_id);
            }
        }
        None
    }

    /// The auth that applies to a request, and the id of whoever owns it
    /// (the request itself, or its collection when inherited).
    pub fn effective_auth(&self, request_id: &str) -> Option<(Auth, String)> {
        let (col, req) = self.find_request(request_id)?;
        Some(match &req.auth {
            Auth::Inherit => (col.auth.clone(), col.id.clone()),
            other => (other.clone(), req.id.clone()),
        })
    }

    /// Stores a refreshed OAuth 2 token back on the request or collection that owns the auth.
    pub fn store_oauth_token(&mut self, owner_id: &str, access: &str, refresh: &str, expires_at: u64) {
        let auth = if let Some(c) = self.collection_mut(owner_id) { Some(&mut c.auth) } else { self.find_request_mut(owner_id).map(|r| &mut r.auth) };
        if let Some(Auth::OAuth2(o)) = auth {
            o.access_token = access.into();
            if !refresh.is_empty() {
                o.refresh_token = refresh.into();
            }
            o.expires_at = expires_at;
        }
    }

    pub fn set_variable(&mut self, scope: VarScope, key: &str, value: &str) {
        let list = match scope {
            VarScope::Runtime => &mut self.runtime,
            VarScope::Environment => {
                let id = self.active_environment.clone();
                match id.and_then(|id| self.environments.iter_mut().find(|e| e.id == id)) {
                    Some(env) => &mut env.variables,
                    None => &mut self.runtime,
                }
            }
        };
        match list.iter_mut().find(|kv| kv.key == key) {
            Some(kv) => {
                kv.value = value.into();
                kv.enabled = true;
            }
            None => list.push(KeyValue::new(key, value)),
        }
    }

    pub fn workflow(&self, id: &str) -> Option<&Workflow> {
        self.workflows.iter().find(|w| w.id == id)
    }
    pub fn workflow_mut(&mut self, id: &str) -> Option<&mut Workflow> {
        self.workflows.iter_mut().find(|w| w.id == id)
    }

    /// Every request with a breadcrumb label, in tree order.
    pub fn all_requests(&self) -> Vec<(String, &Request)> {
        fn walk<'a>(items: &'a [Item], prefix: &str, out: &mut Vec<(String, &'a Request)>) {
            for i in items {
                match i {
                    Item::Request(r) => out.push((format!("{prefix} / {}", r.name), r)),
                    Item::Folder(f) => walk(&f.items, &format!("{prefix} / {}", f.name), out),
                }
            }
        }
        let mut out = Vec::new();
        for c in &self.collections {
            walk(&c.items, &c.name, &mut out);
        }
        out
    }

    /// Merges another catalog into this one. Collections and environments
    /// whose id already exists are replaced, the rest are appended.
    pub fn merge(&mut self, other: Catalog) {
        for c in other.collections {
            match self.collections.iter_mut().find(|x| x.id == c.id) {
                Some(existing) => *existing = c,
                None => self.collections.push(c),
            }
        }
        for e in other.environments {
            match self.environments.iter_mut().find(|x| x.id == e.id) {
                Some(existing) => *existing = e,
                None => self.environments.push(e),
            }
        }
        for w in other.workflows {
            match self.workflows.iter_mut().find(|x| x.id == w.id) {
                Some(existing) => *existing = w,
                None => self.workflows.push(w),
            }
        }
        if self.active_environment.is_none() {
            self.active_environment = other.active_environment;
        }
    }
}

pub fn find_request_in<'a>(items: &'a [Item], id: &str) -> Option<&'a Request> {
    items.iter().find_map(|i| match i {
        Item::Request(r) if r.id == id => Some(r),
        Item::Folder(f) => find_request_in(&f.items, id),
        _ => None,
    })
}

fn find_request_in_mut<'a>(items: &'a mut [Item], id: &str) -> Option<&'a mut Request> {
    items.iter_mut().find_map(|i| match i {
        Item::Request(r) if r.id == id => Some(r),
        Item::Folder(f) => find_request_in_mut(&mut f.items, id),
        _ => None,
    })
}

fn find_item_in<'a>(items: &'a [Item], id: &str) -> Option<&'a Item> {
    items.iter().find_map(|i| match i {
        _ if i.id() == id => Some(i),
        Item::Folder(f) => find_item_in(&f.items, id),
        _ => None,
    })
}

fn parent_in(container: &str, items: &[Item], id: &str) -> Option<(String, usize)> {
    if let Some(pos) = items.iter().position(|i| i.id() == id) {
        return Some((container.to_string(), pos));
    }
    items.iter().find_map(|i| match i {
        Item::Folder(f) => parent_in(&f.id, &f.items, id),
        _ => None,
    })
}

fn contains_item(items: &[Item], id: &str) -> bool {
    items.iter().any(|i| {
        i.id() == id
            || match i {
                Item::Folder(f) => contains_item(&f.items, id),
                _ => false,
            }
    })
}

fn folder_items_mut<'a>(items: &'a mut [Item], id: &str) -> Option<&'a mut Vec<Item>> {
    for i in items.iter_mut() {
        if let Item::Folder(f) = i {
            if f.id == id {
                return Some(&mut f.items);
            }
            if let Some(found) = folder_items_mut(&mut f.items, id) {
                return Some(found);
            }
        }
    }
    None
}

fn remove_from(items: &mut Vec<Item>, id: &str) -> Option<Item> {
    if let Some(pos) = items.iter().position(|i| i.id() == id) {
        return Some(items.remove(pos));
    }
    items.iter_mut().find_map(|i| match i {
        Item::Folder(f) => remove_from(&mut f.items, id),
        _ => None,
    })
}

fn rename_in(items: &mut [Item], id: &str, name: &str) -> bool {
    for i in items.iter_mut() {
        match i {
            Item::Request(r) if r.id == id => {
                r.name = name.into();
                return true;
            }
            Item::Folder(f) => {
                if f.id == id {
                    f.name = name.into();
                    return true;
                }
                if rename_in(&mut f.items, id, name) {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

fn duplicate_in(items: &mut Vec<Item>, id: &str) -> Option<String> {
    if let Some(pos) = items.iter().position(|i| matches!(i, Item::Request(r) if r.id == id)) {
        if let Item::Request(r) = &items[pos] {
            let mut copy = r.clone();
            copy.id = new_id();
            copy.name = format!("{} (copy)", r.name);
            let new_id = copy.id.clone();
            items.insert(pos + 1, Item::Request(copy));
            return Some(new_id);
        }
    }
    items.iter_mut().find_map(|i| match i {
        Item::Folder(f) => duplicate_in(&mut f.items, id),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One collection: [a, f1 [b, f2 [c]], d], plus an empty second collection.
    fn tree() -> Catalog {
        let req = |n: &str| {
            let mut r = Request::new(n, Protocol::Rest);
            r.id = n.into();
            Item::Request(r)
        };
        let folder = |n: &str, items: Vec<Item>| Item::Folder(Folder { id: n.into(), name: n.into(), items });
        let mut c = Collection::new("one");
        c.id = "c1".into();
        c.items = vec![req("a"), folder("f1", vec![req("b"), folder("f2", vec![req("c")])]), req("d")];
        let mut c2 = Collection::new("two");
        c2.id = "c2".into();
        Catalog { collections: vec![c, c2], ..Default::default() }
    }

    fn ids(items: &[Item]) -> Vec<&str> {
        items.iter().map(|i| i.id()).collect()
    }

    #[test]
    fn moves_between_siblings_folders_and_collections() {
        let mut cat = tree();
        assert!(cat.move_item("d", &MoveTo::Before("a".into())));
        assert_eq!(ids(&cat.collections[0].items), ["d", "a", "f1"]);
        assert!(cat.move_item("d", &MoveTo::After("a".into())));
        assert_eq!(ids(&cat.collections[0].items), ["a", "d", "f1"]);
        assert!(cat.move_item("a", &MoveTo::Into("f2".into())));
        assert_eq!(cat.parent_of("a"), Some(("f2".into(), 1)));
        assert!(cat.move_item("f1", &MoveTo::Into("c2".into())));
        assert_eq!(ids(&cat.collections[0].items), ["d"]);
        assert_eq!(ids(&cat.collections[1].items), ["f1"]);
    }

    #[test]
    fn refuses_moving_a_folder_into_itself() {
        let mut cat = tree();
        let before = format!("{cat:?}");
        assert!(!cat.move_item("f1", &MoveTo::Into("f1".into())));
        assert!(!cat.move_item("f1", &MoveTo::Into("f2".into())));
        assert!(!cat.move_item("f1", &MoveTo::Before("c".into())));
        assert!(!cat.move_item("a", &MoveTo::Into("b".into())), "a request is not a container");
        assert!(!cat.move_item("a", &MoveTo::Into("missing".into())));
        assert_eq!(before, format!("{cat:?}"));
    }

    #[test]
    fn merge_into_appends_items_and_missing_variables() {
        let mut cat = tree();
        cat.collections[1].variables.push(KeyValue::new("host", "kept"));
        let mut incoming = Collection::new("imported");
        incoming.variables = vec![KeyValue::new("host", "ignored"), KeyValue::new("token", "t")];
        incoming.items.push(Item::Request(Request::new("x", Protocol::Rest)));
        cat.merge_into(Catalog { collections: vec![incoming], ..Default::default() }, "c2");
        assert_eq!(cat.collections.len(), 2);
        let c2 = &cat.collections[1];
        assert_eq!(c2.items.len(), 1);
        assert_eq!(c2.variables.iter().map(|v| (v.key.as_str(), v.value.as_str())).collect::<Vec<_>>(), [("host", "kept"), ("token", "t")]);
    }
}

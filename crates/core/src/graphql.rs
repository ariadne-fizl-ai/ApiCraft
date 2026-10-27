//! GraphQL over HTTP: the request body (or GET parameters) for a query,
//! schema introspection, query skeletons built from the schema, error
//! messages from responses, and import of an introspection result as a
//! collection with one request per query and mutation.

use serde_json::{json, Map, Value};

use crate::model::*;

/// The standard introspection query (types, fields, arguments, enums and
/// input objects; directives are left out).
pub const INTROSPECTION_QUERY: &str = "query IntrospectionQuery {
  __schema {
    queryType { name }
    mutationType { name }
    subscriptionType { name }
    types { ...FullType }
  }
}
fragment FullType on __Type {
  kind name description
  fields(includeDeprecated: true) { name description args { ...InputValue } type { ...TypeRef } }
  inputFields { ...InputValue }
  interfaces { ...TypeRef }
  enumValues(includeDeprecated: true) { name }
  possibleTypes { ...TypeRef }
}
fragment InputValue on __InputValue { name description type { ...TypeRef } defaultValue }
fragment TypeRef on __Type {
  kind name
  ofType { kind name ofType { kind name ofType { kind name ofType { kind name ofType { kind name ofType { kind name } } } } } }
}";

/// The JSON body of a POST: `{"query", "variables", "operationName"}`.
/// `variables` is the text from the editor; it must be a JSON object.
pub fn request_body(query: &str, variables: &str, operation_name: &str) -> Result<String, String> {
    let mut body = Map::new();
    body.insert("query".into(), json!(query));
    if let Some(v) = parse_variables(variables)? {
        body.insert("variables".into(), v);
    }
    if !operation_name.trim().is_empty() {
        body.insert("operationName".into(), json!(operation_name.trim()));
    }
    Ok(Value::Object(body).to_string())
}

/// Query parameters for GraphQL over GET.
pub fn get_params(query: &str, variables: &str, operation_name: &str) -> Result<Vec<(String, String)>, String> {
    let mut out = vec![("query".to_string(), query.to_string())];
    if let Some(v) = parse_variables(variables)? {
        out.push(("variables".into(), v.to_string()));
    }
    if !operation_name.trim().is_empty() {
        out.push(("operationName".into(), operation_name.trim().to_string()));
    }
    Ok(out)
}

fn parse_variables(text: &str) -> Result<Option<Value>, String> {
    if text.trim().is_empty() {
        return Ok(None);
    }
    match serde_json::from_str::<Value>(text) {
        Ok(v @ Value::Object(_)) => Ok(Some(v)),
        Ok(Value::Null) => Ok(None),
        Ok(_) => Err("GraphQL variables must be a JSON object".into()),
        Err(e) => Err(format!("GraphQL variables are not valid JSON: {e}")),
    }
}

/// The operations a document defines, as `(kind, name)`. An anonymous
/// operation has an empty name.
pub fn operations(document: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut words = Tokens::new(document);
    while let Some(tok) = words.next() {
        match tok {
            "{" => {
                if depth == 0 && out.is_empty() && !words.saw_keyword {
                    // A bare `{ ... }` is an anonymous query.
                    out.push(("query".to_string(), String::new()));
                }
                depth += 1;
            }
            "}" => depth -= 1,
            "query" | "mutation" | "subscription" if depth == 0 => {
                words.saw_keyword = true;
                let name = match words.peek() {
                    Some(n) if n.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_') => n.to_string(),
                    _ => String::new(),
                };
                out.push((tok.to_string(), name));
            }
            "fragment" if depth == 0 => words.saw_keyword = true,
            _ => {}
        }
    }
    out
}

/// A minimal GraphQL tokenizer: names, punctuation, and strings and
/// comments skipped.
struct Tokens<'a> {
    s: &'a str,
    i: usize,
    saw_keyword: bool,
}

impl<'a> Tokens<'a> {
    fn new(s: &'a str) -> Self {
        Tokens { s, i: 0, saw_keyword: false }
    }
    fn peek(&self) -> Option<&'a str> {
        let mut t = Tokens { s: self.s, i: self.i, saw_keyword: false };
        t.next()
    }
}

impl<'a> Iterator for Tokens<'a> {
    type Item = &'a str;
    fn next(&mut self) -> Option<&'a str> {
        let b = self.s.as_bytes();
        loop {
            while self.i < b.len() && (b[self.i].is_ascii_whitespace() || b[self.i] == b',') {
                self.i += 1;
            }
            if self.i >= b.len() {
                return None;
            }
            match b[self.i] {
                b'#' => {
                    while self.i < b.len() && b[self.i] != b'\n' {
                        self.i += 1;
                    }
                }
                b'"' => {
                    let block = self.s[self.i..].starts_with("\"\"\"");
                    if block {
                        self.i += 3;
                        match self.s[self.i..].find("\"\"\"") {
                            Some(e) => self.i += e + 3,
                            None => self.i = b.len(),
                        }
                    } else {
                        self.i += 1;
                        while self.i < b.len() && b[self.i] != b'"' && b[self.i] != b'\n' {
                            if b[self.i] == b'\\' {
                                self.i += 1;
                            }
                            self.i += 1;
                        }
                        self.i = (self.i + 1).min(b.len());
                    }
                }
                c if c.is_ascii_alphanumeric() || c == b'_' => {
                    let start = self.i;
                    while self.i < b.len() && (b[self.i].is_ascii_alphanumeric() || b[self.i] == b'_') {
                        self.i += 1;
                    }
                    return Some(&self.s[start..self.i]);
                }
                _ => {
                    let start = self.i;
                    // Step over a whole character so slicing stays on boundaries.
                    self.i += self.s[self.i..].chars().next().map(char::len_utf8).unwrap_or(1);
                    return Some(&self.s[start..self.i]);
                }
            }
        }
    }
}

/// The `errors` of a GraphQL response, as one line for display.
pub fn error_message(body: &str) -> Option<String> {
    let v: Value = serde_json::from_str(body).ok()?;
    let errors = v.get("errors")?.as_array()?;
    let first = errors.first()?;
    let msg = first.get("message").and_then(|m| m.as_str()).unwrap_or("error");
    let path =
        first.get("path").and_then(|p| p.as_array()).map(|p| p.iter().map(|s| s.to_string().trim_matches('"').to_string()).collect::<Vec<_>>().join("."));
    let mut out = match path {
        Some(p) if !p.is_empty() => format!("{msg} (at {p})"),
        _ => msg.to_string(),
    };
    if errors.len() > 1 {
        out.push_str(&format!(" \u{2014} and {} more error(s)", errors.len() - 1));
    }
    Some(out)
}

// ---------------------------------------------------------------- schema

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Schema {
    pub query_type: Option<String>,
    pub mutation_type: Option<String>,
    pub subscription_type: Option<String>,
    pub types: Vec<TypeDef>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct TypeDef {
    pub name: String,
    /// `OBJECT`, `INPUT_OBJECT`, `ENUM`, `SCALAR`, `INTERFACE` or `UNION`.
    pub kind: String,
    pub description: String,
    pub fields: Vec<Field>,
    pub input_fields: Vec<InputValue>,
    pub enum_values: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Field {
    pub name: String,
    pub description: String,
    pub args: Vec<InputValue>,
    pub ty: TypeRef,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct InputValue {
    pub name: String,
    pub ty: TypeRef,
    pub default: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct TypeRef {
    /// `NON_NULL`, `LIST` or the named type's kind.
    pub kind: String,
    pub name: Option<String>,
    pub of: Option<Box<TypeRef>>,
}

impl TypeRef {
    /// SDL notation, e.g. `[User!]!`.
    pub fn display(&self) -> String {
        match (self.kind.as_str(), &self.of) {
            ("NON_NULL", Some(of)) => format!("{}!", of.display()),
            ("LIST", Some(of)) => format!("[{}]", of.display()),
            _ => self.name.clone().unwrap_or_default(),
        }
    }
    /// The named type at the bottom of any list/non-null wrappers.
    pub fn named(&self) -> &str {
        match &self.of {
            Some(of) if self.name.is_none() => of.named(),
            _ => self.name.as_deref().unwrap_or(""),
        }
    }
    pub fn required(&self) -> bool {
        self.kind == "NON_NULL"
    }
}

/// What a root field is run as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpKind {
    Query,
    Mutation,
    Subscription,
}

impl OpKind {
    pub fn keyword(self) -> &'static str {
        match self {
            OpKind::Query => "query",
            OpKind::Mutation => "mutation",
            OpKind::Subscription => "subscription",
        }
    }
    pub fn plural(self) -> &'static str {
        match self {
            OpKind::Query => "Queries",
            OpKind::Mutation => "Mutations",
            OpKind::Subscription => "Subscriptions",
        }
    }
}

impl Schema {
    pub fn type_def(&self, name: &str) -> Option<&TypeDef> {
        self.types.iter().find(|t| t.name == name)
    }

    /// The root types that exist, in query, mutation, subscription order.
    pub fn roots(&self) -> Vec<(OpKind, &TypeDef)> {
        [(OpKind::Query, &self.query_type), (OpKind::Mutation, &self.mutation_type), (OpKind::Subscription, &self.subscription_type)]
            .into_iter()
            .filter_map(|(k, n)| n.as_deref().and_then(|n| self.type_def(n)).map(|t| (k, t)))
            .collect()
    }

    /// The types a person would browse: not the `__` introspection types.
    pub fn user_types(&self) -> impl Iterator<Item = &TypeDef> {
        self.types.iter().filter(|t| !t.name.starts_with("__"))
    }
}

/// Reads an introspection result: `{"data": {"__schema": ...}}`, or the
/// `__schema` object with or without its wrapper.
pub fn parse_schema(v: &Value) -> Result<Schema, String> {
    let schema =
        v.pointer("/data/__schema").or_else(|| v.get("__schema")).or_else(|| v.get("types").is_some().then_some(v)).ok_or_else(|| {
            match error_message(&v.to_string()) {
                Some(e) => format!("the server refused introspection: {e}"),
                None => "the response has no __schema (is introspection turned off?)".to_string(),
            }
        })?;
    let root = |k: &str| schema.get(k).and_then(|t| t.get("name")).and_then(|n| n.as_str()).map(String::from);
    let types = schema.get("types").and_then(|t| t.as_array()).ok_or("__schema has no types")?;
    Ok(Schema {
        query_type: root("queryType"),
        mutation_type: root("mutationType"),
        subscription_type: root("subscriptionType"),
        types: types.iter().map(type_def).collect(),
    })
}

/// True for JSON that holds an introspection result.
pub fn is_introspection(v: &Value) -> bool {
    v.pointer("/data/__schema/types").is_some() || v.pointer("/__schema/types").is_some()
}

fn text(v: &Value, k: &str) -> String {
    v.get(k).and_then(|s| s.as_str()).unwrap_or_default().to_string()
}

fn type_def(v: &Value) -> TypeDef {
    let list = |k: &str| v.get(k).and_then(|a| a.as_array()).cloned().unwrap_or_default();
    TypeDef {
        name: text(v, "name"),
        kind: text(v, "kind"),
        description: text(v, "description"),
        fields: list("fields")
            .iter()
            .map(|f| Field {
                name: text(f, "name"),
                description: text(f, "description"),
                args: f.get("args").and_then(|a| a.as_array()).map(|a| a.iter().map(input_value).collect()).unwrap_or_default(),
                ty: type_ref(f.get("type")),
            })
            .collect(),
        input_fields: list("inputFields").iter().map(input_value).collect(),
        enum_values: list("enumValues").iter().map(|e| text(e, "name")).collect(),
    }
}

fn input_value(v: &Value) -> InputValue {
    InputValue { name: text(v, "name"), ty: type_ref(v.get("type")), default: v.get("defaultValue").and_then(|d| d.as_str()).map(String::from) }
}

fn type_ref(v: Option<&Value>) -> TypeRef {
    let Some(v) = v.filter(|v| v.is_object()) else { return TypeRef::default() };
    TypeRef {
        kind: text(v, "kind"),
        name: v.get("name").and_then(|n| n.as_str()).map(String::from),
        of: v.get("ofType").filter(|o| o.is_object()).map(|o| Box::new(type_ref(Some(o)))),
    }
}

// ---------------------------------------------------------------- query skeletons

/// A ready-to-run operation for one root field: the document, with a
/// variable per argument, and example variables for the required ones.
pub fn operation(schema: &Schema, kind: OpKind, field: &Field) -> (String, String) {
    let op_name = pascal(&field.name);
    let mut doc = format!("{} {op_name}", kind.keyword());
    if !field.args.is_empty() {
        let defs: Vec<String> = field.args.iter().map(|a| format!("${}: {}", a.name, a.ty.display())).collect();
        doc.push_str(&format!("({})", defs.join(", ")));
    }
    doc.push_str(" {\n  ");
    doc.push_str(&field.name);
    if !field.args.is_empty() {
        let args: Vec<String> = field.args.iter().map(|a| format!("{0}: ${0}", a.name)).collect();
        doc.push_str(&format!("({})", args.join(", ")));
    }
    let selection = selection_set(schema, field.ty.named(), 2, 2);
    if !selection.is_empty() {
        doc.push(' ');
        doc.push_str(&selection);
    }
    doc.push_str("\n}\n");

    let vars: Map<String, Value> = field.args.iter().filter(|a| a.ty.required()).map(|a| (a.name.clone(), sample(schema, &a.ty, 0))).collect();
    let vars = if vars.is_empty() { String::new() } else { serde_json::to_string_pretty(&Value::Object(vars)).unwrap_or_default() };
    (doc, vars)
}

/// `{ ... }` for a named output type: its scalar and enum fields, and
/// object fields (that need no arguments) down to `depth` levels.
fn selection_set(schema: &Schema, type_name: &str, depth: usize, indent: usize) -> String {
    let Some(t) = schema.type_def(type_name) else { return String::new() };
    match t.kind.as_str() {
        "OBJECT" | "INTERFACE" => {}
        "UNION" => return format!("{{\n{}__typename\n{}}}", "  ".repeat(indent), "  ".repeat(indent - 1)),
        _ => return String::new(),
    }
    let pad = "  ".repeat(indent);
    let mut lines = Vec::new();
    for f in t.fields.iter().filter(|f| !f.args.iter().any(|a| a.ty.required())) {
        let inner = schema.type_def(f.ty.named()).map(|t| t.kind.as_str()).unwrap_or("SCALAR");
        match inner {
            "SCALAR" | "ENUM" => lines.push(format!("{pad}{}", f.name)),
            _ if depth > 1 => {
                let sub = selection_set(schema, f.ty.named(), depth - 1, indent + 1);
                if !sub.is_empty() {
                    lines.push(format!("{pad}{} {sub}", f.name));
                }
            }
            _ => {}
        }
        if lines.len() >= 25 {
            break;
        }
    }
    if lines.is_empty() {
        lines.push(format!("{pad}__typename"));
    }
    format!("{{\n{}\n{}}}", lines.join("\n"), "  ".repeat(indent - 1))
}

/// An example value for an input type.
fn sample(schema: &Schema, ty: &TypeRef, depth: usize) -> Value {
    match (ty.kind.as_str(), &ty.of) {
        ("NON_NULL", Some(of)) => return sample(schema, of, depth),
        ("LIST", Some(of)) => return json!([sample(schema, of, depth)]),
        _ => {}
    }
    let name = ty.named();
    match name {
        "Int" => return json!(0),
        "Float" => return json!(0.0),
        "Boolean" => return json!(false),
        "String" | "ID" => return json!(""),
        _ => {}
    }
    match schema.type_def(name) {
        Some(t) if t.kind == "ENUM" => t.enum_values.first().map(|v| json!(v)).unwrap_or(Value::Null),
        Some(t) if t.kind == "INPUT_OBJECT" && depth < 4 => {
            Value::Object(t.input_fields.iter().filter(|f| f.ty.required()).map(|f| (f.name.clone(), sample(schema, &f.ty, depth + 1))).collect())
        }
        Some(t) if t.kind == "INPUT_OBJECT" => json!({}),
        _ => json!(""),
    }
}

fn pascal(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

// ---------------------------------------------------------------- requests from a schema

/// One GraphQL request per query and mutation, in a folder per kind.
/// Subscriptions are left out: they need a WebSocket.
pub fn request_folders(schema: &Schema, url: &str) -> Vec<Item> {
    let mut out = Vec::new();
    for (kind, root) in schema.roots() {
        if kind == OpKind::Subscription {
            continue;
        }
        let mut folder = Folder::new(kind.plural());
        for f in &root.fields {
            let (query, variables) = operation(schema, kind, f);
            let mut r = Request::new(f.name.clone(), Protocol::GraphQl);
            r.url = url.to_string();
            r.graphql = GraphQlOptions { query, variables, operation_name: String::new() };
            r.description = f.description.clone();
            r.checks.push(Check { kind: CheckKind::Status, target: String::new(), expected: "200".into(), enabled: true });
            r.checks.push(Check { kind: CheckKind::JsonEquals, target: format!("$.data.{}", f.name), expected: String::new(), enabled: true });
            folder.items.push(Item::Request(r));
        }
        if !folder.items.is_empty() {
            out.push(Item::Folder(folder));
        }
    }
    out
}

#[derive(Debug)]
pub struct GraphQlImport {
    pub collection: Collection,
    pub operations: usize,
}

/// A collection from an introspection result. The endpoint is the
/// `{{graphqlUrl}}` collection variable, set to `url` when known.
pub fn import(v: &Value, url: Option<&str>) -> Result<GraphQlImport, String> {
    let schema = parse_schema(v)?;
    let mut col = Collection::new("GraphQL API");
    col.variables.push(KeyValue::new("graphqlUrl", url.unwrap_or("https://example.com/graphql")));
    col.items = request_folders(&schema, "{{graphqlUrl}}");
    let operations = col.items.iter().map(|i| if let Item::Folder(f) = i { f.items.len() } else { 0 }).sum();
    if operations == 0 {
        return Err("the schema has no queries or mutations".into());
    }
    Ok(GraphQlImport { collection: col, operations })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema() -> Schema {
        let v = json!({"data": {"__schema": {
            "queryType": {"name": "Query"}, "mutationType": {"name": "Mutation"}, "subscriptionType": null,
            "types": [
                {"kind": "OBJECT", "name": "Query", "fields": [
                    {"name": "user", "args": [{"name": "id", "type": {"kind": "NON_NULL", "name": null, "ofType": {"kind": "SCALAR", "name": "ID"}}}],
                     "type": {"kind": "OBJECT", "name": "User"}},
                    {"name": "users", "args": [{"name": "first", "type": {"kind": "SCALAR", "name": "Int"}}],
                     "type": {"kind": "NON_NULL", "name": null, "ofType": {"kind": "LIST", "name": null, "ofType": {"kind": "OBJECT", "name": "User"}}}}
                ]},
                {"kind": "OBJECT", "name": "Mutation", "fields": [
                    {"name": "addUser", "args": [{"name": "input", "type": {"kind": "NON_NULL", "name": null, "ofType": {"kind": "INPUT_OBJECT", "name": "NewUser"}}}],
                     "type": {"kind": "OBJECT", "name": "User"}}
                ]},
                {"kind": "OBJECT", "name": "User", "fields": [
                    {"name": "id", "args": [], "type": {"kind": "SCALAR", "name": "ID"}},
                    {"name": "role", "args": [], "type": {"kind": "ENUM", "name": "Role"}},
                    {"name": "friends", "args": [], "type": {"kind": "LIST", "name": null, "ofType": {"kind": "OBJECT", "name": "User"}}}
                ]},
                {"kind": "INPUT_OBJECT", "name": "NewUser", "inputFields": [
                    {"name": "name", "type": {"kind": "NON_NULL", "name": null, "ofType": {"kind": "SCALAR", "name": "String"}}},
                    {"name": "role", "type": {"kind": "NON_NULL", "name": null, "ofType": {"kind": "ENUM", "name": "Role"}}},
                    {"name": "age", "type": {"kind": "SCALAR", "name": "Int"}}
                ]},
                {"kind": "ENUM", "name": "Role", "enumValues": [{"name": "ADMIN"}, {"name": "USER"}]},
                {"kind": "SCALAR", "name": "ID"}, {"kind": "SCALAR", "name": "Int"}, {"kind": "SCALAR", "name": "String"}
            ]
        }}});
        parse_schema(&v).unwrap()
    }

    #[test]
    fn builds_operations_from_the_schema() {
        let s = schema();
        let q = s.roots()[0].1;
        assert_eq!(q.fields[1].ty.display(), "[User]!");
        let (doc, vars) = operation(&s, OpKind::Query, &q.fields[0]);
        assert_eq!(doc, "query User($id: ID!) {\n  user(id: $id) {\n    id\n    role\n    friends {\n      id\n      role\n    }\n  }\n}\n");
        assert_eq!(serde_json::from_str::<Value>(&vars).unwrap(), json!({"id": ""}));
        let (_, vars) = operation(&s, OpKind::Query, &q.fields[1]);
        assert_eq!(vars, "", "optional arguments get no example");
        let m = s.roots()[1].1;
        let (doc, vars) = operation(&s, OpKind::Mutation, &m.fields[0]);
        assert!(doc.starts_with("mutation AddUser($input: NewUser!) {\n  addUser(input: $input) {"));
        assert_eq!(serde_json::from_str::<Value>(&vars).unwrap(), json!({"input": {"name": "", "role": "ADMIN"}}));
    }

    #[test]
    fn imports_a_collection() {
        let v = json!({"data": {"__schema": {"queryType": {"name": "Query"}, "types": [
            {"kind": "OBJECT", "name": "Query", "fields": [{"name": "ping", "args": [], "type": {"kind": "SCALAR", "name": "String"}}]}
        ]}}});
        assert!(is_introspection(&v));
        let imp = import(&v, Some("https://api.test/graphql")).unwrap();
        assert_eq!(imp.operations, 1);
        let Item::Folder(f) = &imp.collection.items[0] else { panic!() };
        let Item::Request(r) = &f.items[0] else { panic!() };
        assert_eq!(r.protocol, Protocol::GraphQl);
        assert_eq!(r.graphql.query, "query Ping {\n  ping\n}\n");
    }

    #[test]
    fn bodies_and_errors() {
        let b: Value = serde_json::from_str(&request_body("{ a }", "{\"x\": 1}", "Op").unwrap()).unwrap();
        assert_eq!(b, json!({"query": "{ a }", "variables": {"x": 1}, "operationName": "Op"}));
        assert_eq!(request_body("{ a }", " ", "").unwrap(), r#"{"query":"{ a }"}"#);
        assert!(request_body("{ a }", "[1]", "").is_err());
        let e = error_message(r#"{"errors":[{"message":"Not found","path":["user",0]},{"message":"x"}],"data":null}"#).unwrap();
        assert_eq!(e, "Not found (at user.0) \u{2014} and 1 more error(s)");
        assert_eq!(error_message(r#"{"data":{}}"#), None);
    }

    #[test]
    fn lists_operations() {
        let doc = "# comment with query Fake\nquery A($x: Int) { a(s: \"mutation B\") { b } }\nmutation B { c }\nfragment F on T { d }";
        assert_eq!(operations(doc), vec![("query".into(), "A".into()), ("mutation".into(), "B".into())]);
        assert_eq!(operations("{ a }"), vec![("query".into(), String::new())]);
    }
}

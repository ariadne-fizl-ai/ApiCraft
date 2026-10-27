//! OData v2-v4: system query options and the protocol headers, error
//! messages from responses, and the service's `$metadata` (CSDL), which
//! feeds the query builder and imports as a collection with list, get,
//! create, update and delete requests per entity set.

use roxmltree::{Document, Node};
use serde_json::{json, Map, Value};

use crate::model::*;

/// The resource URL: the service root with the resource path under it.
pub fn resource_url(root: &str, resource: &str) -> String {
    let root = root.trim().trim_end_matches('/');
    let resource = resource.trim().trim_start_matches('/');
    if resource.is_empty() {
        root.to_string()
    } else {
        format!("{root}/{resource}")
    }
}

/// The `$metadata` URL for a service root.
pub fn metadata_url(root: &str) -> String {
    let root = root.trim().split(['?', '#']).next().unwrap_or_default().trim_end_matches('/');
    if root.ends_with("/$metadata") {
        root.to_string()
    } else {
        format!("{root}/$metadata")
    }
}

/// The system query options that are set, unencoded, in the usual order.
pub fn query_options(o: &ODataOptions) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut opt = |k: &str, v: &str| {
        if !v.trim().is_empty() {
            out.push((k.to_string(), v.trim().to_string()));
        }
    };
    opt("$select", &o.select);
    opt("$expand", &o.expand);
    opt("$filter", &o.filter);
    if o.version == ODataVersion::V4 {
        opt("$search", &o.search);
    }
    opt("$orderby", &o.orderby);
    opt("$top", &o.top);
    opt("$skip", &o.skip);
    if o.count {
        match o.version {
            ODataVersion::V4 => out.push(("$count".into(), "true".into())),
            _ => out.push(("$inlinecount".into(), "allpages".into())),
        }
    }
    out
}

/// A query string that keeps OData's punctuation readable: only what would
/// break the URL (`&`, `#`, `+`, `%`, spaces, non-ASCII) is escaped.
pub fn encode_query(pairs: &[(String, String)]) -> String {
    pairs.iter().map(|(k, v)| format!("{}={}", encode(k), encode(v))).collect::<Vec<_>>().join("&")
}

fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' => out.push(b as char),
            b'-' | b'_' | b'.' | b'~' | b'$' | b'\'' | b'(' | b')' | b',' | b':' | b'/' | b'@' | b'*' | b'!' | b';' | b'=' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// The protocol headers for a version: what it speaks and that it wants JSON.
pub fn headers(version: ODataVersion) -> [(&'static str, &'static str); 3] {
    match version {
        ODataVersion::V4 => [("OData-Version", "4.0"), ("OData-MaxVersion", "4.0"), ("Accept", "application/json")],
        ODataVersion::V3 => [("DataServiceVersion", "3.0"), ("MaxDataServiceVersion", "3.0"), ("Accept", "application/json")],
        ODataVersion::V2 => [("DataServiceVersion", "2.0"), ("MaxDataServiceVersion", "2.0"), ("Accept", "application/json")],
    }
}

/// The message of an OData error response, JSON (any version) or XML.
pub fn error_message(body: &str) -> Option<String> {
    let t = body.trim_start();
    if t.starts_with('<') {
        let doc = Document::parse(t).ok()?;
        let root = doc.root_element();
        if root.tag_name().name() != "error" {
            return None;
        }
        return root.descendants().find(|n| n.tag_name().name() == "message").and_then(|n| n.text()).map(|s| s.trim().to_string());
    }
    let v: Value = serde_json::from_str(t).ok()?;
    let err = v.get("error").or_else(|| v.get("odata.error"))?;
    let msg = err.get("message")?;
    let msg = msg.as_str().map(String::from).or_else(|| msg.get("value").and_then(|m| m.as_str()).map(String::from))?;
    let code = err.get("code").and_then(|c| c.as_str()).unwrap_or_default();
    Some(if code.is_empty() { msg } else { format!("{msg} ({code})") })
}

// ---------------------------------------------------------------- $metadata

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Metadata {
    pub version: ODataVersion,
    pub entity_sets: Vec<EntitySet>,
    pub singletons: Vec<EntitySet>,
    pub operations: Vec<Operation>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct EntitySet {
    pub name: String,
    pub type_name: String,
    pub keys: Vec<Property>,
    pub properties: Vec<Property>,
    pub navigation: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Property {
    pub name: String,
    /// `Edm.String`, `Collection(NS.Type)`, ...
    pub ty: String,
    pub nullable: bool,
}

/// A function or action import (v4), or a function import (v2/v3).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Operation {
    pub name: String,
    pub is_action: bool,
    pub params: Vec<Property>,
    /// v2's `m:HttpMethod`.
    pub http_method: Option<String>,
}

impl Metadata {
    pub fn entity_set(&self, name: &str) -> Option<&EntitySet> {
        self.entity_sets.iter().chain(self.singletons.iter()).find(|s| s.name == name)
    }
}

/// True for a CSDL document (`<edmx:Edmx>`).
pub fn is_metadata(xml: &str) -> bool {
    Document::parse(xml).map(|d| d.root_element().tag_name().name() == "Edmx").unwrap_or(false)
}

/// Reads a `$metadata` document. Namespaces are matched by local name so
/// every CSDL version reads the same way.
pub fn parse_metadata(xml: &str) -> Result<Metadata, String> {
    let doc = Document::parse(xml).map_err(|e| format!("$metadata is not valid XML: {e}"))?;
    let root = doc.root_element();
    if root.tag_name().name() != "Edmx" {
        return Err("not an OData $metadata document (expected <edmx:Edmx>)".into());
    }
    let version = if root.attribute("Version").is_some_and(|v| v.starts_with('4')) {
        ODataVersion::V4
    } else {
        let ds = root.children().find(|n| n.tag_name().name() == "DataServices");
        let declared = ds.and_then(|d| d.attributes().find(|a| a.name() == "MaxDataServiceVersion" || a.name() == "DataServiceVersion").map(|a| a.value()));
        if declared.is_some_and(|v| v.starts_with('3')) {
            ODataVersion::V3
        } else {
            ODataVersion::V2
        }
    };

    let schemas: Vec<Node> = root.descendants().filter(|n| n.tag_name().name() == "Schema").collect();
    let is = |n: &Node, name: &str| n.is_element() && n.tag_name().name() == name;
    // Entity types by qualified name (and by alias, for v4 schemas that declare one).
    let mut types: Vec<(String, Node)> = Vec::new();
    for s in &schemas {
        let ns = s.attribute("Namespace").unwrap_or_default();
        let alias = s.attribute("Alias");
        for t in s.children().filter(|n| is(n, "EntityType")) {
            let name = t.attribute("Name").unwrap_or_default();
            types.push((format!("{ns}.{name}"), t));
            if let Some(a) = alias {
                types.push((format!("{a}.{name}"), t));
            }
        }
    }
    let find_type = |q: &str| types.iter().find(|(n, _)| n == q).map(|(_, t)| *t);

    let entity = |set_name: &str, type_name: &str| -> EntitySet {
        let mut set = EntitySet { name: set_name.to_string(), type_name: type_name.to_string(), ..Default::default() };
        // Walk up the BaseType chain; keys and properties come from every level.
        let mut chain = Vec::new();
        let mut next = find_type(type_name);
        while let Some(t) = next.filter(|_| chain.len() < 8) {
            chain.push(t);
            next = t.attribute("BaseType").and_then(find_type);
        }
        let mut key_names = Vec::new();
        for t in chain.iter().rev() {
            for k in t.children().filter(|n| is(n, "Key")).flat_map(|k| k.children().filter(|n| is(n, "PropertyRef"))) {
                key_names.extend(k.attribute("Name").map(String::from));
            }
            for p in t.children().filter(|n| is(n, "Property")) {
                set.properties.push(Property {
                    name: p.attribute("Name").unwrap_or_default().to_string(),
                    ty: p.attribute("Type").unwrap_or_default().to_string(),
                    nullable: p.attribute("Nullable") != Some("false"),
                });
            }
            for n in t.children().filter(|n| is(n, "NavigationProperty")) {
                set.navigation.extend(n.attribute("Name").map(String::from));
            }
        }
        set.keys = key_names.iter().filter_map(|k| set.properties.iter().find(|p| &p.name == k).cloned()).collect();
        set
    };

    // v4 function/action definitions, for their parameters.
    let params_of = |n: Node| -> Vec<Property> {
        n.children()
            .filter(|c| is(c, "Parameter"))
            .map(|p| Property {
                name: p.attribute("Name").unwrap_or_default().to_string(),
                ty: p.attribute("Type").unwrap_or_default().to_string(),
                nullable: p.attribute("Nullable") != Some("false"),
            })
            .collect()
    };
    let definition = |kind: &str, qname: &str| -> Option<Node> {
        let (ns, name) = qname.rsplit_once('.')?;
        schemas
            .iter()
            .filter(|s| s.attribute("Namespace") == Some(ns) || s.attribute("Alias") == Some(ns))
            .flat_map(|s| s.children())
            .find(|c| is(c, kind) && c.attribute("Name") == Some(name) && c.attribute("IsBound") != Some("true"))
    };

    let mut md = Metadata { version, ..Default::default() };
    for container in schemas.iter().flat_map(|s| s.children().filter(|n| is(n, "EntityContainer"))) {
        for c in container.children().filter(|n| n.is_element()) {
            let name = c.attribute("Name").unwrap_or_default();
            match c.tag_name().name() {
                "EntitySet" => md.entity_sets.push(entity(name, c.attribute("EntityType").unwrap_or_default())),
                "Singleton" => md.singletons.push(entity(name, c.attribute("Type").unwrap_or_default())),
                "FunctionImport" if version == ODataVersion::V4 => {
                    let params = c.attribute("Function").and_then(|f| definition("Function", f)).map(params_of).unwrap_or_default();
                    md.operations.push(Operation { name: name.into(), is_action: false, params, http_method: None });
                }
                "ActionImport" => {
                    let params = c.attribute("Action").and_then(|f| definition("Action", f)).map(params_of).unwrap_or_default();
                    md.operations.push(Operation { name: name.into(), is_action: true, params, http_method: None });
                }
                "FunctionImport" => {
                    let http_method = c.attributes().find(|a| a.name() == "HttpMethod").map(|a| a.value().to_uppercase());
                    let is_action = c.attribute("IsSideEffecting") == Some("true") && http_method.is_none();
                    md.operations.push(Operation { name: name.into(), is_action, params: params_of(c), http_method });
                }
                _ => {}
            }
        }
    }
    if md.entity_sets.is_empty() && md.singletons.is_empty() && md.operations.is_empty() {
        return Err("$metadata has no entity sets, singletons or operations".into());
    }
    Ok(md)
}

// ---------------------------------------------------------------- examples

/// A key predicate with example values: `('id')`, `(1)`, or `(A=1,B='x')`.
pub fn key_predicate(set: &EntitySet, version: ODataVersion) -> String {
    match set.keys.as_slice() {
        [] => "('id')".into(),
        [k] => format!("({})", literal(&k.ty, version)),
        keys => format!("({})", keys.iter().map(|k| format!("{}={}", k.name, literal(&k.ty, version))).collect::<Vec<_>>().join(",")),
    }
}

/// An example literal of an Edm type, as written in a URL.
fn literal(ty: &str, version: ODataVersion) -> String {
    let v4 = version == ODataVersion::V4;
    match ty {
        "Edm.Int16" | "Edm.Int32" | "Edm.Byte" | "Edm.SByte" => "1".into(),
        "Edm.Int64" if v4 => "1".into(),
        "Edm.Int64" => "1L".into(),
        "Edm.Decimal" if v4 => "1.0".into(),
        "Edm.Decimal" => "1.0M".into(),
        "Edm.Double" | "Edm.Single" => "1.0".into(),
        "Edm.Boolean" => "true".into(),
        "Edm.Guid" if v4 => "00000000-0000-0000-0000-000000000000".into(),
        "Edm.Guid" => "guid'00000000-0000-0000-0000-000000000000'".into(),
        "Edm.Date" => "2026-01-01".into(),
        "Edm.DateTimeOffset" if v4 => "2026-01-01T00:00:00Z".into(),
        "Edm.DateTime" => "datetime'2026-01-01T00:00:00'".into(),
        _ => "'id'".into(),
    }
}

/// An example JSON value of an Edm type, for request bodies.
fn json_sample(ty: &str) -> Value {
    match ty {
        "Edm.Int16" | "Edm.Int32" | "Edm.Int64" | "Edm.Byte" | "Edm.SByte" => json!(0),
        "Edm.Decimal" | "Edm.Double" | "Edm.Single" => json!(0.0),
        "Edm.Boolean" => json!(false),
        "Edm.Guid" => json!("00000000-0000-0000-0000-000000000000"),
        "Edm.Date" => json!("2026-01-01"),
        "Edm.DateTimeOffset" | "Edm.DateTime" => json!("2026-01-01T00:00:00Z"),
        "Edm.String" => json!(""),
        t if t.starts_with("Collection(") => json!([]),
        _ => Value::Null,
    }
}

/// An example entity for create and update requests: its primitive
/// properties (complex and navigation properties are left out).
pub fn example_entity(set: &EntitySet) -> String {
    let obj: Map<String, Value> = set
        .properties
        .iter()
        .filter(|p| p.ty.starts_with("Edm.") || p.ty.starts_with("Collection(Edm."))
        .map(|p| (p.name.clone(), json_sample(&p.ty)))
        .collect();
    serde_json::to_string_pretty(&Value::Object(obj)).unwrap_or_default()
}

// ---------------------------------------------------------------- requests from $metadata

fn odata_request(name: String, root: &str, version: ODataVersion, method: &str, resource: String) -> Request {
    let mut r = Request::new(name, Protocol::OData);
    r.url = root.to_string();
    r.method = method.into();
    r.odata = ODataOptions { version, resource, ..Default::default() };
    r
}

fn status(r: &mut Request, expected: &str) {
    r.checks.push(Check { kind: CheckKind::Status, target: String::new(), expected: expected.into(), enabled: true });
}

/// A folder of requests per entity set (list, get, create, update, delete),
/// then singletons and operations.
pub fn request_folders(md: &Metadata, root: &str) -> Vec<Item> {
    let v = md.version;
    let update = match v {
        ODataVersion::V2 => "MERGE",
        _ => "PATCH",
    };
    let mut out = Vec::new();
    for set in &md.entity_sets {
        let mut f = Folder::new(set.name.clone());
        let mut list = odata_request(format!("List {}", set.name), root, v, "GET", set.name.clone());
        list.odata.top = "10".into();
        status(&mut list, "200");
        if v == ODataVersion::V4 {
            list.checks.push(Check { kind: CheckKind::JsonEquals, target: "$.value".into(), expected: String::new(), enabled: true });
        }
        let one = format!("{}{}", set.name, key_predicate(set, v));
        let mut get = odata_request(format!("Get {} by key", set.name), root, v, "GET", one.clone());
        status(&mut get, "200");
        let mut create = odata_request(format!("Create {}", set.name), root, v, "POST", set.name.clone());
        create.body = Body::Raw { content_type: "application/json".into(), text: example_entity(set) };
        status(&mut create, "2xx");
        let mut patch = odata_request(format!("Update {}", set.name), root, v, update, one.clone());
        patch.body = Body::Raw { content_type: "application/json".into(), text: example_entity(&EntitySet { keys: Vec::new(), ..set.clone() }) };
        status(&mut patch, "2xx");
        let mut delete = odata_request(format!("Delete {}", set.name), root, v, "DELETE", one);
        status(&mut delete, "2xx");
        f.items = [list, get, create, patch, delete].into_iter().map(Item::Request).collect();
        out.push(Item::Folder(f));
    }
    if !md.singletons.is_empty() {
        let mut f = Folder::new("Singletons");
        for s in &md.singletons {
            let mut r = odata_request(format!("Get {}", s.name), root, v, "GET", s.name.clone());
            status(&mut r, "200");
            f.items.push(Item::Request(r));
        }
        out.push(Item::Folder(f));
    }
    if !md.operations.is_empty() {
        let mut f = Folder::new("Operations");
        for op in &md.operations {
            f.items.push(Item::Request(operation_request(op, root, v)));
        }
        out.push(Item::Folder(f));
    }
    out
}

fn operation_request(op: &Operation, root: &str, v: ODataVersion) -> Request {
    let mut r = if op.is_action {
        let mut r = odata_request(op.name.clone(), root, v, "POST", op.name.clone());
        let obj: Map<String, Value> = op.params.iter().map(|p| (p.name.clone(), json_sample(&p.ty))).collect();
        r.body = Body::Raw { content_type: "application/json".into(), text: serde_json::to_string_pretty(&Value::Object(obj)).unwrap_or_default() };
        r
    } else if v == ODataVersion::V4 {
        let args: Vec<String> = op.params.iter().map(|p| format!("{}={}", p.name, literal(&p.ty, v))).collect();
        odata_request(op.name.clone(), root, v, "GET", format!("{}({})", op.name, args.join(",")))
    } else {
        // v2/v3 function imports take their parameters in the query string.
        let mut r = odata_request(op.name.clone(), root, v, op.http_method.as_deref().unwrap_or("GET"), op.name.clone());
        r.params = op.params.iter().map(|p| KeyValue::new(p.name.clone(), literal(&p.ty, v))).collect();
        r
    };
    status(&mut r, "2xx");
    r
}

#[derive(Debug)]
pub struct ODataImport {
    pub collection: Collection,
    pub entity_sets: usize,
    pub operations: usize,
}

/// A collection from a `$metadata` document. Requests use the
/// `{{odataRoot}}` collection variable, set from `source` (the URL the
/// document came from) when known.
pub fn import(xml: &str, source: Option<&str>) -> Result<ODataImport, String> {
    let md = parse_metadata(xml)?;
    let root = source.map(|s| metadata_url(s).trim_end_matches("/$metadata").to_string()).unwrap_or_else(|| "https://example.com/odata".into());
    let name = root.rsplit('/').find(|s| !s.is_empty() && !s.contains(':')).unwrap_or("OData service");
    let mut col = Collection::new(if source.is_some() { name.to_string() } else { "OData service".to_string() });
    col.variables.push(KeyValue::new("odataRoot", root));
    col.items = request_folders(&md, "{{odataRoot}}");
    Ok(ODataImport { collection: col, entity_sets: md.entity_sets.len(), operations: md.operations.len() })
}

#[cfg(test)]
mod tests {
    use super::*;

    const V4: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<edmx:Edmx Version="4.0" xmlns:edmx="http://docs.oasis-open.org/odata/ns/edmx">
  <edmx:DataServices>
    <Schema Namespace="Trippin" xmlns="http://docs.oasis-open.org/odata/ns/edm">
      <EntityType Name="Person">
        <Key><PropertyRef Name="UserName"/></Key>
        <Property Name="UserName" Type="Edm.String" Nullable="false"/>
        <Property Name="Age" Type="Edm.Int64"/>
        <Property Name="AddressInfo" Type="Collection(Trippin.Location)"/>
        <NavigationProperty Name="Friends" Type="Collection(Trippin.Person)"/>
      </EntityType>
      <EntityType Name="Employee" BaseType="Trippin.Person">
        <Property Name="Cost" Type="Edm.Decimal"/>
      </EntityType>
      <Function Name="GetNearestAirport">
        <Parameter Name="lat" Type="Edm.Double" Nullable="false"/>
        <Parameter Name="lon" Type="Edm.Double" Nullable="false"/>
        <ReturnType Type="Trippin.Airport"/>
      </Function>
      <Action Name="ResetDataSource"/>
      <EntityContainer Name="Container">
        <EntitySet Name="People" EntityType="Trippin.Person"/>
        <EntitySet Name="Employees" EntityType="Trippin.Employee"/>
        <Singleton Name="Me" Type="Trippin.Person"/>
        <FunctionImport Name="GetNearestAirport" Function="Trippin.GetNearestAirport"/>
        <ActionImport Name="ResetDataSource" Action="Trippin.ResetDataSource"/>
      </EntityContainer>
    </Schema>
  </edmx:DataServices>
</edmx:Edmx>"#;

    const V2: &str = r#"<edmx:Edmx Version="1.0" xmlns:edmx="http://schemas.microsoft.com/ado/2007/06/edmx">
  <edmx:DataServices m:DataServiceVersion="2.0" xmlns:m="http://schemas.microsoft.com/ado/2007/08/dataservices/metadata">
    <Schema Namespace="NorthwindModel" xmlns="http://schemas.microsoft.com/ado/2008/09/edm">
      <EntityType Name="Order_Detail">
        <Key><PropertyRef Name="OrderID"/><PropertyRef Name="ProductID"/></Key>
        <Property Name="OrderID" Type="Edm.Int32" Nullable="false"/>
        <Property Name="ProductID" Type="Edm.Int32" Nullable="false"/>
        <Property Name="UnitPrice" Type="Edm.Decimal" Nullable="false"/>
      </EntityType>
    </Schema>
    <Schema Namespace="ODataWeb.Northwind.Model" xmlns="http://schemas.microsoft.com/ado/2008/09/edm">
      <EntityContainer Name="NorthwindEntities" m:IsDefaultEntityContainer="true">
        <EntitySet Name="Order_Details" EntityType="NorthwindModel.Order_Detail"/>
        <FunctionImport Name="TopProducts" ReturnType="Collection(NorthwindModel.Product)" m:HttpMethod="GET">
          <Parameter Name="count" Type="Edm.Int32" Mode="In"/>
        </FunctionImport>
      </EntityContainer>
    </Schema>
  </edmx:DataServices>
</edmx:Edmx>"#;

    #[test]
    fn reads_v4_metadata() {
        assert!(is_metadata(V4));
        let md = parse_metadata(V4).unwrap();
        assert_eq!(md.version, ODataVersion::V4);
        let people = md.entity_set("People").unwrap();
        assert_eq!(people.keys[0].name, "UserName");
        assert_eq!(people.navigation, vec!["Friends"]);
        let emp = md.entity_set("Employees").unwrap();
        assert_eq!(emp.keys[0].name, "UserName", "keys come from the base type");
        assert_eq!(emp.properties.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["UserName", "Age", "AddressInfo", "Cost"]);
        assert_eq!(md.singletons[0].name, "Me");
        assert_eq!(md.operations.len(), 2);
        assert_eq!(md.operations[0].params.len(), 2);
        assert!(md.operations[1].is_action);
        assert_eq!(example_entity(people), "{\n  \"Age\": 0,\n  \"UserName\": \"\"\n}");
    }

    #[test]
    fn reads_v2_metadata_and_imports() {
        let md = parse_metadata(V2).unwrap();
        assert_eq!(md.version, ODataVersion::V2);
        let set = md.entity_set("Order_Details").unwrap();
        assert_eq!(key_predicate(set, md.version), "(OrderID=1,ProductID=1)");
        let imp = import(V2, Some("https://services.odata.org/V2/Northwind/Northwind.svc/$metadata")).unwrap();
        assert_eq!(imp.collection.name, "Northwind.svc");
        assert_eq!(imp.collection.variables[0].value, "https://services.odata.org/V2/Northwind/Northwind.svc");
        let Item::Folder(f) = &imp.collection.items[0] else { panic!() };
        assert_eq!(f.items.len(), 5);
        let Item::Request(update) = &f.items[3] else { panic!() };
        assert_eq!(update.method, "MERGE");
        assert_eq!(update.odata.resource, "Order_Details(OrderID=1,ProductID=1)");
        let Item::Folder(ops) = &imp.collection.items[1] else { panic!() };
        let Item::Request(top) = &ops.items[0] else { panic!() };
        assert_eq!((top.odata.resource.as_str(), top.params[0].key.as_str()), ("TopProducts", "count"));
    }

    #[test]
    fn query_options_and_errors() {
        let o = ODataOptions {
            version: ODataVersion::V4,
            select: "Name, Age".into(),
            filter: "Name eq 'A&B' and Age gt 3".into(),
            top: "5".into(),
            count: true,
            ..Default::default()
        };
        assert_eq!(encode_query(&query_options(&o)), "$select=Name,%20Age&$filter=Name%20eq%20'A%26B'%20and%20Age%20gt%203&$top=5&$count=true");
        let v2 = ODataOptions { version: ODataVersion::V2, count: true, search: "x".into(), ..Default::default() };
        assert_eq!(query_options(&v2), vec![("$inlinecount".to_string(), "allpages".to_string())]);
        assert_eq!(resource_url("https://h/svc/", "/People('a')"), "https://h/svc/People('a')");
        assert_eq!(metadata_url("https://h/svc/?x=1"), "https://h/svc/$metadata");
        assert_eq!(error_message(r#"{"error":{"code":"404","message":"Not found"}}"#).as_deref(), Some("Not found (404)"));
        assert_eq!(error_message(r#"{"error":{"code":"","message":{"lang":"en-US","value":"Bad"}}}"#).as_deref(), Some("Bad"));
        assert_eq!(error_message(r#"{"odata.error":{"code":"","message":{"lang":"en","value":"V3"}}}"#).as_deref(), Some("V3"));
        assert_eq!(
            error_message(
                r#"<error xmlns="http://schemas.microsoft.com/ado/2007/08/dataservices/metadata"><code/><message xml:lang="en">Xml</message></error>"#
            )
            .as_deref(),
            Some("Xml")
        );
        assert_eq!(error_message(r#"{"value":[]}"#), None);
    }
}

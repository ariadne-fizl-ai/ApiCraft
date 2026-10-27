//! WSDL 1.1 import: one SOAP request per operation, with the SOAPAction,
//! SOAP version and endpoint from the binding, and an example envelope built
//! from the XML Schema types. Each request gets checks for a 200 status and
//! the operation's response element.

use roxmltree::{Document, Node};

use crate::model::*;
use crate::soap;

const WSDL_NS: &str = "http://schemas.xmlsoap.org/wsdl/";
const SOAP11_NS: &str = "http://schemas.xmlsoap.org/wsdl/soap/";
const SOAP12_NS: &str = "http://schemas.xmlsoap.org/wsdl/soap12/";
const XSD_NS: &str = "http://www.w3.org/2001/XMLSchema";

#[derive(Debug)]
pub struct WsdlImport {
    pub collection: Collection,
    pub operations: usize,
}

fn local(qname: &str) -> &str {
    qname.rsplit(':').next().unwrap_or(qname)
}

fn children<'a, 'i>(n: Node<'a, 'i>, ns: &'a str, name: &'a str) -> impl Iterator<Item = Node<'a, 'i>> + 'a {
    n.children().filter(move |c| c.is_element() && c.tag_name().name() == name && c.tag_name().namespace() == Some(ns))
}

fn child<'a, 'i>(n: Node<'a, 'i>, ns: &'a str, name: &'a str) -> Option<Node<'a, 'i>> {
    children(n, ns, name).next()
}

/// The XML Schemas inside `<wsdl:types>`.
struct Schemas<'a, 'i> {
    schemas: Vec<Node<'a, 'i>>,
}

impl<'a, 'i> Schemas<'a, 'i> {
    fn find(&self, kind: &str, name: &str) -> Option<(Node<'a, 'i>, Node<'a, 'i>)> {
        for s in &self.schemas {
            if let Some(n) = s.children().find(|c| c.is_element() && c.tag_name().name() == kind && c.attribute("name") == Some(name)) {
                return Some((*s, n));
            }
        }
        None
    }

    fn qualified(schema: Node) -> bool {
        schema.attribute("elementFormDefault") == Some("qualified")
    }
}

fn builtin_sample(ty: &str) -> &'static str {
    match ty {
        "int" | "integer" | "long" | "short" | "byte" | "unsignedInt" | "unsignedLong" | "unsignedShort" | "unsignedByte" | "nonNegativeInteger"
        | "positiveInteger" => "0",
        "decimal" | "double" | "float" => "0.0",
        "boolean" => "false",
        "dateTime" => "2024-01-01T12:00:00",
        "date" => "2024-01-01",
        "time" => "12:00:00",
        "base64Binary" => "ZXhhbXBsZQ==",
        "guid" => "00000000-0000-0000-0000-000000000000",
        "anyURI" => "https://example.com",
        "string" | "normalizedString" | "token" | "anyType" => "string",
        _ => "?",
    }
}

struct Writer<'s, 'a, 'i> {
    schemas: &'s Schemas<'a, 'i>,
    out: String,
    depth: usize,
}

impl<'s, 'a, 'i> Writer<'s, 'a, 'i> {
    fn indent(&mut self) {
        for _ in 0..self.depth {
            self.out.push_str("  ");
        }
    }

    /// Writes `<name>...</name>` for an element declaration.
    fn element(&mut self, el: Node, schema: Node, name: &str, open_extra: &str) {
        if self.depth > 12 {
            return;
        }
        let max = el.attribute("maxOccurs").unwrap_or("1");
        let repeat_note = if max != "1" && max != "0" { " <!-- repeatable -->" } else { "" };
        let optional_note = if el.attribute("minOccurs") == Some("0") { " <!-- optional -->" } else { "" };
        // type="tns:Foo" or xs:string, or an inline complexType / simpleType.
        if let Some(ty) = el.attribute("type") {
            let ns = el.lookup_namespace_uri(ty.split_once(':').map(|(p, _)| p));
            let ty_local = local(ty);
            if ns == Some(XSD_NS) || ns.is_none() && self.schemas.find("complexType", ty_local).is_none() && self.schemas.find("simpleType", ty_local).is_none()
            {
                self.leaf(name, open_extra, builtin_sample(ty_local), repeat_note, optional_note);
                return;
            }
            if let Some((s, ct)) = self.schemas.find("complexType", ty_local) {
                self.complex(name, open_extra, ct, s, repeat_note, optional_note);
                return;
            }
            if let Some((_, st)) = self.schemas.find("simpleType", ty_local) {
                let v = simple_sample(st);
                self.leaf(name, open_extra, &v, repeat_note, optional_note);
                return;
            }
            self.leaf(name, open_extra, "?", repeat_note, optional_note);
            return;
        }
        if let Some(ct) = child(el, XSD_NS, "complexType") {
            self.complex(name, open_extra, ct, schema, repeat_note, optional_note);
        } else if let Some(st) = child(el, XSD_NS, "simpleType") {
            let v = simple_sample(st);
            self.leaf(name, open_extra, &v, repeat_note, optional_note);
        } else {
            self.leaf(name, open_extra, "?", repeat_note, optional_note);
        }
    }

    fn leaf(&mut self, name: &str, open_extra: &str, value: &str, a: &str, b: &str) {
        self.indent();
        self.out.push_str(&format!("<{name}{open_extra}>{value}</{name}>{a}{b}\n"));
    }

    fn complex(&mut self, name: &str, open_extra: &str, ct: Node, schema: Node, a: &str, b: &str) {
        let mut fields = Vec::new();
        collect_fields(self.schemas, ct, &mut fields, 0);
        if fields.is_empty() {
            self.indent();
            self.out.push_str(&format!("<{name}{open_extra}/>{a}{b}\n"));
            return;
        }
        self.indent();
        self.out.push_str(&format!("<{name}{open_extra}>{a}{b}\n"));
        self.depth += 1;
        for (el, fschema) in fields {
            let fname = el.attribute("name").map(str::to_string).or_else(|| el.attribute("ref").map(|r| local(r).to_string())).unwrap_or_default();
            if let Some(r) = el.attribute("ref") {
                if let Some((s, target)) = self.schemas.find("element", local(r)) {
                    self.element(target, s, &fname, "");
                    continue;
                }
            }
            self.element(el, fschema, &fname, "");
        }
        self.depth -= 1;
        self.indent();
        self.out.push_str(&format!("</{name}>\n"));
        let _ = schema;
    }
}

/// The element declarations inside a complexType, following
/// sequence/all/choice and complexContent extensions.
fn collect_fields<'a, 'i>(schemas: &Schemas<'a, 'i>, ct: Node<'a, 'i>, out: &mut Vec<(Node<'a, 'i>, Node<'a, 'i>)>, depth: usize) {
    if depth > 8 {
        return;
    }
    let schema = ct.ancestors().find(|a| a.tag_name().name() == "schema").unwrap_or(ct);
    for c in ct.children().filter(|c| c.is_element()) {
        match c.tag_name().name() {
            "sequence" | "all" | "choice" => {
                for e in c.children().filter(|e| e.is_element()) {
                    match e.tag_name().name() {
                        "element" => out.push((e, schema)),
                        "sequence" | "choice" | "all" => collect_fields(schemas, c, out, depth + 1),
                        _ => {}
                    }
                    if c.tag_name().name() == "choice" && !out.is_empty() {
                        break;
                    }
                }
            }
            "complexContent" => {
                if let Some(ext) = c.children().find(|e| e.is_element() && (e.tag_name().name() == "extension" || e.tag_name().name() == "restriction")) {
                    if let Some(base) = ext.attribute("base") {
                        if let Some((_, base_ct)) = schemas.find("complexType", local(base)) {
                            collect_fields(schemas, base_ct, out, depth + 1);
                        }
                    }
                    collect_fields(schemas, ext, out, depth + 1);
                }
            }
            _ => {}
        }
    }
}

fn simple_sample(st: Node) -> String {
    let restriction = st.children().find(|c| c.is_element() && c.tag_name().name() == "restriction");
    if let Some(r) = restriction {
        if let Some(e) = r.children().find(|c| c.is_element() && c.tag_name().name() == "enumeration") {
            return e.attribute("value").unwrap_or("").to_string();
        }
        if let Some(base) = r.attribute("base") {
            return builtin_sample(local(base)).to_string();
        }
    }
    "?".into()
}

/// A message part: (name, element, type).
type Part = (String, Option<String>, Option<String>);

struct Op {
    name: String,
    input_message: Option<String>,
    output_message: Option<String>,
    doc: String,
}

pub fn import(xml: &str) -> Result<WsdlImport, String> {
    let doc = Document::parse(xml).map_err(|e| format!("not valid XML: {e}"))?;
    let defs = doc.root_element();
    if defs.tag_name().name() != "definitions" {
        if defs.tag_name().name() == "description" {
            return Err("WSDL 2.0 isn't supported yet; most services also publish a WSDL 1.1 (e.g. ?wsdl)".into());
        }
        return Err("not a WSDL document (expected <wsdl:definitions>)".into());
    }
    let tns = defs.attribute("targetNamespace").unwrap_or("").to_string();
    let schemas = Schemas { schemas: child(defs, WSDL_NS, "types").map(|t| children(t, XSD_NS, "schema").collect()).unwrap_or_default() };

    // messages: name -> parts (name, element?, type?)
    let messages: Vec<(String, Vec<Part>)> = children(defs, WSDL_NS, "message")
        .map(|m| {
            let parts = children(m, WSDL_NS, "part")
                .map(|p| (p.attribute("name").unwrap_or("").to_string(), p.attribute("element").map(str::to_string), p.attribute("type").map(str::to_string)))
                .collect();
            (m.attribute("name").unwrap_or("").to_string(), parts)
        })
        .collect();
    let message = |qname: &str| messages.iter().find(|(n, _)| n == local(qname)).map(|(_, p)| p.clone()).unwrap_or_default();

    // portTypes: name -> operations
    let port_types: Vec<(String, Vec<Op>)> = children(defs, WSDL_NS, "portType")
        .map(|pt| {
            let ops = children(pt, WSDL_NS, "operation")
                .map(|o| Op {
                    name: o.attribute("name").unwrap_or("").to_string(),
                    input_message: child(o, WSDL_NS, "input").and_then(|i| i.attribute("message")).map(str::to_string),
                    output_message: child(o, WSDL_NS, "output").and_then(|i| i.attribute("message")).map(str::to_string),
                    doc: child(o, WSDL_NS, "documentation").and_then(|d| d.text()).unwrap_or("").trim().to_string(),
                })
                .collect();
            (pt.attribute("name").unwrap_or("").to_string(), ops)
        })
        .collect();

    // service ports: (port name, binding qname, address, soap version)
    let service = child(defs, WSDL_NS, "service");
    let service_name = service.and_then(|s| s.attribute("name")).or_else(|| defs.attribute("name")).unwrap_or("SOAP service").to_string();
    let mut ports: Vec<(String, String, String, SoapVersion)> = Vec::new();
    if let Some(s) = service {
        for p in children(s, WSDL_NS, "port") {
            let (addr, ver) = if let Some(a) = child(p, SOAP11_NS, "address") {
                (a.attribute("location").unwrap_or("").to_string(), SoapVersion::V11)
            } else if let Some(a) = child(p, SOAP12_NS, "address") {
                (a.attribute("location").unwrap_or("").to_string(), SoapVersion::V12)
            } else {
                continue; // HTTP GET/POST bindings
            };
            ports.push((p.attribute("name").unwrap_or("").to_string(), p.attribute("binding").unwrap_or("").to_string(), addr, ver));
        }
    }
    // Prefer one SOAP 1.1 port; fall back to 1.2.
    ports.sort_by_key(|p| p.3 == SoapVersion::V12);
    let Some((_, binding_q, address, version)) = ports.first().cloned() else {
        return Err("the WSDL has no SOAP port in its <service>".into());
    };
    let binding = children(defs, WSDL_NS, "binding").find(|b| b.attribute("name") == Some(local(&binding_q))).ok_or("the SOAP port's binding is missing")?;
    let soap_ns = if version == SoapVersion::V12 { SOAP12_NS } else { SOAP11_NS };
    let default_style = child(binding, soap_ns, "binding").and_then(|b| b.attribute("style")).unwrap_or("document").to_string();
    let port_type_name = local(binding.attribute("type").unwrap_or("")).to_string();
    let ops = port_types.into_iter().find(|(n, _)| *n == port_type_name).map(|(_, o)| o).unwrap_or_default();

    let mut col = Collection::new(&service_name);
    col.description = format!("Imported from WSDL. Target namespace: {tns}");
    col.variables.push(KeyValue::new("serviceUrl", if address.is_empty() { "http://localhost/service".to_string() } else { address }));

    let mut count = 0;
    for op in &ops {
        let bop = children(binding, WSDL_NS, "operation").find(|b| b.attribute("name") == Some(&op.name));
        let soap_op = bop.and_then(|b| child(b, soap_ns, "operation"));
        let action = soap_op.and_then(|s| s.attribute("soapAction")).unwrap_or("").to_string();
        let style = soap_op.and_then(|s| s.attribute("style")).map(str::to_string).unwrap_or_else(|| default_style.clone());

        let mut payload = String::new();
        let input_parts = op.input_message.as_deref().map(message).unwrap_or_default();
        if style == "rpc" {
            let mut w = Writer { schemas: &schemas, out: String::new(), depth: 1 };
            w.out.push_str(&format!("    <tns:{} xmlns:tns=\"{tns}\">\n", op.name));
            w.depth = 3;
            for (pname, _, ty) in &input_parts {
                let sample = ty.as_deref().map(|t| builtin_sample(local(t))).unwrap_or("?");
                w.leaf(pname, "", sample, "", "");
            }
            w.out.push_str(&format!("    </tns:{}>", op.name));
            payload = w.out;
        } else {
            for (_, element, _) in &input_parts {
                let Some(el_q) = element else { continue };
                let Some((schema, el)) = schemas.find("element", local(el_q)) else { continue };
                let ns = schema.attribute("targetNamespace").unwrap_or(&tns).to_string();
                let mut w = Writer { schemas: &schemas, out: String::new(), depth: 2 };
                // Qualified schemas: a default namespace qualifies every child too.
                let (name, extra) = if Schemas::qualified(schema) {
                    (local(el_q).to_string(), format!(" xmlns=\"{ns}\""))
                } else {
                    (format!("tns:{}", local(el_q)), format!(" xmlns:tns=\"{ns}\""))
                };
                w.element(el, schema, &name, &extra);
                payload.push_str(w.out.trim_end());
            }
        }
        let body = soap::envelope_with_payload(&payload, version);

        let mut r = Request::new(op.name.clone(), Protocol::Soap);
        r.url = "{{serviceUrl}}".into();
        r.soap = SoapOptions { version, action };
        r.body = Body::Raw { content_type: String::new(), text: body };
        r.description = op.doc.clone();
        r.checks.push(Check { kind: CheckKind::Status, target: String::new(), expected: "200".into(), enabled: true });
        let response_el = op.output_message.as_deref().map(message).and_then(|parts| parts.into_iter().find_map(|(_, e, _)| e)).map(|e| local(&e).to_string());
        let response_name = if style == "rpc" { Some(format!("{}Response", op.name)) } else { response_el };
        if let Some(name) = response_name {
            r.checks.push(Check { kind: CheckKind::BodyContains, target: String::new(), expected: name, enabled: true });
        }
        col.items.push(Item::Request(r));
        count += 1;
    }
    if count == 0 {
        return Err("the WSDL's SOAP binding has no operations".into());
    }
    Ok(WsdlImport { collection: col, operations: count })
}

#[cfg(test)]
mod tests {
    use super::*;

    const CALCULATOR: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<wsdl:definitions xmlns:soap="http://schemas.xmlsoap.org/wsdl/soap/" xmlns:soap12="http://schemas.xmlsoap.org/wsdl/soap12/"
    xmlns:tns="http://tempuri.org/" xmlns:s="http://www.w3.org/2001/XMLSchema" xmlns:wsdl="http://schemas.xmlsoap.org/wsdl/" targetNamespace="http://tempuri.org/">
  <wsdl:types>
    <s:schema elementFormDefault="qualified" targetNamespace="http://tempuri.org/">
      <s:element name="Add">
        <s:complexType><s:sequence>
          <s:element minOccurs="1" maxOccurs="1" name="intA" type="s:int" />
          <s:element minOccurs="1" maxOccurs="1" name="intB" type="s:int" />
        </s:sequence></s:complexType>
      </s:element>
      <s:element name="AddResponse">
        <s:complexType><s:sequence><s:element name="AddResult" type="s:int" /></s:sequence></s:complexType>
      </s:element>
      <s:element name="Lookup">
        <s:complexType><s:sequence>
          <s:element name="who" type="tns:Person" />
          <s:element minOccurs="0" name="mode" type="tns:Mode" />
        </s:sequence></s:complexType>
      </s:element>
      <s:complexType name="Person"><s:sequence><s:element name="Name" type="s:string" /></s:sequence></s:complexType>
      <s:simpleType name="Mode"><s:restriction base="s:string"><s:enumeration value="Fast" /><s:enumeration value="Slow" /></s:restriction></s:simpleType>
    </s:schema>
  </wsdl:types>
  <wsdl:message name="AddSoapIn"><wsdl:part name="parameters" element="tns:Add" /></wsdl:message>
  <wsdl:message name="AddSoapOut"><wsdl:part name="parameters" element="tns:AddResponse" /></wsdl:message>
  <wsdl:message name="LookupSoapIn"><wsdl:part name="parameters" element="tns:Lookup" /></wsdl:message>
  <wsdl:portType name="CalculatorSoap">
    <wsdl:operation name="Add"><wsdl:documentation>Adds two integers.</wsdl:documentation><wsdl:input message="tns:AddSoapIn" /><wsdl:output message="tns:AddSoapOut" /></wsdl:operation>
    <wsdl:operation name="Lookup"><wsdl:input message="tns:LookupSoapIn" /></wsdl:operation>
  </wsdl:portType>
  <wsdl:binding name="CalculatorSoap12" type="tns:CalculatorSoap">
    <soap12:binding transport="http://schemas.xmlsoap.org/soap/http" />
    <wsdl:operation name="Add"><soap12:operation soapAction="http://tempuri.org/Add" style="document" /></wsdl:operation>
  </wsdl:binding>
  <wsdl:binding name="CalculatorSoap" type="tns:CalculatorSoap">
    <soap:binding transport="http://schemas.xmlsoap.org/soap/http" />
    <wsdl:operation name="Add"><soap:operation soapAction="http://tempuri.org/Add" style="document" /></wsdl:operation>
    <wsdl:operation name="Lookup"><soap:operation soapAction="http://tempuri.org/Lookup" style="document" /></wsdl:operation>
  </wsdl:binding>
  <wsdl:service name="Calculator">
    <wsdl:port name="CalculatorSoap12" binding="tns:CalculatorSoap12"><soap12:address location="http://www.dneonline.com/calculator.asmx" /></wsdl:port>
    <wsdl:port name="CalculatorSoap" binding="tns:CalculatorSoap"><soap:address location="http://www.dneonline.com/calculator.asmx" /></wsdl:port>
  </wsdl:service>
</wsdl:definitions>"#;

    #[test]
    fn imports_document_literal_wsdl() {
        let imp = import(CALCULATOR).unwrap();
        let c = imp.collection;
        assert_eq!(c.name, "Calculator");
        assert_eq!(imp.operations, 2);
        assert!(c.variables.contains(&KeyValue::new("serviceUrl", "http://www.dneonline.com/calculator.asmx")));
        let Item::Request(add) = &c.items[0] else { panic!() };
        assert_eq!(add.protocol, Protocol::Soap);
        assert_eq!(add.soap.version, SoapVersion::V11, "prefers the SOAP 1.1 port");
        assert_eq!(add.soap.action, "http://tempuri.org/Add");
        let body = add.body.raw_text();
        assert!(body.contains("<soap:Envelope"), "{body}");
        assert!(body.contains("<Add xmlns=\"http://tempuri.org/\">"), "{body}");
        assert!(body.contains("<intA>0</intA>"), "{body}");
        assert!(add.checks.iter().any(|ch| ch.expected == "AddResponse"));
        assert_eq!(add.description, "Adds two integers.");
        let Item::Request(lookup) = &c.items[1] else { panic!() };
        let body = lookup.body.raw_text();
        assert!(body.contains("<Name>string</Name>") || body.contains("<Name>?</Name>"), "{body}");
        assert!(body.contains("<mode>Fast</mode> <!-- optional -->"), "{body}");
    }
}

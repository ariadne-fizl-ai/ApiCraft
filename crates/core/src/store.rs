//! Catalog persistence: JSON, optionally gzip-compressed.
//!
//! [`decode`] sniffs the input, so the same entry point accepts our own
//! catalogs (`.json` or `.json.gz`), Postman collections and Postman
//! environments.

use std::io::{Read, Write};

use base64::Engine;
use flate2::{read::GzDecoder, write::GzEncoder, Compression};

use crate::model::{Catalog, Collection, Environment, CATALOG_FORMAT, LEGACY_CATALOG_FORMAT};
use crate::postman;

pub fn to_json(catalog: &Catalog) -> String {
    serde_json::to_string_pretty(catalog).expect("catalog serialises")
}

pub fn to_gzip(catalog: &Catalog) -> Vec<u8> {
    let json = serde_json::to_vec(catalog).expect("catalog serialises");
    gzip(&json)
}

pub fn gzip(data: &[u8]) -> Vec<u8> {
    let mut enc = GzEncoder::new(Vec::new(), Compression::best());
    enc.write_all(data).expect("in-memory write");
    enc.finish().expect("in-memory write")
}

pub fn gunzip(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    GzDecoder::new(data).read_to_end(&mut out).map_err(|e| format!("not a valid gzip file: {e}"))?;
    Ok(out)
}

pub fn is_gzip(data: &[u8]) -> bool {
    data.starts_with(&[0x1f, 0x8b])
}

/// Compact text form for browser storage: base64 of the gzip bytes.
pub fn to_storage_string(catalog: &Catalog) -> String {
    base64::engine::general_purpose::STANDARD.encode(to_gzip(catalog))
}

pub fn from_storage_string(s: &str) -> Result<Catalog, String> {
    let bytes = base64::engine::general_purpose::STANDARD.decode(s.trim()).map_err(|e| e.to_string())?;
    match decode(&bytes)? {
        Imported::Catalog(c) => Ok(c),
        _ => Err("stored data is not a catalog".into()),
    }
}

/// What a file turned out to contain.
#[allow(clippy::large_enum_variant)] // short-lived, one per import
#[derive(Debug)]
pub enum Imported {
    Catalog(Catalog),
    /// A Postman collection, already converted.
    PostmanCollection(Collection),
    PostmanEnvironment(Environment),
    /// An OpenAPI/Swagger definition, with generated tests and workflows.
    OpenApi(crate::openapi::OpenApiImport),
    /// A WSDL: one SOAP request per operation.
    Wsdl(crate::wsdl::WsdlImport),
    /// A GraphQL introspection result: one request per query and mutation.
    GraphQl(crate::graphql::GraphQlImport),
    /// An OData `$metadata` document: requests per entity set and operation.
    OData(crate::odata::ODataImport),
}

impl Imported {
    /// Folds the import into a catalog so callers can merge it in one step.
    pub fn into_catalog(self) -> Catalog {
        match self {
            Imported::Catalog(c) => c,
            Imported::PostmanCollection(col) => Catalog { collections: vec![col], ..Catalog::default() },
            Imported::PostmanEnvironment(env) => Catalog { environments: vec![env], ..Catalog::default() },
            Imported::OpenApi(o) => Catalog { collections: vec![o.collection], workflows: o.workflows, ..Catalog::default() },
            Imported::Wsdl(w) => Catalog { collections: vec![w.collection], ..Catalog::default() },
            Imported::GraphQl(g) => Catalog { collections: vec![g.collection], ..Catalog::default() },
            Imported::OData(o) => Catalog { collections: vec![o.collection], ..Catalog::default() },
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Imported::Catalog(c) => format!("{} collection(s), {} environment(s)", c.collections.len(), c.environments.len()),
            Imported::PostmanCollection(c) => format!("Postman collection \u{201c}{}\u{201d}", c.name),
            Imported::PostmanEnvironment(e) => format!("Postman environment \u{201c}{}\u{201d}", e.name),
            Imported::OpenApi(o) => {
                format!("OpenAPI \u{201c}{}\u{201d}: {} operation(s) with checks, {} workflow(s)", o.collection.name, o.operations, o.workflows.len())
            }
            Imported::Wsdl(w) => format!("WSDL \u{201c}{}\u{201d}: {} SOAP operation(s)", w.collection.name, w.operations),
            Imported::GraphQl(g) => format!("GraphQL schema: {} query and mutation request(s)", g.operations),
            Imported::OData(o) => {
                format!("OData service \u{201c}{}\u{201d}: {} entity set(s), {} operation(s)", o.collection.name, o.entity_sets, o.operations)
            }
        }
    }
}

pub fn decode(data: &[u8]) -> Result<Imported, String> {
    decode_from(data, None)
}

/// Like [`decode`]; `source` is the URL the data came from (it resolves
/// relative server URLs in OpenAPI documents).
pub fn decode_from(data: &[u8], source: Option<&str>) -> Result<Imported, String> {
    let raw;
    let data = if is_gzip(data) {
        raw = gunzip(data)?;
        &raw[..]
    } else {
        data
    };
    let text = std::str::from_utf8(data).map_err(|_| "file is not UTF-8 text".to_string())?;
    let text = text.trim_start_matches('\u{feff}');
    if text.trim_start().starts_with('<') {
        if crate::odata::is_metadata(text) {
            return crate::odata::import(text, source).map(Imported::OData);
        }
        return crate::wsdl::import(text).map(Imported::Wsdl);
    }
    let value: serde_json::Value = match serde_json::from_str(text) {
        Ok(v) => v,
        // OpenAPI definitions are often YAML.
        Err(e) => match crate::openapi::parse_document(text) {
            Ok(v) if crate::openapi::is_openapi(&v) => v,
            _ => return Err(format!("not valid JSON: {e}")),
        },
    };
    if crate::openapi::is_openapi(&value) {
        return crate::openapi::import_from(&value, source).map(Imported::OpenApi);
    }
    if crate::graphql::is_introspection(&value) {
        return crate::graphql::import(&value, None).map(Imported::GraphQl);
    }

    let format = value.get("format").and_then(|f| f.as_str());
    if matches!(format, Some(CATALOG_FORMAT | LEGACY_CATALOG_FORMAT)) || value.get("collections").is_some() {
        let catalog: Catalog = serde_json::from_value(value).map_err(|e| format!("invalid catalog: {e}"))?;
        return Ok(Imported::Catalog(catalog));
    }
    if postman::is_collection(&value) {
        return postman::import_collection(&value).map(Imported::PostmanCollection);
    }
    if postman::is_environment(&value) {
        return postman::import_environment(&value).map(Imported::PostmanEnvironment);
    }
    Err("unrecognised file: expected a Fizl ApiCraft catalog, a Postman collection or environment, an OpenAPI/Swagger definition, a WSDL, a GraphQL introspection result or an OData $metadata document".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::*;

    #[test]
    fn gzip_round_trip() {
        let mut cat = Catalog::default();
        let mut col = Collection::new("Demo");
        col.items.push(Item::Request(Request::new("Get", Protocol::Rest)));
        col.items.push(Item::Request(Request::new("Soap", Protocol::Soap)));
        cat.collections.push(col);
        let bytes = to_gzip(&cat);
        assert!(is_gzip(&bytes));
        match decode(&bytes).unwrap() {
            Imported::Catalog(back) => assert_eq!(back, cat),
            other => panic!("{other:?}"),
        }
        assert_eq!(from_storage_string(&to_storage_string(&cat)).unwrap(), cat);
        match decode(to_json(&cat).as_bytes()).unwrap() {
            Imported::Catalog(back) => assert_eq!(back, cat),
            other => panic!("{other:?}"),
        }
    }
}

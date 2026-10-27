//! Pulling values out of responses (for `{{variables}}`) and checking them.

use crate::model::{Check, CheckKind, ExtractSource, Extraction};

/// The parts of a response that extraction and checks look at.
pub struct ResponseView<'a> {
    pub status: u16,
    pub headers: &'a [(String, String)],
    /// Body as text; for gRPC this is the decoded message as JSON.
    pub body: &'a str,
    pub elapsed_ms: f64,
}

impl ResponseView<'_> {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

pub fn extract(e: &Extraction, r: &ResponseView) -> Result<String, String> {
    let expr = e.expression.trim();
    match e.source {
        ExtractSource::Status => Ok(r.status.to_string()),
        ExtractSource::Header => r.header(expr).map(str::to_string).ok_or_else(|| format!("no header {expr}")),
        ExtractSource::Json => {
            let v: serde_json::Value = serde_json::from_str(r.body).map_err(|_| "response is not JSON".to_string())?;
            json_path(&v, expr).map(|x| value_text(&x)).ok_or_else(|| format!("nothing at {expr}"))
        }
        ExtractSource::Xml => xml_path(r.body, expr).ok_or_else(|| format!("nothing at {expr}")),
        ExtractSource::Regex => {
            let re = regex_lite::Regex::new(expr).map_err(|err| format!("bad regex: {err}"))?;
            let caps = re.captures(r.body).ok_or_else(|| format!("no match for {expr}"))?;
            Ok(caps.get(1).or_else(|| caps.get(0)).map(|m| m.as_str().to_string()).unwrap_or_default())
        }
    }
}

pub fn value_text(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// `$.a.b[0]['c d']`, `a.b.0.c`, and `.length` on arrays/strings.
pub fn json_path<'a>(root: &'a serde_json::Value, path: &str) -> Option<std::borrow::Cow<'a, serde_json::Value>> {
    use std::borrow::Cow;
    let mut cur: Cow<serde_json::Value> = Cow::Borrowed(root);
    for seg in path_segments(path)? {
        let next = match (&*cur, &seg) {
            (serde_json::Value::Object(o), Seg::Key(k)) => o.get(k.as_str()).cloned(),
            (serde_json::Value::Array(a), Seg::Index(i)) => a.get(*i).cloned(),
            (serde_json::Value::Array(a), Seg::Key(k)) if k == "length" => Some(a.len().into()),
            (serde_json::Value::Array(a), Seg::Key(k)) => k.parse::<usize>().ok().and_then(|i| a.get(i).cloned()),
            (serde_json::Value::String(s), Seg::Key(k)) if k == "length" => Some(s.chars().count().into()),
            (serde_json::Value::Object(o), Seg::Index(i)) => o.get(&i.to_string()).cloned(),
            _ => None,
        }?;
        cur = Cow::Owned(next);
    }
    Some(cur)
}

pub(crate) enum Seg {
    Key(String),
    Index(usize),
}

pub(crate) fn path_segments(path: &str) -> Option<Vec<Seg>> {
    let p = path.trim();
    let p = p.strip_prefix('$').unwrap_or(p);
    let b = p.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'.' => i += 1,
            b'[' => {
                let end = p[i..].find(']')? + i;
                let inner = p[i + 1..end].trim();
                if let Some(q) =
                    inner.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')).or_else(|| inner.strip_prefix('"').and_then(|s| s.strip_suffix('"')))
                {
                    out.push(Seg::Key(q.to_string()));
                } else {
                    out.push(Seg::Index(inner.parse().ok()?));
                }
                i = end + 1;
            }
            _ => {
                let end = p[i..].find(['.', '[']).map(|n| n + i).unwrap_or(p.len());
                out.push(Seg::Key(p[i..end].to_string()));
                i = end;
            }
        }
    }
    Some(out)
}

/// `Envelope/Body/AddResponse/AddResult`, matching local names (prefixes ignored).
/// Each step finds the first matching descendant, so intermediate steps can
/// be skipped. A last step of `@name` reads an attribute.
pub fn xml_path(xml: &str, path: &str) -> Option<String> {
    let mut scope = xml;
    let mut open_tag = "";
    let segs: Vec<&str> = path.split('/').map(str::trim).filter(|s| !s.is_empty()).collect();
    for (n, seg) in segs.iter().enumerate() {
        if let Some(attr) = seg.strip_prefix('@') {
            if n + 1 != segs.len() {
                return None;
            }
            return attribute(open_tag, attr);
        }
        let (tag, inner) = find_element(scope, seg)?;
        open_tag = tag;
        scope = inner;
    }
    let text = scope.trim();
    if text.contains('<') && !text.starts_with("<![CDATA[") {
        return Some(text.to_string());
    }
    let text = text.strip_prefix("<![CDATA[").and_then(|t| t.strip_suffix("]]>")).map(str::to_string).unwrap_or_else(|| unescape_xml(text));
    Some(text)
}

fn local(name: &str) -> &str {
    name.rsplit(':').next().unwrap_or(name)
}

/// First element with this local name: (open tag text, inner content).
fn find_element<'a>(xml: &'a str, name: &str) -> Option<(&'a str, &'a str)> {
    let mut pos = 0;
    while let Some(off) = xml[pos..].find('<') {
        let start = pos + off;
        let end = xml[start..].find('>')? + start;
        let tag = &xml[start + 1..end];
        pos = end + 1;
        if tag.starts_with('/') || tag.starts_with('?') || tag.starts_with('!') {
            continue;
        }
        let tag_name = tag.split(|c: char| c.is_whitespace() || c == '/').next().unwrap_or("");
        if local(tag_name) != name {
            continue;
        }
        if tag.ends_with('/') {
            return Some((tag, ""));
        }
        // find the matching close, counting nested same-name elements
        let mut depth = 1;
        let mut p = pos;
        while let Some(o) = xml[p..].find('<') {
            let s = p + o;
            let e = xml[s..].find('>')? + s;
            let t = &xml[s + 1..e];
            let t_name = t.trim_start_matches('/').split(|c: char| c.is_whitespace() || c == '/').next().unwrap_or("");
            if t_name == tag_name {
                if t.starts_with('/') {
                    depth -= 1;
                    if depth == 0 {
                        return Some((tag, &xml[pos..s]));
                    }
                } else if !t.ends_with('/') {
                    depth += 1;
                }
            }
            p = e + 1;
        }
        return None;
    }
    None
}

fn attribute(tag: &str, name: &str) -> Option<String> {
    let mut rest = tag;
    while let Some(eq) = rest.find('=') {
        let key = rest[..eq].split_whitespace().last().unwrap_or("");
        let after = rest[eq + 1..].trim_start();
        let quote = after.chars().next()?;
        if quote != '"' && quote != '\'' {
            return None;
        }
        let end = after[1..].find(quote)? + 1;
        if local(key) == name {
            return Some(unescape_xml(&after[1..end]));
        }
        rest = &after[end + 1..];
    }
    None
}

pub fn unescape_xml(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

// ---------------------------------------------------------------- checks

#[derive(Debug, Clone, PartialEq)]
pub struct CheckResult {
    pub label: String,
    pub passed: bool,
    pub detail: String,
}

pub fn describe(c: &Check) -> String {
    match c.kind {
        CheckKind::Status => format!("status is {}", c.expected),
        CheckKind::BodyContains => format!("body contains \u{201c}{}\u{201d}", c.expected),
        CheckKind::JsonEquals if c.expected.is_empty() => format!("{} exists", c.target),
        CheckKind::JsonEquals => format!("{} = {}", c.target, c.expected),
        CheckKind::Header if c.expected.is_empty() => format!("header {} present", c.target),
        CheckKind::Header => format!("header {} = {}", c.target, c.expected),
        CheckKind::TimeBelow => format!("time < {} ms", c.expected),
    }
}

pub fn run_check(c: &Check, r: &ResponseView) -> CheckResult {
    let label = describe(c);
    let (passed, detail) = match c.kind {
        CheckKind::Status => (status_matches(&c.expected, r.status), format!("got {}", r.status)),
        CheckKind::BodyContains => (r.body.contains(&c.expected), String::new()),
        CheckKind::JsonEquals => match serde_json::from_str::<serde_json::Value>(r.body).ok().and_then(|v| json_path(&v, &c.target).map(|x| value_text(&x))) {
            Some(actual) => (c.expected.is_empty() || actual == c.expected, format!("got {actual}")),
            None => (false, "not found".into()),
        },
        CheckKind::Header => match r.header(&c.target) {
            Some(v) => (c.expected.is_empty() || v == c.expected, format!("got {v}")),
            None => (false, "missing".into()),
        },
        CheckKind::TimeBelow => {
            let limit: f64 = c.expected.trim().parse().unwrap_or(f64::MAX);
            (r.elapsed_ms < limit, format!("took {:.0} ms", r.elapsed_ms))
        }
    };
    CheckResult { label, passed, detail }
}

/// `200`, `2xx`, `200,201`, `200-299`. Empty means any 2xx.
pub fn status_matches(expected: &str, status: u16) -> bool {
    let e = expected.trim();
    if e.is_empty() {
        return (200..300).contains(&status);
    }
    e.split(',').map(str::trim).any(|part| {
        let lower = part.to_ascii_lowercase();
        if lower.len() == 3 && lower.ends_with("xx") {
            return lower[..1].parse::<u16>().map(|d| status / 100 == d).unwrap_or(false);
        }
        if let Some((a, b)) = part.split_once('-') {
            return matches!((a.trim().parse::<u16>(), b.trim().parse::<u16>()), (Ok(a), Ok(b)) if (a..=b).contains(&status));
        }
        part.parse::<u16>().map(|s| s == status).unwrap_or(false)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::VarScope;

    fn view<'a>(body: &'a str, headers: &'a [(String, String)]) -> ResponseView<'a> {
        ResponseView { status: 201, headers, body, elapsed_ms: 12.0 }
    }

    fn ex(source: ExtractSource, expression: &str) -> Extraction {
        Extraction { variable: "v".into(), source, expression: expression.into(), scope: VarScope::Runtime, enabled: true }
    }

    #[test]
    fn json_paths() {
        let body = r#"{"data":{"items":[{"id":7,"name":"a b"}],"token":"xyz"},"odd key":true}"#;
        let r = view(body, &[]);
        assert_eq!(extract(&ex(ExtractSource::Json, "$.data.token"), &r).unwrap(), "xyz");
        assert_eq!(extract(&ex(ExtractSource::Json, "data.items[0].id"), &r).unwrap(), "7");
        assert_eq!(extract(&ex(ExtractSource::Json, "data.items.0.name"), &r).unwrap(), "a b");
        assert_eq!(extract(&ex(ExtractSource::Json, "$['odd key']"), &r).unwrap(), "true");
        assert_eq!(extract(&ex(ExtractSource::Json, "data.items.length"), &r).unwrap(), "1");
        assert!(extract(&ex(ExtractSource::Json, "nope"), &r).is_err());
    }

    #[test]
    fn xml_paths() {
        let body = r#"<soap:Envelope xmlns:soap="x"><soap:Body><AddResponse><AddResult id="r1">5</AddResult></AddResponse></soap:Body></soap:Envelope>"#;
        let r = view(body, &[]);
        assert_eq!(extract(&ex(ExtractSource::Xml, "Envelope/Body/AddResponse/AddResult"), &r).unwrap(), "5");
        assert_eq!(extract(&ex(ExtractSource::Xml, "AddResult"), &r).unwrap(), "5");
        assert_eq!(extract(&ex(ExtractSource::Xml, "AddResult/@id"), &r).unwrap(), "r1");
    }

    #[test]
    fn other_sources_and_checks() {
        let headers = vec![("Location".to_string(), "/items/9".to_string())];
        let r = view("id=42;", &headers);
        assert_eq!(extract(&ex(ExtractSource::Header, "location"), &r).unwrap(), "/items/9");
        assert_eq!(extract(&ex(ExtractSource::Status, ""), &r).unwrap(), "201");
        assert_eq!(extract(&ex(ExtractSource::Regex, r"id=(\d+)"), &r).unwrap(), "42");
        assert!(status_matches("2xx", 201) && status_matches("200,201", 201) && status_matches("200-204", 201) && !status_matches("200", 201));
        let c = Check { kind: CheckKind::TimeBelow, target: String::new(), expected: "10".into(), enabled: true };
        assert!(!run_check(&c, &r).passed);
    }
}

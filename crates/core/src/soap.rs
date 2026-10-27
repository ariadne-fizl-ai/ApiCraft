//! SOAP 1.1 / 1.2 envelopes.

use crate::model::SoapVersion;

pub fn namespace(version: SoapVersion) -> &'static str {
    match version {
        SoapVersion::V11 => "http://schemas.xmlsoap.org/soap/envelope/",
        SoapVersion::V12 => "http://www.w3.org/2003/05/soap-envelope",
    }
}

pub fn envelope_template(version: SoapVersion) -> String {
    wrap("    <!-- request payload -->", version)
}

/// An envelope around an already-indented payload.
pub fn envelope_with_payload(payload: &str, version: SoapVersion) -> String {
    wrap(payload, version)
}

fn wrap(body: &str, version: SoapVersion) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<soap:Envelope xmlns:soap=\"{}\">\n  <soap:Header/>\n  <soap:Body>\n{}\n  </soap:Body>\n</soap:Envelope>",
        namespace(version),
        body
    )
}

/// Leaves a full envelope alone and wraps a bare payload in one, so users
/// can paste either form.
pub fn ensure_envelope(text: &str, version: SoapVersion) -> String {
    let t = text.trim();
    if t.is_empty() {
        return wrap("", version);
    }
    if has_envelope(t) {
        return text.to_string();
    }
    let indented: Vec<String> = t.lines().map(|l| format!("    {l}")).collect();
    wrap(&indented.join("\n"), version)
}

fn has_envelope(t: &str) -> bool {
    // Any prefix: <soap:Envelope, <s:Envelope, <SOAP-ENV:Envelope, <Envelope
    t.match_indices("Envelope").any(|(i, _)| {
        let before = &t[..i];
        let tag_start = before.rfind('<');
        matches!(tag_start, Some(s) if !before[s..].contains('>') && !before[s..].contains(' ') && !before[s + 1..].starts_with('/'))
    })
}

/// Pulls the text of a `<faultstring>` (1.1) or `<Text>` inside `<Reason>` (1.2).
pub fn fault_message(xml: &str) -> Option<String> {
    if !xml.contains("Fault") {
        return None;
    }
    for tag in ["faultstring", "Text"] {
        if let Some(text) = element_text(xml, tag) {
            return Some(text);
        }
    }
    Some("SOAP fault".into())
}

fn element_text(xml: &str, local: &str) -> Option<String> {
    let mut search = 0;
    while let Some(off) = xml[search..].find('<') {
        let start = search + off;
        let close = xml[start..].find('>')? + start;
        let tag = &xml[start + 1..close];
        let name = tag.split_whitespace().next().unwrap_or("");
        let name_local = name.rsplit(':').next().unwrap_or(name);
        if name_local == local && !tag.starts_with('/') && !tag.ends_with('/') {
            let end = xml[close + 1..].find("</")? + close + 1;
            return Some(xml[close + 1..end].trim().to_string());
        }
        search = close + 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_bare_payloads_only() {
        let bare = "<Add xmlns=\"http://tempuri.org/\"><a>1</a></Add>";
        let wrapped = ensure_envelope(bare, SoapVersion::V11);
        assert!(wrapped.contains("<soap:Envelope"));
        assert!(wrapped.contains(bare));
        assert_eq!(ensure_envelope(&wrapped, SoapVersion::V11), wrapped);
        let other = "<s:Envelope xmlns:s=\"x\"><s:Body/></s:Envelope>";
        assert_eq!(ensure_envelope(other, SoapVersion::V12), other);
    }

    #[test]
    fn finds_faults() {
        let xml = "<soap:Envelope><soap:Body><soap:Fault><faultcode>x</faultcode><faultstring>Bad input</faultstring></soap:Fault></soap:Body></soap:Envelope>";
        assert_eq!(fault_message(xml).as_deref(), Some("Bad input"));
        assert_eq!(fault_message("<ok/>"), None);
    }
}

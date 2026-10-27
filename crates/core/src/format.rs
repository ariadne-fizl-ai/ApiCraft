//! Pretty-printing for response bodies.

/// Re-indents JSON; returns `None` if the text is not JSON.
pub fn pretty_json(text: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    serde_json::to_string_pretty(&v).ok()
}

/// A small, forgiving XML indenter. It does not validate; it only puts each
/// tag on its own line, keeping short text content inline.
pub fn pretty_xml(text: &str) -> Option<String> {
    let t = text.trim();
    if !t.starts_with('<') {
        return None;
    }
    let mut out = String::with_capacity(t.len() * 2);
    let mut depth: usize = 0;
    let mut i = 0;
    let b = t.as_bytes();
    while i < b.len() {
        if b[i] == b'<' {
            let end = find_tag_end(t, i)?;
            let tag = &t[i..=end];
            let closing = tag.starts_with("</");
            let self_closing = tag.ends_with("/>") || tag.starts_with("<?") || tag.starts_with("<!");
            if closing {
                depth = depth.saturating_sub(1);
            }
            // keep <a>text</a> on one line
            let next_lt = t[end + 1..].find('<').map(|n| n + end + 1);
            if !closing && !self_closing {
                if let Some(n) = next_lt {
                    let inner = &t[end + 1..n];
                    if t[n..].starts_with("</") && !inner.contains('\n') {
                        let close_end = find_tag_end(t, n)?;
                        push_line(&mut out, depth, &format!("{tag}{}{}", inner.trim(), &t[n..=close_end]));
                        i = close_end + 1;
                        continue;
                    }
                }
            }
            push_line(&mut out, depth, tag);
            if !closing && !self_closing {
                depth += 1;
            }
            i = end + 1;
        } else {
            let next = t[i..].find('<').map(|n| n + i).unwrap_or(t.len());
            let txt = t[i..next].trim();
            if !txt.is_empty() {
                push_line(&mut out, depth, txt);
            }
            i = next;
        }
    }
    Some(out.trim_end().to_string())
}

fn find_tag_end(t: &str, start: usize) -> Option<usize> {
    if t[start..].starts_with("<!--") {
        return t[start..].find("-->").map(|n| start + n + 2);
    }
    if t[start..].starts_with("<![CDATA[") {
        return t[start..].find("]]>").map(|n| start + n + 2);
    }
    let mut quote: Option<u8> = None;
    for (off, &c) in t.as_bytes()[start..].iter().enumerate() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None if c == b'"' || c == b'\'' => quote = Some(c),
            None if c == b'>' => return Some(start + off),
            None => {}
        }
    }
    None
}

fn push_line(out: &mut String, depth: usize, s: &str) {
    for _ in 0..depth {
        out.push_str("  ");
    }
    out.push_str(s);
    out.push('\n');
}

/// Pretty-prints by content type, falling back to sniffing the text.
pub fn pretty_body(content_type: &str, text: &str) -> String {
    let ct = content_type.to_ascii_lowercase();
    if ct.contains("json") {
        if let Some(p) = pretty_json(text) {
            return p;
        }
    }
    if ct.contains("xml") || ct.contains("soap") {
        if let Some(p) = pretty_xml(text) {
            return p;
        }
    }
    if ct.contains("html") {
        return text.to_string();
    }
    pretty_json(text).or_else(|| if text.trim_start().starts_with("<?xml") { pretty_xml(text) } else { None }).unwrap_or_else(|| text.to_string())
}

pub fn human_size(n: usize) -> String {
    if n < 1024 {
        format!("{n} B")
    } else if n < 1024 * 1024 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else {
        format!("{:.1} MB", n as f64 / 1024.0 / 1024.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xml_indent() {
        let x = pretty_xml("<?xml version=\"1.0\"?><a><b>1</b><c x=\"a>b\"/></a>").unwrap();
        assert_eq!(x, "<?xml version=\"1.0\"?>\n<a>\n  <b>1</b>\n  <c x=\"a>b\"/>\n</a>");
    }
}

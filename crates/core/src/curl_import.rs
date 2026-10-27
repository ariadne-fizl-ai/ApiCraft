//! Paste a curl command, get a request.

use crate::model::{Auth, Body, KeyValue, Protocol, Request};
use crate::prepare::percent_decode;

/// Splits a shell command line: quotes, backslash escapes and `\`-newline
/// continuations (also PowerShell's backtick and cmd's `^` at line ends).
pub fn shell_words(input: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(ch) => cur.push(ch),
                        None => return Err("unterminated ' quote".into()),
                    }
                }
            }
            '"' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(ch @ ('"' | '\\' | '$' | '`')) => cur.push(ch),
                            Some('\n') => {}
                            Some(ch) => {
                                cur.push('\\');
                                cur.push(ch);
                            }
                            None => return Err("unterminated \" quote".into()),
                        },
                        Some(ch) => cur.push(ch),
                        None => return Err("unterminated \" quote".into()),
                    }
                }
            }
            '$' if chars.peek() == Some(&'\'') => {
                // ANSI-C quoting: $'...\n...'
                chars.next();
                in_word = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some('\\') => match chars.next() {
                            Some('n') => cur.push('\n'),
                            Some('t') => cur.push('\t'),
                            Some('r') => cur.push('\r'),
                            Some(ch) => cur.push(ch),
                            None => break,
                        },
                        Some(ch) => cur.push(ch),
                        None => return Err("unterminated $' quote".into()),
                    }
                }
            }
            '\\' | '`' | '^' if matches!(chars.peek(), Some('\n') | Some('\r')) => {
                while matches!(chars.peek(), Some('\n') | Some('\r')) {
                    chars.next();
                }
            }
            '\\' => {
                if let Some(ch) = chars.next() {
                    cur.push(ch);
                    in_word = true;
                }
            }
            c if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            c => {
                cur.push(c);
                in_word = true;
            }
        }
    }
    if in_word {
        words.push(cur);
    }
    Ok(words)
}

pub fn parse_curl(command: &str) -> Result<Request, String> {
    let words = shell_words(command.trim())?;
    let mut it = words.into_iter().peekable();
    match it.next() {
        Some(w) if w == "curl" || w.ends_with("/curl") || w.eq_ignore_ascii_case("curl.exe") => {}
        _ => return Err("expected a command starting with curl".into()),
    }
    let mut r = Request::new("Imported from curl", Protocol::Rest);
    let mut method: Option<String> = None;
    let mut data: Vec<String> = Vec::new();
    let mut form: Vec<KeyValue> = Vec::new();
    let mut get = false;
    let mut digest = false;
    let mut user: Option<String> = None;
    let mut url: Option<String> = None;

    while let Some(arg) = it.next() {
        // --opt=value
        let (flag, inline) = match arg.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f.to_string(), Some(v.to_string())),
            _ => (arg.clone(), None),
        };
        let value = |it: &mut std::iter::Peekable<std::vec::IntoIter<String>>| inline.clone().or_else(|| it.next()).unwrap_or_default();
        match flag.as_str() {
            "-X" | "--request" => method = Some(value(&mut it).to_uppercase()),
            "-H" | "--header" => {
                let h = value(&mut it);
                if let Some((k, v)) = h.split_once(':') {
                    r.headers.push(KeyValue::new(k.trim(), v.trim()));
                }
            }
            "-d" | "--data" | "--data-raw" | "--data-binary" | "--data-ascii" | "--json" => {
                let d = value(&mut it);
                if flag == "--json" {
                    r.headers.push(KeyValue::new("Content-Type", "application/json"));
                    r.headers.push(KeyValue::new("Accept", "application/json"));
                }
                data.push(d);
            }
            "--data-urlencode" => {
                let d = value(&mut it);
                data.push(match d.split_once('=') {
                    Some((k, v)) => format!("{k}={}", crate::prepare::percent_encode(v)),
                    None => crate::prepare::percent_encode(&d),
                });
            }
            "-F" | "--form" | "--form-string" => {
                let f = value(&mut it);
                let (k, v) = f.split_once('=').unwrap_or((&f, ""));
                form.push(KeyValue::new(k, v));
            }
            "-u" | "--user" => user = Some(value(&mut it)),
            "--digest" => digest = true,
            "--basic" => {}
            "-G" | "--get" => get = true,
            "-I" | "--head" => method = Some("HEAD".into()),
            "-A" | "--user-agent" => r.headers.push(KeyValue::new("User-Agent", value(&mut it))),
            "-e" | "--referer" => r.headers.push(KeyValue::new("Referer", value(&mut it))),
            "-b" | "--cookie" => r.headers.push(KeyValue::new("Cookie", value(&mut it))),
            "--url" => url = Some(value(&mut it)),
            "-o" | "--output" | "-m" | "--max-time" | "--connect-timeout" | "-w" | "--write-out" | "--retry" | "-x" | "--proxy" | "--cacert" | "--cert"
            | "--key" | "-c" | "--cookie-jar" => {
                let _ = value(&mut it);
            }
            f if f.starts_with('-') => {} // -L, -k, -s, -v, --compressed, ...
            _ => {
                if url.is_none() {
                    url = Some(arg);
                }
            }
        }
    }

    let url = url.ok_or("no URL in the curl command")?;
    match url.split_once('?') {
        Some((base, q)) => {
            r.url = base.into();
            r.params = q
                .split('&')
                .filter(|p| !p.is_empty())
                .map(|p| {
                    let (k, v) = p.split_once('=').unwrap_or((p, ""));
                    KeyValue::new(percent_decode(k), percent_decode(v))
                })
                .collect();
        }
        None => r.url = url,
    }
    if r.url.starts_with("ws://") || r.url.starts_with("wss://") {
        r.protocol = Protocol::WebSocket;
    }

    if get && !data.is_empty() {
        for d in data.drain(..) {
            for p in d.split('&').filter(|p| !p.is_empty()) {
                let (k, v) = p.split_once('=').unwrap_or((p, ""));
                r.params.push(KeyValue::new(percent_decode(k), percent_decode(v)));
            }
        }
    }

    let ct = r.headers.iter().find(|h| h.key.eq_ignore_ascii_case("content-type")).map(|h| h.value.clone());
    if !form.is_empty() {
        r.body = Body::FormData { fields: form };
        r.headers.retain(|h| !h.key.eq_ignore_ascii_case("content-type"));
    } else if !data.is_empty() {
        let joined = data.join("&");
        let is_form =
            ct.as_deref().map(|c| c.starts_with("application/x-www-form-urlencoded")).unwrap_or(true) && !joined.trim_start().starts_with(['{', '[', '<']);
        if is_form && joined.contains('=') {
            r.body = Body::UrlEncoded {
                fields: joined
                    .split('&')
                    .filter(|p| !p.is_empty())
                    .map(|p| {
                        let (k, v) = p.split_once('=').unwrap_or((p, ""));
                        KeyValue::new(percent_decode(k), percent_decode(v))
                    })
                    .collect(),
            };
            r.headers.retain(|h| !h.key.eq_ignore_ascii_case("content-type"));
        } else {
            r.body = Body::Raw { content_type: String::new(), text: joined };
        }
    }

    r.method = method.unwrap_or_else(|| if !matches!(r.body, Body::None) { "POST".into() } else { "GET".into() });

    if let Some(u) = user {
        let (name, pass) = u.split_once(':').unwrap_or((&u, ""));
        r.auth =
            if digest { Auth::Digest { username: name.into(), password: pass.into() } } else { Auth::Basic { username: name.into(), password: pass.into() } };
    } else if let Some(i) = r.headers.iter().position(|h| h.key.eq_ignore_ascii_case("authorization") && h.value.starts_with("Bearer ")) {
        r.auth = Auth::Bearer { token: r.headers[i].value[7..].trim().into() };
        r.headers.remove(i);
    }

    if let Some(i) = r.headers.iter().position(|h| h.key.eq_ignore_ascii_case("soapaction")) {
        r.protocol = Protocol::Soap;
        r.soap.action = r.headers[i].value.trim_matches('"').into();
        r.headers.remove(i);
        r.headers.retain(|h| !h.key.eq_ignore_ascii_case("content-type"));
    }

    if let Ok(u) = crate::url::Url::parse(&r.url) {
        let last = u.path.rsplit('/').find(|s| !s.is_empty()).unwrap_or(&u.host);
        r.name = format!("{} {}", r.method, last);
    }
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_typical_command() {
        let cmd = "curl -X POST 'https://api.test/v1/items?limit=10' \\\n  -H 'Content-Type: application/json' \\\n  -H \"Authorization: Bearer abc\" \\\n  --data-raw '{\"name\":\"it'\\''s\"}' --compressed";
        let r = parse_curl(cmd).unwrap();
        assert_eq!(r.method, "POST");
        assert_eq!(r.url, "https://api.test/v1/items");
        assert_eq!(r.params, vec![KeyValue::new("limit", "10")]);
        assert_eq!(r.auth, Auth::Bearer { token: "abc".into() });
        assert_eq!(r.body.raw_text(), "{\"name\":\"it's\"}");
        assert_eq!(r.name, "POST items");
    }

    #[test]
    fn form_user_and_get() {
        let r = parse_curl("curl -u bob:pw --digest -d a=1 -d b=x%20y http://h/form").unwrap();
        assert_eq!(r.auth, Auth::Digest { username: "bob".into(), password: "pw".into() });
        assert_eq!(r.body, Body::UrlEncoded { fields: vec![KeyValue::new("a", "1"), KeyValue::new("b", "x y")] });
        let g = parse_curl("curl -G -d q=rust https://h/search").unwrap();
        assert_eq!(g.method, "GET");
        assert_eq!(g.params, vec![KeyValue::new("q", "rust")]);
    }

    #[test]
    fn round_trips_our_own_output() {
        use crate::codegen::{generate, Target};
        use crate::model::*;
        let mut r = Request::new("r", Protocol::Rest);
        r.method = "PUT".into();
        r.url = "https://api.test/x".into();
        r.headers.push(KeyValue::new("X-A", "it's"));
        r.body = Body::Raw { content_type: "application/json".into(), text: "{\"a\":1}".into() };
        let mut cat = Catalog::default();
        let mut c = Collection::new("c");
        let id = r.id.clone();
        c.items.push(Item::Request(r));
        cat.collections.push(c);
        let back = parse_curl(&generate(&cat, &id, 0, Target::Curl).unwrap()).unwrap();
        assert_eq!(back.method, "PUT");
        assert_eq!(back.url, "https://api.test/x");
        assert_eq!(back.headers[0], KeyValue::new("X-A", "it's"));
        assert_eq!(back.body.raw_text(), "{\"a\":1}");
    }
}

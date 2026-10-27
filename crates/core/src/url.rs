//! Just enough URL parsing for signing and code generation.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Url {
    pub scheme: String,
    pub host: String,
    pub port: Option<u16>,
    /// As written (already percent-encoded), at least "/".
    pub path: String,
    /// Without the '?'.
    pub query: String,
}

impl Url {
    pub fn parse(s: &str) -> Result<Url, String> {
        let s = s.trim();
        let (scheme, rest) = s.split_once("://").ok_or_else(|| format!("URL needs a scheme (http://, https://, ws://...): {s}"))?;
        let rest = rest.split('#').next().unwrap_or("");
        let (authority, path_query) = match rest.find(['/', '?']) {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, ""),
        };
        let authority = authority.rsplit('@').next().unwrap_or(authority);
        if authority.is_empty() {
            return Err(format!("URL has no host: {s}"));
        }
        let (host, port) = if let Some(stripped) = authority.strip_prefix('[') {
            let end = stripped.find(']').ok_or("bad IPv6 host")?;
            let host = format!("[{}]", &stripped[..end]);
            let port = stripped[end + 1..].strip_prefix(':').map(|p| p.parse::<u16>().map_err(|_| format!("bad port in {s}"))).transpose()?;
            (host, port)
        } else {
            match authority.rsplit_once(':') {
                Some((h, p)) => (h.to_string(), Some(p.parse::<u16>().map_err(|_| format!("bad port in {s}"))?)),
                None => (authority.to_string(), None),
            }
        };
        let (path, query) = match path_query.split_once('?') {
            Some((p, q)) => (p, q),
            None => (path_query, ""),
        };
        Ok(Url {
            scheme: scheme.to_ascii_lowercase(),
            host: host.to_ascii_lowercase(),
            port,
            path: if path.is_empty() { "/".into() } else { path.into() },
            query: query.into(),
        })
    }

    pub fn default_port(&self) -> u16 {
        match self.scheme.as_str() {
            "https" | "wss" | "grpcs" => 443,
            _ => 80,
        }
    }

    pub fn port_or_default(&self) -> u16 {
        self.port.unwrap_or_else(|| self.default_port())
    }

    pub fn is_tls(&self) -> bool {
        matches!(self.scheme.as_str(), "https" | "wss" | "grpcs")
    }

    /// `host[:port]`, omitting the default port, as the Host header has it.
    pub fn host_header(&self) -> String {
        match self.port {
            Some(p) if p != self.default_port() => format!("{}:{p}", self.host),
            _ => self.host.clone(),
        }
    }

    /// `scheme://host[:port]/path` without the query.
    pub fn base(&self) -> String {
        format!("{}://{}{}", self.scheme, self.host_header(), self.path)
    }

    pub fn path_and_query(&self) -> String {
        if self.query.is_empty() {
            self.path.clone()
        } else {
            format!("{}?{}", self.path, self.query)
        }
    }

    /// Decoded query pairs.
    pub fn query_pairs(&self) -> Vec<(String, String)> {
        parse_query(&self.query)
    }
}

pub fn parse_query(q: &str) -> Vec<(String, String)> {
    q.split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (crate::prepare::percent_decode(k), crate::prepare::percent_decode(v))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses() {
        let u = Url::parse("https://user:pw@API.example.com:8443/v1/items?a=1&b=x%20y#frag").unwrap();
        assert_eq!(u.host, "api.example.com");
        assert_eq!(u.port, Some(8443));
        assert_eq!(u.path, "/v1/items");
        assert_eq!(u.query_pairs(), vec![("a".into(), "1".into()), ("b".into(), "x y".into())]);
        assert_eq!(u.host_header(), "api.example.com:8443");
        let u = Url::parse("http://localhost").unwrap();
        assert_eq!(u.path, "/");
        assert_eq!(u.port_or_default(), 80);
        assert!(Url::parse("example.com").is_err());
    }
}

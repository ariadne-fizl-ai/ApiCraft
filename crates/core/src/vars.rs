//! `{{variable}}` substitution, Postman style.

use std::collections::HashMap;

use crate::clock;
use crate::ids::new_id;
use crate::model::{Catalog, Collection, KeyValue};

#[derive(Debug, Default, Clone)]
pub struct Vars {
    map: HashMap<String, String>,
    /// Unix ms used for `{{$timestamp}}` and friends; 0 disables them.
    now_ms: u64,
}

impl Vars {
    /// Precedence, lowest first: collection, workflow, environment, runtime
    /// (values captured from responses).
    pub fn for_request(catalog: &Catalog, collection: Option<&Collection>, workflow_vars: &[KeyValue], now_ms: u64) -> Vars {
        let mut v = Vars { map: HashMap::new(), now_ms };
        if let Some(c) = collection {
            v.extend(&c.variables);
        }
        v.extend(workflow_vars);
        if let Some(env) = catalog.active_env() {
            v.extend(&env.variables);
        }
        v.extend(&catalog.runtime);
        v
    }

    pub fn with_now(mut self, now_ms: u64) -> Self {
        self.now_ms = now_ms;
        self
    }

    pub fn extend(&mut self, kvs: &[KeyValue]) {
        for kv in kvs.iter().filter(|kv| kv.enabled && !kv.key.is_empty()) {
            self.map.insert(kv.key.clone(), kv.value.clone());
        }
    }

    pub fn set(&mut self, key: &str, value: &str) {
        self.map.insert(key.into(), value.into());
    }

    pub fn get(&self, key: &str) -> Option<String> {
        if let Some(v) = self.map.get(key) {
            return Some(v.clone());
        }
        self.dynamic(key)
    }

    /// Postman's dynamic variables.
    fn dynamic(&self, key: &str) -> Option<String> {
        let rand = || u64::from_str_radix(&new_id()[..8], 36).unwrap_or(7);
        Some(match key {
            "$guid" | "$randomUUID" => uuid_v4(),
            "$timestamp" if self.now_ms > 0 => (self.now_ms / 1000).to_string(),
            "$isoTimestamp" if self.now_ms > 0 => clock::iso8601(self.now_ms),
            "$randomInt" => (rand() % 1001).to_string(),
            "$randomAlphaNumeric" => new_id()[..1].to_string(),
            _ => return None,
        })
    }

    /// Replaces `{{name}}` with its value. Unknown names are left as written,
    /// so the problem shows in the request rather than going silently empty.
    pub fn apply(&self, input: &str) -> String {
        let mut out = String::with_capacity(input.len());
        let mut rest = input;
        while let Some(start) = rest.find("{{") {
            out.push_str(&rest[..start]);
            let after = &rest[start + 2..];
            match after.find("}}") {
                Some(end) => {
                    let name = after[..end].trim();
                    match self.get(name) {
                        Some(v) => out.push_str(&v),
                        None => out.push_str(&rest[start..start + 2 + end + 2]),
                    }
                    rest = &after[end + 2..];
                }
                None => {
                    out.push_str(&rest[start..]);
                    rest = "";
                }
            }
        }
        out.push_str(rest);
        out
    }

    /// Names used in `input` that have no value.
    pub fn missing(&self, input: &str) -> Vec<String> {
        let mut found = Vec::new();
        let mut rest = input;
        while let Some(start) = rest.find("{{") {
            let after = &rest[start + 2..];
            let Some(end) = after.find("}}") else { break };
            let name = after[..end].trim().to_string();
            if self.get(&name).is_none() && !found.contains(&name) {
                found.push(name);
            }
            rest = &after[end + 2..];
        }
        found
    }
}

pub fn uuid_v4() -> String {
    let hex: String = (0..3).map(|_| new_id()).collect::<String>().bytes().map(|b| format!("{:x}", b % 16)).take(32).collect();
    let mut chars: Vec<char> = hex.chars().collect();
    chars[12] = '4';
    chars[16] = ['8', '9', 'a', 'b'][(chars[16].to_digit(16).unwrap_or(0) % 4) as usize];
    let s: String = chars.into_iter().collect();
    format!("{}-{}-{}-{}-{}", &s[..8], &s[8..12], &s[12..16], &s[16..20], &s[20..32])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substitutes() {
        let mut v = Vars::default();
        v.set("host", "example.com");
        assert_eq!(v.apply("https://{{host}}/{{ missing }}/x"), "https://example.com/{{ missing }}/x");
        assert_eq!(v.missing("{{host}} {{a}} {{a}}"), vec!["a".to_string()]);
        assert_eq!(v.apply("{{unterminated"), "{{unterminated");
        let v = v.with_now(1_000_000);
        assert_eq!(v.apply("{{$timestamp}}"), "1000");
        assert_eq!(v.apply("{{$guid}}").len(), 36);
    }

    #[test]
    fn runtime_overrides_environment() {
        let mut cat = Catalog::default();
        let mut env = crate::model::Environment::new("dev");
        env.variables.push(KeyValue::new("token", "env"));
        cat.active_environment = Some(env.id.clone());
        cat.environments.push(env);
        assert_eq!(Vars::for_request(&cat, None, &[], 0).apply("{{token}}"), "env");
        cat.runtime.push(KeyValue::new("token", "captured"));
        assert_eq!(Vars::for_request(&cat, None, &[], 0).apply("{{token}}"), "captured");
    }
}

//! Environment variables whose values come from running a command: any
//! command line, or the Bitwarden CLI (`bw`) for secrets. The commands
//! themselves run on the host (see [`Transport::run_command`]); this module
//! builds them and reads their output.
//!
//! Fetched values are only held in memory and injected into a copy of the
//! catalog for sending, so secrets never end up in the saved catalog.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::http::Transport;
use crate::model::{Catalog, KeyValue, SourceKind, VarSource};

/// A process to start.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CommandSpec {
    /// A command line for the system shell (`sh -c` or `cmd /C`). When
    /// empty, `program` runs directly with `args`.
    pub shell: String,
    pub program: String,
    pub args: Vec<String>,
    /// Extra environment variables for the process.
    pub env: Vec<(String, String)>,
    /// Working folder; empty for the host's current one.
    pub cwd: String,
    /// 0 means the default (60 seconds).
    pub timeout_ms: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CommandOutput {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

impl CommandOutput {
    pub fn ok(&self) -> bool {
        self.status == 0
    }

    /// What went wrong, for a failed command.
    pub fn failure(&self) -> String {
        let err = self.stderr.trim();
        let err = if err.is_empty() { self.stdout.trim() } else { err };
        if err.is_empty() {
            format!("exited with status {}", self.status)
        } else {
            err.lines().take(4).collect::<Vec<_>>().join(" ")
        }
    }
}

/// The Bitwarden fields `bw get` reads directly; anything else is a custom field.
pub const BITWARDEN_FIELDS: [&str; 5] = ["password", "username", "totp", "notes", "uri"];

/// The environment variable `bw unlock` reads the master password from.
const BW_PASSWORD_ENV: &str = "APICRAFT_BW_PASSWORD";

fn bw(args: &[&str], session: &str) -> CommandSpec {
    let mut env = Vec::new();
    if !session.is_empty() {
        env.push(("BW_SESSION".to_string(), session.to_string()));
    }
    let mut args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    args.push("--nointeraction".into());
    CommandSpec { program: "bw".into(), args, env, ..Default::default() }
}

/// The command that reads a source's value. `bw_session` is the key from
/// `bw unlock` (or empty when `BW_SESSION` is already set for the host).
pub fn spec_for(src: &VarSource, bw_session: &str) -> Result<CommandSpec, String> {
    match src.kind {
        SourceKind::Command => {
            let line = src.command.trim();
            if line.is_empty() {
                return Err(format!("{}: no command given", src.key));
            }
            Ok(CommandSpec { shell: line.to_string(), ..Default::default() })
        }
        SourceKind::Bitwarden => {
            let item = src.item.trim();
            if item.is_empty() {
                return Err(format!("{}: no Bitwarden item given", src.key));
            }
            let field = bitwarden_field(src);
            Ok(if BITWARDEN_FIELDS.contains(&field.as_str()) { bw(&["get", &field, item], bw_session) } else { bw(&["get", "item", item], bw_session) })
        }
    }
}

fn bitwarden_field(src: &VarSource) -> String {
    let f = src.field.trim();
    if f.is_empty() {
        "password".into()
    } else if BITWARDEN_FIELDS.contains(&f.to_ascii_lowercase().as_str()) {
        f.to_ascii_lowercase()
    } else {
        f.to_string()
    }
}

/// Reads the value out of a source command's output.
pub fn value_from(src: &VarSource, out: &CommandOutput) -> Result<String, String> {
    if !out.ok() {
        let why = out.failure();
        if src.kind == SourceKind::Bitwarden {
            let lower = why.to_ascii_lowercase();
            if lower.contains("locked") || lower.contains("master password") {
                return Err("Bitwarden is locked: unlock it in Environments \u{2192} Values from commands".into());
            }
            if lower.contains("not logged in") {
                return Err("Bitwarden isn't logged in: run `bw login` once in a terminal".into());
            }
        }
        return Err(why);
    }
    let text = out.stdout.trim_end_matches(['\r', '\n']);
    match src.kind {
        SourceKind::Command => Ok(text.to_string()),
        SourceKind::Bitwarden => {
            let field = bitwarden_field(src);
            if BITWARDEN_FIELDS.contains(&field.as_str()) {
                return Ok(text.to_string());
            }
            let item: serde_json::Value = serde_json::from_str(text).map_err(|e| format!("unexpected output from bw: {e}"))?;
            let fields = item["fields"].as_array().cloned().unwrap_or_default();
            match fields.iter().find(|f| f["name"].as_str().is_some_and(|n| n.eq_ignore_ascii_case(&field))) {
                Some(f) => Ok(f["value"].as_str().unwrap_or_default().to_string()),
                None => {
                    let names: Vec<&str> = fields.iter().filter_map(|f| f["name"].as_str()).collect();
                    Err(if names.is_empty() {
                        format!("the item has no field called {field:?}")
                    } else {
                        format!("the item has no field called {field:?} (it has {})", names.join(", "))
                    })
                }
            }
        }
    }
}

/// `bw unlock`, giving the session key on stdout. The password travels in an
/// environment variable of the child process, never on its command line.
pub fn bitwarden_unlock(password: &str) -> CommandSpec {
    let mut spec = bw(&["unlock", "--raw", "--passwordenv", BW_PASSWORD_ENV], "");
    spec.env.push((BW_PASSWORD_ENV.into(), password.into()));
    spec
}

/// `bw sync`: pulls the latest vault.
pub fn bitwarden_sync(session: &str) -> CommandSpec {
    bw(&["sync"], session)
}

/// `bw status`.
pub fn bitwarden_status(session: &str) -> CommandSpec {
    bw(&["status"], session)
}

/// Reads `bw status` output: (`unlocked` / `locked` / `unauthenticated`, account email).
pub fn parse_bitwarden_status(out: &CommandOutput) -> Result<(String, String), String> {
    if !out.ok() {
        return Err(out.failure());
    }
    let v: serde_json::Value = serde_json::from_str(out.stdout.trim()).map_err(|e| format!("unexpected output from bw: {e}"))?;
    Ok((v["status"].as_str().unwrap_or("unknown").to_string(), v["userEmail"].as_str().unwrap_or_default().to_string()))
}

/// The active environment's enabled sources.
pub fn active_sources(cat: &Catalog) -> Vec<VarSource> {
    cat.active_env().map(|e| e.sources.iter().filter(|s| s.enabled && !s.key.trim().is_empty()).cloned().collect()).unwrap_or_default()
}

/// Puts values into the active environment of `cat` (a copy made for
/// sending), overriding variables of the same name.
pub fn inject(cat: &mut Catalog, values: &HashMap<String, String>) {
    let Some(id) = cat.active_environment.clone() else { return };
    let Some(env) = cat.environments.iter_mut().find(|e| e.id == id) else { return };
    for src in env.sources.iter().filter(|s| s.enabled) {
        let key = src.key.trim();
        let Some(value) = values.get(key) else { continue };
        match env.variables.iter_mut().find(|kv| kv.key == key) {
            Some(kv) => {
                kv.value = value.clone();
                kv.enabled = true;
            }
            None => env.variables.push(KeyValue::new(key, value.clone())),
        }
    }
}

/// What [`resolve`] fetched.
#[derive(Debug, Clone, Default)]
pub struct Resolved {
    /// Newly fetched values, to keep for next time.
    pub fetched: Vec<(String, String)>,
    pub errors: Vec<String>,
}

/// Fills in the active environment's command-backed variables: from `known`
/// where already fetched, otherwise by running their commands.
pub async fn resolve(cat: &mut Catalog, known: &HashMap<String, String>, bw_session: &str, transport: &dyn Transport) -> Resolved {
    let mut out = Resolved::default();
    let mut values = known.clone();
    for src in active_sources(cat) {
        let key = src.key.trim().to_string();
        if values.contains_key(&key) {
            continue;
        }
        match fetch(&src, bw_session, transport).await {
            Ok(v) => {
                out.fetched.push((key.clone(), v.clone()));
                values.insert(key, v);
            }
            Err(e) => out.errors.push(format!("{{{{{key}}}}}: {e}")),
        }
    }
    inject(cat, &values);
    out
}

/// Runs one source's command and reads its value.
pub async fn fetch(src: &VarSource, bw_session: &str, transport: &dyn Transport) -> Result<String, String> {
    let spec = spec_for(src, bw_session)?;
    let out = transport.run_command(spec).await.map_err(|e| explain_missing(&e))?;
    value_from(src, &out)
}

/// A friendlier message when the Bitwarden CLI isn't installed.
pub fn explain_missing(e: &str) -> String {
    if e.starts_with("could not run bw") && (e.contains("not found") || e.contains("cannot find")) {
        "the Bitwarden CLI (bw) isn't installed or isn't on PATH; see bitwarden.com/help/cli".into()
    } else {
        e.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Environment;

    fn src(kind: SourceKind, field: &str) -> VarSource {
        VarSource { key: "secret".into(), kind, command: "echo hi".into(), item: "My API".into(), field: field.into(), enabled: true }
    }

    #[test]
    fn builds_bitwarden_commands() {
        let s = spec_for(&src(SourceKind::Bitwarden, "Password"), "KEY").unwrap();
        assert_eq!(s.program, "bw");
        assert_eq!(s.args, ["get", "password", "My API", "--nointeraction"]);
        assert_eq!(s.env, [("BW_SESSION".to_string(), "KEY".to_string())]);
        let s = spec_for(&src(SourceKind::Bitwarden, "api_key"), "").unwrap();
        assert_eq!(s.args, ["get", "item", "My API", "--nointeraction"]);
        assert!(s.env.is_empty());
        let unlock = bitwarden_unlock("pw");
        assert!(!unlock.args.iter().any(|a| a == "pw"), "the password never goes on the command line");
    }

    #[test]
    fn reads_values_and_custom_fields() {
        let ok = |stdout: &str| CommandOutput { status: 0, stdout: stdout.into(), stderr: String::new() };
        assert_eq!(value_from(&src(SourceKind::Command, ""), &ok("hello\r\n")).unwrap(), "hello");
        let item = r#"{"fields":[{"name":"API_KEY","value":"abc"}],"login":{"password":"pw"}}"#;
        assert_eq!(value_from(&src(SourceKind::Bitwarden, "api_key"), &ok(item)).unwrap(), "abc");
        assert!(value_from(&src(SourceKind::Bitwarden, "other"), &ok(item)).unwrap_err().contains("API_KEY"));
        let locked = CommandOutput { status: 1, stdout: String::new(), stderr: "Vault is locked.".into() };
        assert!(value_from(&src(SourceKind::Bitwarden, "password"), &locked).unwrap_err().contains("unlock"));
    }

    #[test]
    fn injects_into_the_active_environment() {
        let mut env = Environment::new("dev");
        env.variables.push(KeyValue::new("secret", "placeholder"));
        env.sources.push(src(SourceKind::Command, ""));
        let mut cat = Catalog { active_environment: Some(env.id.clone()), environments: vec![env], ..Default::default() };
        inject(&mut cat, &HashMap::from([("secret".to_string(), "real".to_string())]));
        assert_eq!(cat.environments[0].variables, [KeyValue::new("secret", "real")]);
    }
}

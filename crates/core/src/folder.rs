//! A collection as a folder of text files, so it can live in version
//! control next to the code it tests:
//!
//! ```text
//! my-api/
//!   collection.json            name, variables, auth, and the order of what's below
//!   Get users.request.json     one file per request
//!   Admin/                     a folder
//!     folder.json              its name and order
//!     Delete user.request.json
//!   Sign in.workflow.json      workflows whose steps start in this collection
//! ```
//!
//! Every file is pretty-printed JSON. Files added by hand are picked up
//! (after the ones listed in the order), and ids are made up when missing.
//! This module only converts; reading and writing the disk is the host's job.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ids::new_id;
use crate::model::*;

pub const COLLECTION_FILE: &str = "collection.json";
pub const FOLDER_FILE: &str = "folder.json";
pub const REQUEST_EXT: &str = ".request.json";
pub const WORKFLOW_EXT: &str = ".workflow.json";
const FORMAT: &str = "fizl-apicraft-collection";

/// True for the files this format owns (the only ones it ever deletes).
pub fn is_owned_file(name: &str) -> bool {
    name == COLLECTION_FILE || name == FOLDER_FILE || name.ends_with(REQUEST_EXT) || name.ends_with(WORKFLOW_EXT)
}

#[derive(Serialize, Deserialize)]
struct CollectionFile {
    #[serde(default)]
    format: String,
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    description: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    variables: Vec<KeyValue>,
    #[serde(default = "no_auth")]
    auth: Auth,
    /// File and folder names, in sidebar order.
    #[serde(default)]
    order: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    workflows: Vec<String>,
}

fn no_auth() -> Auth {
    Auth::None
}

#[derive(Serialize, Deserialize)]
struct FolderFile {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    order: Vec<String>,
}

/// A name that's safe as a file name on Windows, macOS and Linux.
pub fn file_stem(name: &str) -> String {
    let cleaned: String =
        name.chars().map(|c| if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || c.is_control() { '_' } else { c }).collect();
    let cleaned = cleaned.trim().trim_end_matches(['.', ' ']).to_string();
    let reserved = ["CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "LPT1", "LPT2", "LPT3"];
    let cleaned = if reserved.iter().any(|r| r.eq_ignore_ascii_case(&cleaned)) { format!("{cleaned}_") } else { cleaned };
    let cleaned: String = cleaned.chars().take(120).collect();
    if cleaned.is_empty() {
        "Untitled".into()
    } else {
        cleaned
    }
}

/// Unique names within one directory (case-insensitively, for Windows and macOS).
struct Names(HashSet<String>);

impl Names {
    fn take(&mut self, stem: &str, ext: &str) -> String {
        let mut n = 1;
        loop {
            let candidate = if n == 1 { format!("{stem}{ext}") } else { format!("{stem} ({n}){ext}") };
            if self.0.insert(candidate.to_lowercase()) {
                return candidate;
            }
            n += 1;
        }
    }
}

fn pretty<T: Serialize>(v: &T) -> String {
    serde_json::to_string_pretty(v).expect("serialises") + "\n"
}

/// The files for a collection and its workflows: `(relative path, text)`,
/// with `/` separators.
pub fn to_files(col: &Collection, workflows: &[&Workflow]) -> Vec<(String, String)> {
    fn write_items(dir: &str, items: &[Item], out: &mut Vec<(String, String)>) -> Vec<String> {
        let mut names = Names(HashSet::from([COLLECTION_FILE.to_string(), FOLDER_FILE.to_string()]));
        let mut order = Vec::new();
        for item in items {
            match item {
                Item::Request(r) => {
                    let file = names.take(&file_stem(&r.name), REQUEST_EXT);
                    out.push((format!("{dir}{file}"), pretty(r)));
                    order.push(file);
                }
                Item::Folder(f) => {
                    let sub = names.take(&file_stem(&f.name), "");
                    let inner = write_items(&format!("{dir}{sub}/"), &f.items, out);
                    out.push((format!("{dir}{sub}/{FOLDER_FILE}"), pretty(&FolderFile { id: f.id.clone(), name: f.name.clone(), order: inner })));
                    order.push(format!("{sub}/"));
                }
            }
        }
        order
    }
    let mut out = Vec::new();
    let order = write_items("", &col.items, &mut out);
    let mut names = Names(out.iter().filter(|(p, _)| !p.contains('/')).map(|(p, _)| p.to_lowercase()).collect());
    let mut wf_files = Vec::new();
    for w in workflows {
        let file = names.take(&file_stem(&w.name), WORKFLOW_EXT);
        out.push((file.clone(), pretty(w)));
        wf_files.push(file);
    }
    let header = CollectionFile {
        format: FORMAT.into(),
        id: col.id.clone(),
        name: col.name.clone(),
        description: col.description.clone(),
        variables: col.variables.clone(),
        auth: col.auth.clone(),
        order,
        workflows: wf_files,
    };
    out.insert(0, (COLLECTION_FILE.to_string(), pretty(&header)));
    out
}

/// Reads a collection back from its files (`(relative path, text)`).
/// `dir_name` names it when there's no `collection.json`. Problems with
/// single files are returned as warnings rather than failing the whole read.
pub fn from_files(files: &[(String, String)], dir_name: &str) -> (Collection, Vec<Workflow>, Vec<String>) {
    let mut warnings = Vec::new();
    let by_path: HashMap<&str, &str> = files.iter().map(|(p, t)| (p.as_str(), t.as_str())).collect();
    let header: Option<CollectionFile> = by_path.get(COLLECTION_FILE).and_then(|t| match serde_json::from_str(t) {
        Ok(h) => Some(h),
        Err(e) => {
            warnings.push(format!("{COLLECTION_FILE}: {e}"));
            None
        }
    });
    let mut col = Collection::new(dir_name);
    let mut wf_order = Vec::new();
    let mut root_order = Vec::new();
    if let Some(h) = header {
        if !h.id.is_empty() {
            col.id = h.id;
        }
        if !h.name.is_empty() {
            col.name = h.name;
        }
        col.description = h.description;
        col.variables = h.variables;
        col.auth = h.auth;
        root_order = h.order;
        wf_order = h.workflows;
    }
    let mut seen_ids = HashSet::new();
    col.items = read_dir("", &root_order, files, &by_path, &mut seen_ids, &mut warnings);

    // Workflows: listed ones first, then any others at the top level.
    let mut wf_files: Vec<&str> = wf_order.iter().map(String::as_str).filter(|f| by_path.contains_key(f)).collect();
    let mut extra: Vec<&str> =
        files.iter().map(|(p, _)| p.as_str()).filter(|p| !p.contains('/') && p.ends_with(WORKFLOW_EXT) && !wf_files.contains(p)).collect();
    extra.sort_by_key(|p| p.to_lowercase());
    wf_files.extend(extra);
    let mut workflows = Vec::new();
    for f in wf_files {
        match serde_json::from_str::<Value>(by_path[f]).map(|v| with_id(v, &mut seen_ids)).and_then(serde_json::from_value::<Workflow>) {
            Ok(w) => workflows.push(w),
            Err(e) => warnings.push(format!("{f}: {e}")),
        }
    }
    (col, workflows, warnings)
}

/// Gives an item read from a file an id when it has none, or a new one
/// when another file already used it (a copied file).
fn with_id(mut v: Value, seen: &mut HashSet<String>) -> Value {
    if let Value::Object(o) = &mut v {
        let id = o.get("id").and_then(|i| i.as_str()).unwrap_or_default().to_string();
        if id.is_empty() || !seen.insert(id) {
            let fresh = new_id();
            seen.insert(fresh.clone());
            o.insert("id".into(), Value::String(fresh));
        }
        if o.get("name").and_then(|n| n.as_str()).is_none() {
            o.insert("name".into(), Value::String("Untitled".into()));
        }
    }
    v
}

fn read_dir(
    dir: &str,
    order: &[String],
    files: &[(String, String)],
    by_path: &HashMap<&str, &str>,
    seen: &mut HashSet<String>,
    warnings: &mut Vec<String>,
) -> Vec<Item> {
    // What sits directly in this directory: request files and subdirectories.
    let mut entries: Vec<String> = Vec::new();
    for (p, _) in files {
        let Some(rest) = p.strip_prefix(dir) else { continue };
        let entry = match rest.split_once('/') {
            Some((sub, _)) => format!("{sub}/"),
            None if rest.ends_with(REQUEST_EXT) => rest.to_string(),
            None => continue,
        };
        if !entries.contains(&entry) {
            entries.push(entry);
        }
    }
    let mut ordered: Vec<String> = order.iter().filter(|o| entries.contains(o)).cloned().collect();
    let mut rest: Vec<String> = entries.into_iter().filter(|e| !ordered.contains(e)).collect();
    rest.sort_by_key(|e| e.to_lowercase());
    ordered.extend(rest);

    let mut items = Vec::new();
    for entry in ordered {
        if let Some(sub) = entry.strip_suffix('/') {
            let path = format!("{dir}{sub}/");
            let meta: Option<FolderFile> = by_path.get(format!("{path}{FOLDER_FILE}").as_str()).and_then(|t| serde_json::from_str(t).ok());
            let (id, name, order) = match meta {
                Some(m) => (m.id, if m.name.is_empty() { sub.to_string() } else { m.name }, m.order),
                None => (String::new(), sub.to_string(), Vec::new()),
            };
            let children = read_dir(&path, &order, files, by_path, seen, warnings);
            // Directories with nothing of ours in them aren't folders.
            if children.is_empty() && !by_path.contains_key(format!("{path}{FOLDER_FILE}").as_str()) {
                continue;
            }
            let id = if id.is_empty() || !seen.insert(id.clone()) { new_id() } else { id };
            items.push(Item::Folder(Folder { id, name, items: children }));
        } else {
            let path = format!("{dir}{entry}");
            match serde_json::from_str::<Value>(by_path[path.as_str()]).map(|v| with_id(v, seen)).and_then(serde_json::from_value::<Request>) {
                Ok(r) => items.push(Item::Request(r)),
                Err(e) => warnings.push(format!("{path}: {e}")),
            }
        }
    }
    items
}

/// The workflows a linked collection stores: those whose first step's
/// request is in it.
pub fn owned_workflows<'a>(catalog: &'a Catalog, collection_id: &str) -> Vec<&'a Workflow> {
    catalog.workflows.iter().filter(|w| w.steps.first().and_then(|s| catalog.collection_of(&s.request_id)).is_some_and(|c| c.id == collection_id)).collect()
}

/// The catalog as stored in the main file: linked collections reduced to
/// their link (id, name, directory), and their workflows left out.
pub fn stub_linked(catalog: &Catalog) -> Catalog {
    let mut out = catalog.clone();
    let owned: HashSet<String> =
        catalog.collections.iter().filter(|c| c.directory.is_some()).flat_map(|c| owned_workflows(catalog, &c.id).into_iter().map(|w| w.id.clone())).collect();
    out.workflows.retain(|w| !owned.contains(&w.id));
    for c in out.collections.iter_mut().filter(|c| c.directory.is_some()) {
        c.items.clear();
        c.variables.clear();
        c.description.clear();
        c.auth = Auth::None;
    }
    out
}

/// Puts a collection read from its folder into the catalog, in place of
/// its link, with its workflows.
pub fn merge_linked(catalog: &mut Catalog, link_id: &str, mut col: Collection, workflows: Vec<Workflow>) {
    let Some(pos) = catalog.collections.iter().position(|c| c.id == link_id) else { return };
    col.directory = catalog.collections[pos].directory.clone();
    // The link's id wins, so sidebar state and references survive.
    col.id = link_id.to_string();
    catalog.collections[pos] = col;
    let ids: HashSet<String> = workflows.iter().map(|w| w.id.clone()).collect();
    catalog.workflows.retain(|w| !ids.contains(&w.id));
    catalog.workflows.extend(workflows);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> (Collection, Workflow) {
        let mut col = Collection::new("My API");
        col.variables.push(KeyValue::new("base", "https://api.test"));
        let a = Request::new("List: users?", Protocol::Rest);
        let b = Request::new("List: users?", Protocol::Rest);
        let mut folder = Folder::new("Admin");
        folder.items.push(Item::Request(Request::new("Delete", Protocol::Rest)));
        let mut wf = Workflow::new("Smoke");
        wf.steps.push(WorkflowStep::new(a.id.clone()));
        col.items = vec![Item::Request(a), Item::Folder(folder), Item::Request(b)];
        (col, wf)
    }

    #[test]
    fn round_trips() {
        let (col, wf) = sample();
        let files = to_files(&col, &[&wf]);
        let paths: Vec<&str> = files.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(
            paths,
            [
                "collection.json",
                "List_ users_.request.json",
                "Admin/Delete.request.json",
                "Admin/folder.json",
                "List_ users_ (2).request.json",
                "Smoke.workflow.json"
            ]
        );
        let (back, wfs, warnings) = from_files(&files, "ignored");
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(back, col);
        assert_eq!(wfs, vec![wf]);
    }

    #[test]
    fn picks_up_hand_made_files() {
        let (col, wf) = sample();
        let mut files = to_files(&col, &[&wf]);
        // A request written by hand (no id), a copied file (duplicate id),
        // a new folder without folder.json, and a broken file.
        files.push(("Zed.request.json".into(), r#"{"name": "Zed", "url": "https://x"}"#.into()));
        let copy = files[1].1.clone();
        files.push(("Copy.request.json".into(), copy));
        files.push(("New/One.request.json".into(), r#"{"name": "One"}"#.into()));
        files.push(("Broken.request.json".into(), "{".into()));
        files.push(("notes.txt".into(), "ignored".into()));
        let (back, _, warnings) = from_files(&files, "dir");
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        let names: Vec<&str> = back.items.iter().map(|i| i.name()).collect();
        assert_eq!(names, ["List: users?", "Admin", "List: users?", "List: users?", "New", "Zed"]);
        let ids: HashSet<&str> = back.items.iter().map(|i| i.id()).collect();
        assert_eq!(ids.len(), 6, "every item has its own id");
    }

    #[test]
    fn names_without_a_collection_file() {
        let (col, _, _) = from_files(&[], "orders-api");
        assert_eq!(col.name, "orders-api");
        assert!(col.items.is_empty());
        assert_eq!(file_stem(" con "), "con_");
        assert_eq!(file_stem("a/b:c."), "a_b_c");
        assert_eq!(file_stem("..."), "Untitled");
    }

    #[test]
    fn stubs_and_merges() {
        let (mut col, wf) = sample();
        col.directory = Some("D:/api".into());
        let mut cat = Catalog::default();
        let cid = col.id.clone();
        cat.collections.push(col.clone());
        cat.workflows.push(wf.clone());
        cat.workflows.push(Workflow::new("Elsewhere"));
        let stub = stub_linked(&cat);
        assert!(stub.collections[0].items.is_empty());
        assert_eq!(stub.workflows.len(), 1);
        let mut back = stub.clone();
        merge_linked(&mut back, &cid, col.clone(), vec![wf]);
        assert_eq!(back.collections[0], col);
        assert_eq!(back.workflows.len(), 2);
    }
}

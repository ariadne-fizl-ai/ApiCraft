//! Collections kept as folders of text files (see [`apicraft_core::folder`]):
//! reading and writing them, and catalogs that link to them.

use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use apicraft_core::folder::{self, is_owned_file};
use apicraft_core::model::{Catalog, Collection, Workflow};
use apicraft_core::store;

/// How deep folders are followed, against symlink loops and huge trees.
const MAX_DEPTH: usize = 12;

/// The files of ours under `dir`, as `(relative path with '/', text)`.
fn read_files(dir: &Path) -> Result<Vec<(String, String)>, String> {
    fn walk(base: &Path, dir: &Path, depth: usize, out: &mut Vec<(String, String)>) -> Result<(), String> {
        let entries = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let ft = entry.file_type().map_err(|e| e.to_string())?;
            if ft.is_dir() && depth < MAX_DEPTH {
                walk(base, &path, depth + 1, out)?;
            } else if ft.is_file() && is_owned_file(&name) {
                let rel =
                    path.strip_prefix(base).unwrap_or(&path).components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/");
                let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
                out.push((rel, text));
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(dir, dir, 0, &mut out)?;
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// Reads the collection in a folder. An empty or new folder gives an empty
/// collection named after it. Problems with single files come back as warnings.
pub fn read_collection(dir: &Path) -> Result<(Collection, Vec<Workflow>, Vec<String>), String> {
    if !dir.is_dir() {
        return Err(format!("{} is not a folder", dir.display()));
    }
    let files = read_files(dir)?;
    let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "Collection".into());
    let (mut col, workflows, warnings) = folder::from_files(&files, &name);
    col.directory = Some(dir.display().to_string());
    Ok((col, workflows, warnings))
}

/// True when a folder can become a collection: empty, or already one.
pub fn can_link(dir: &Path) -> Result<(), String> {
    if !dir.exists() || dir.join(folder::COLLECTION_FILE).is_file() {
        return Ok(());
    }
    let mut entries = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    match entries.next() {
        None => Ok(()),
        Some(_) => Err(format!("{} isn't empty and isn't a collection folder; choose an empty folder", dir.display())),
    }
}

/// Writes a collection into its folder: changed files are rewritten, and
/// files of ours that no longer belong (renamed or deleted requests) are
/// removed, along with folders left empty. Other files are never touched.
pub fn write_collection(dir: &Path, col: &Collection, workflows: &[&Workflow]) -> Result<(), String> {
    if !dir.exists() && col.items.is_empty() && workflows.is_empty() {
        // A link whose folder is missing (an unplugged drive, say): don't
        // recreate it empty.
        return Ok(());
    }
    let files = folder::to_files(col, workflows);
    let wanted: HashSet<PathBuf> = files.iter().map(|(p, _)| dir.join(p)).collect();
    for (rel, text) in &files {
        let path = dir.join(rel);
        if std::fs::read_to_string(&path).ok().as_deref() == Some(text.as_str()) {
            continue;
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        crate::server::write_atomic(&path, text.as_bytes()).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    // Stale files of ours.
    let existing = read_files(dir)?;
    let mut dirs = HashSet::new();
    for (rel, _) in existing {
        let path = dir.join(&rel);
        if !wanted.contains(&path) {
            std::fs::remove_file(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            let mut p = path.parent();
            while let Some(d) = p.filter(|d| *d != dir) {
                dirs.insert(d.to_path_buf());
                p = d.parent();
            }
        }
    }
    // Deepest first, so parents empty out after their children.
    let mut dirs: Vec<PathBuf> = dirs.into_iter().collect();
    dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
    for d in dirs {
        if std::fs::read_dir(&d).map(|mut e| e.next().is_none()).unwrap_or(false) {
            let _ = std::fs::remove_dir(&d);
        }
    }
    Ok(())
}

/// Replaces each linked collection's stub with what its folder holds.
/// Returns a warning per problem; a folder that can't be read keeps its stub.
pub fn expand(cat: &mut Catalog) -> Vec<String> {
    let mut warnings = Vec::new();
    let links: Vec<(String, String)> = cat.collections.iter().filter_map(|c| c.directory.clone().map(|d| (c.id.clone(), d))).collect();
    for (id, dir) in links {
        match read_collection(Path::new(&dir)) {
            Ok((col, workflows, w)) => {
                warnings.extend(w.into_iter().map(|w| format!("{dir}: {w}")));
                folder::merge_linked(cat, &id, col, workflows);
            }
            Err(e) => warnings.push(format!("collection folder {e}")),
        }
    }
    warnings
}

/// Writes every linked collection to its folder and returns the catalog
/// for the main file (links only).
pub fn write_linked(cat: &Catalog) -> Result<Catalog, String> {
    for c in cat.collections.iter().filter(|c| c.directory.is_some()) {
        let dir = PathBuf::from(c.directory.as_deref().unwrap_or_default());
        write_collection(&dir, c, &folder::owned_workflows(cat, &c.id))?;
    }
    Ok(folder::stub_linked(cat))
}

/// Loads a catalog file, or a collection folder given directly, with its
/// linked collections read in. Warnings about linked folders are returned.
pub fn load(path: &Path) -> Result<(Catalog, Vec<String>), String> {
    let mut cat = if path.is_dir() {
        let (col, workflows, warnings) = read_collection(path)?;
        let cat = Catalog { collections: vec![col], workflows, ..Default::default() };
        return Ok((cat, warnings));
    } else {
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        store::decode(&bytes)?.into_catalog()
    };
    let warnings = expand(&mut cat);
    Ok((cat, warnings))
}

/// Saves a catalog loaded with [`load`]: linked collections to their
/// folders, the rest to the file (gzip unless it ends in `.json`). A
/// folder path only gets its collections written.
pub fn save(path: &Path, cat: &Catalog) -> Result<(), String> {
    let stub = write_linked(cat)?;
    if path.is_dir() {
        return Ok(());
    }
    let bytes = if path.extension().is_some_and(|e| e == "json") { store::to_json(&stub).into_bytes() } else { store::to_gzip(&stub) };
    crate::server::write_atomic(path, &bytes).map_err(|e| format!("{}: {e}", path.display()))
}

/// Changes whenever a file in a linked folder changes, so hosts can notice
/// edits made outside the app (an editor, `git pull`).
pub fn fingerprint(cat: &Catalog) -> u64 {
    fn walk(dir: &Path, depth: usize, h: &mut impl Hasher) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let name = e.file_name().to_string_lossy().into_owned();
            let Ok(meta) = e.metadata() else { continue };
            if meta.is_dir() && depth < MAX_DEPTH && !name.starts_with('.') {
                walk(&e.path(), depth + 1, h);
            } else if is_owned_file(&name) {
                name.hash(h);
                meta.len().hash(h);
                meta.modified().ok().hash(h);
            }
        }
    }
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for d in cat.collections.iter().filter_map(|c| c.directory.as_deref()) {
        d.hash(&mut h);
        walk(Path::new(d), 0, &mut h);
    }
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use apicraft_core::model::*;

    fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("apicraft-folders-{name}-{}", apicraft_core::ids::new_id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn saves_and_loads_linked_collections() {
        let base = temp_dir("link");
        let dir = base.join("api");
        let file = base.join("catalog.json.gz");
        let mut col = Collection::new("API");
        col.directory = Some(dir.display().to_string());
        let mut folder_ = Folder::new("Admin");
        folder_.items.push(Item::Request(Request::new("Delete", Protocol::Rest)));
        let get = Request::new("Get", Protocol::Rest);
        let mut wf = Workflow::new("Flow");
        wf.steps.push(WorkflowStep::new(get.id.clone()));
        col.items = vec![Item::Request(get), Item::Folder(folder_)];
        let mut cat = Catalog::default();
        cat.collections.push(col);
        cat.collections.push(Collection::new("Plain"));
        cat.workflows.push(wf);
        save(&file, &cat).unwrap();
        assert!(dir.join("Get.request.json").is_file());
        assert!(dir.join("Admin/Delete.request.json").is_file());
        assert!(dir.join("Flow.workflow.json").is_file());
        let stored = store::decode(&std::fs::read(&file).unwrap()).unwrap().into_catalog();
        assert!(stored.collections[0].items.is_empty() && stored.workflows.is_empty());

        let (back, warnings) = load(&file).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(back, cat);

        // Renaming and removing remove the old files, and the emptied folder.
        let mut cat = back;
        if let Item::Request(r) = &mut cat.collections[0].items[0] {
            r.name = "Get all".into();
        }
        cat.collections[0].items.truncate(1);
        std::fs::write(dir.join("notes.txt"), "mine").unwrap();
        save(&file, &cat).unwrap();
        assert!(!dir.join("Get.request.json").exists());
        assert!(dir.join("Get all.request.json").is_file());
        assert!(!dir.join("Admin").exists());
        assert!(dir.join("notes.txt").is_file(), "other files are left alone");

        // A folder given directly.
        let (direct, _) = load(&dir).unwrap();
        assert_eq!(direct.collections[0].items.len(), 1);
        assert_eq!(direct.workflows.len(), 1);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn missing_folders_are_kept_as_links() {
        let base = temp_dir("missing");
        let mut col = Collection::new("Gone");
        col.directory = Some(base.join("nope").display().to_string());
        let mut cat = Catalog::default();
        cat.collections.push(col);
        let warnings = expand(&mut cat);
        assert_eq!(warnings.len(), 1);
        save(&base.join("c.json.gz"), &cat).unwrap();
        assert!(!base.join("nope").exists(), "not recreated empty");
        assert!(can_link(&base).is_err());
        assert!(can_link(&base.join("new")).is_ok());
        let _ = std::fs::remove_dir_all(&base);
    }
}

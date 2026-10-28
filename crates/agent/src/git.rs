//! Runs git for a collection folder (see [`apicraft_core::git`]). Every
//! command is limited to the files under that folder.

use std::path::Path;
use std::time::Duration;

use apicraft_core::git::{self, GitReply, GitRequest, GitStatus};
use apicraft_core::sources::CommandOutput;

/// Network operations (pull, push) can take a while; the rest are quick.
const LOCAL: Duration = Duration::from_secs(30);
const REMOTE: Duration = Duration::from_secs(180);

fn git_in(dir: &str, args: &[&str], timeout: Duration) -> Result<CommandOutput, String> {
    let mut cmd = crate::process::command("git");
    cmd.arg("-C").arg(dir).args(args);
    // Never wait on a credential or editor prompt nobody can see.
    cmd.env("GIT_TERMINAL_PROMPT", "0").env("GIT_EDITOR", "true").env("GCM_INTERACTIVE", "never");
    crate::process::run_command(cmd, timeout).map_err(|e| {
        if e.contains("not found") || e.contains("cannot find") || e.contains("No such file") {
            "git isn't installed (or isn't on PATH)".to_string()
        } else {
            format!("git: {e}")
        }
    })
}

/// Runs git and returns its stdout, or its error message.
fn git_ok(dir: &str, args: &[&str], timeout: Duration) -> Result<String, String> {
    let out = git_in(dir, args, timeout)?;
    if out.ok() {
        Ok(out.stdout)
    } else {
        Err(out.failure())
    }
}

/// The repository root and the folder's path within it, or `None` when the
/// folder isn't in a repository.
fn locate(dir: &str) -> Result<Option<(String, String)>, String> {
    if !Path::new(dir).is_dir() {
        return Err(format!("{dir} doesn't exist"));
    }
    let out = git_in(dir, &["rev-parse", "--show-toplevel", "--show-prefix"], LOCAL)?;
    if !out.ok() {
        return if out.stderr.contains("not a git repository") { Ok(None) } else { Err(out.failure()) };
    }
    let mut lines = out.stdout.lines();
    let root = lines.next().unwrap_or_default().trim().to_string();
    let prefix = git::normalise(lines.next().unwrap_or_default().trim());
    let prefix = if prefix.is_empty() { prefix } else { format!("{prefix}/") };
    Ok(Some((root, prefix)))
}

/// The pathspec for the folder, from the repository root.
fn scope(prefix: &str) -> String {
    if prefix.is_empty() {
        ".".into()
    } else {
        prefix.trim_end_matches('/').to_string()
    }
}

/// Only paths inside the collection folder may be touched.
fn check_paths(prefix: &str, paths: &[String]) -> Result<(), String> {
    match paths.iter().find(|p| !p.starts_with(prefix) || p.split('/').any(|seg| seg == "..")) {
        Some(p) => Err(format!("{p} isn't in the collection's folder")),
        None if paths.is_empty() => Err("choose at least one file".into()),
        None => Ok(()),
    }
}

pub fn run(req: &GitRequest) -> Result<GitReply, String> {
    let dir = req.dir();
    if let GitRequest::Init { .. } = req {
        let msg = git_ok(dir, &["init"], LOCAL)?;
        return Ok(GitReply::Done { message: msg.trim().to_string() });
    }
    let Some((root, prefix)) = locate(dir)? else { return Ok(GitReply::NotRepo) };
    let here = scope(&prefix);
    match req {
        GitRequest::Init { .. } => unreachable!(),
        GitRequest::Status { .. } => status(&root, &prefix, &here).map(GitReply::Status),
        GitRequest::Diff { path, .. } => {
            check_paths(&prefix, std::slice::from_ref(path))?;
            let tracked = git_in(&root, &["ls-files", "--error-unmatch", "--", path], LOCAL)?.ok();
            let text = if tracked {
                // Against the last commit when there is one, so staged edits show too.
                match git_in(&root, &["diff", "HEAD", "--", path], LOCAL)? {
                    out if out.ok() => out.stdout,
                    _ => git_ok(&root, &["diff", "--cached", "--", path], LOCAL)?,
                }
            } else {
                let full = Path::new(&root).join(path);
                let body = std::fs::read_to_string(&full).unwrap_or_default();
                format!("new file\n{}", body.lines().map(|l| format!("+{l}")).collect::<Vec<_>>().join("\n"))
            };
            Ok(GitReply::Diff { path: path.clone(), text: if text.trim().is_empty() { "(no changes)".into() } else { text } })
        }
        GitRequest::Commit { message, paths, .. } => {
            check_paths(&prefix, paths)?;
            if message.trim().is_empty() {
                return Err("write a commit message first".into());
            }
            let mut add = vec!["add", "-A", "--"];
            add.extend(paths.iter().map(String::as_str));
            git_ok(&root, &add, LOCAL)?;
            let mut commit = vec!["commit", "-m", message.as_str(), "--only", "--"];
            commit.extend(paths.iter().map(String::as_str));
            let out = git_ok(&root, &commit, LOCAL)?;
            Ok(GitReply::Done { message: out.lines().next().unwrap_or("Committed").trim().to_string() })
        }
        GitRequest::Discard { paths, .. } => {
            check_paths(&prefix, paths)?;
            let st = status(&root, &prefix, &here)?;
            for p in paths {
                let Some(f) = st.files.iter().find(|f| &f.path == p) else { continue };
                if f.label() == "new" {
                    let _ = std::fs::remove_file(Path::new(&root).join(p));
                } else if f.index == 'A' {
                    git_ok(&root, &["rm", "--cached", "--quiet", "--", p], LOCAL)?;
                    let _ = std::fs::remove_file(Path::new(&root).join(p));
                } else {
                    git_ok(&root, &["checkout", "HEAD", "--", p], LOCAL)?;
                }
            }
            Ok(GitReply::Done { message: format!("Discarded changes to {} file(s)", paths.len()) })
        }
        GitRequest::Pull { .. } => {
            let out = git_ok(&root, &["pull", "--ff-only"], REMOTE)?;
            Ok(GitReply::Done { message: out.lines().last().unwrap_or("Pulled").trim().to_string() })
        }
        GitRequest::Push { .. } => {
            let out = git_in(&root, &["push"], REMOTE)?;
            if !out.ok() && out.stderr.contains("no upstream branch") {
                let branch = git_ok(&root, &["symbolic-ref", "--short", "HEAD"], LOCAL)?;
                git_ok(&root, &["push", "--set-upstream", "origin", branch.trim()], REMOTE)?;
            } else if !out.ok() {
                return Err(out.failure());
            }
            Ok(GitReply::Done { message: "Pushed".into() })
        }
    }
}

fn status(root: &str, prefix: &str, here: &str) -> Result<GitStatus, String> {
    let porcelain = git_ok(root, &["status", "--porcelain=v1", "-z", "--untracked-files=all", "--", here], LOCAL)?;
    let branch = git_ok(root, &["symbolic-ref", "--short", "HEAD"], LOCAL)
        .or_else(|_| git_ok(root, &["rev-parse", "--short", "HEAD"], LOCAL).map(|h| format!("detached at {}", h.trim())))
        .unwrap_or_default()
        .trim()
        .to_string();
    let upstream = git_ok(root, &["rev-parse", "--abbrev-ref", "@{u}"], LOCAL).ok().map(|u| u.trim().to_string()).filter(|u| !u.is_empty());
    let (ahead, behind) = match &upstream {
        Some(_) => git_ok(root, &["rev-list", "--left-right", "--count", "HEAD...@{u}"], LOCAL).map(|o| git::parse_ahead_behind(&o)).unwrap_or((0, 0)),
        None => (0, 0),
    };
    // A new repository has no commits, and so no log.
    let log = git_ok(root, &["log", "-n", "15", "--pretty=format:%h%x1f%an%x1f%ar%x1f%s", "--", here], LOCAL).map(|o| git::parse_log(&o)).unwrap_or_default();
    Ok(GitStatus { root: root.to_string(), prefix: prefix.to_string(), branch, upstream, ahead, behind, files: git::parse_porcelain(&porcelain, prefix), log })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_commit_and_discard_inside_the_folder_only() {
        if git_in(".", &["--version"], LOCAL).is_err() {
            return; // no git here
        }
        let base = std::env::temp_dir().join(format!("apicraft-git-{}", apicraft_core::ids::new_id()));
        let dir = base.join("api");
        std::fs::create_dir_all(&dir).unwrap();
        let b = base.display().to_string();
        let d = dir.display().to_string();
        assert_eq!(run(&GitRequest::Status { dir: d.clone() }).unwrap(), GitReply::NotRepo);
        run(&GitRequest::Init { dir: b.clone() }).unwrap();
        git_ok(&b, &["config", "user.email", "test@example.com"], LOCAL).unwrap();
        git_ok(&b, &["config", "user.name", "Test"], LOCAL).unwrap();
        std::fs::write(dir.join("Get.request.json"), "{}").unwrap();
        std::fs::write(base.join("outside.txt"), "x").unwrap();
        let GitReply::Status(st) = run(&GitRequest::Status { dir: d.clone() }).unwrap() else { panic!() };
        assert_eq!(st.prefix, "api/");
        assert_eq!(st.files.iter().map(|f| f.shown.as_str()).collect::<Vec<_>>(), ["Get.request.json"], "only the folder's files");
        let paths: Vec<String> = st.files.iter().map(|f| f.path.clone()).collect();
        assert!(run(&GitRequest::Commit { dir: d.clone(), message: "x".into(), paths: vec!["outside.txt".into()] }).is_err());
        run(&GitRequest::Commit { dir: d.clone(), message: "Add Get".into(), paths }).unwrap();
        std::fs::write(dir.join("Get.request.json"), "{\"changed\":1}").unwrap();
        let GitReply::Status(st) = run(&GitRequest::Status { dir: d.clone() }).unwrap() else { panic!() };
        assert_eq!(st.log[0].subject, "Add Get");
        assert_eq!(st.files[0].label(), "modified");
        let GitReply::Diff { text, .. } = run(&GitRequest::Diff { dir: d.clone(), path: st.files[0].path.clone() }).unwrap() else { panic!() };
        assert!(text.contains("+{\"changed\":1}"), "{text}");
        run(&GitRequest::Discard { dir: d.clone(), paths: vec![st.files[0].path.clone()] }).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("Get.request.json")).unwrap(), "{}");
        let _ = std::fs::remove_dir_all(&base);
    }
}

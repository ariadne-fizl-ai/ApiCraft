//! Git for collections kept as folders of files: what the UI asks for, what
//! comes back, and reading git's output. The commands run on the host (the
//! desktop app, or the local agent for the browser); everything is limited
//! to the files under the collection's folder.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
pub enum GitRequest {
    /// Branch, changed files under the folder, and recent commits.
    Status {
        dir: String,
    },
    /// The changes to one file (a path from [`GitFile::path`]).
    Diff {
        dir: String,
        path: String,
    },
    /// Stages the given files (additions, edits and deletions) and commits
    /// just those, leaving anything else that is staged alone.
    Commit {
        dir: String,
        message: String,
        paths: Vec<String>,
    },
    /// Throws away the changes to these files (restoring them from the last
    /// commit, or deleting them when they're new).
    Discard {
        dir: String,
        paths: Vec<String>,
    },
    Pull {
        dir: String,
    },
    Push {
        dir: String,
    },
    /// Starts a repository in the folder.
    Init {
        dir: String,
    },
}

impl GitRequest {
    pub fn dir(&self) -> &str {
        match self {
            GitRequest::Status { dir }
            | GitRequest::Diff { dir, .. }
            | GitRequest::Commit { dir, .. }
            | GitRequest::Discard { dir, .. }
            | GitRequest::Pull { dir }
            | GitRequest::Push { dir }
            | GitRequest::Init { dir } => dir,
        }
    }

    /// A short description for progress messages.
    pub fn verb(&self) -> &'static str {
        match self {
            GitRequest::Status { .. } => "Reading status",
            GitRequest::Diff { .. } => "Reading changes",
            GitRequest::Commit { .. } => "Committing",
            GitRequest::Discard { .. } => "Discarding changes",
            GitRequest::Pull { .. } => "Pulling",
            GitRequest::Push { .. } => "Pushing",
            GitRequest::Init { .. } => "Creating the repository",
        }
    }
}

/// A changed file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitFile {
    /// Relative to the repository root, with `/`.
    pub path: String,
    /// Relative to the collection's folder, for display.
    pub shown: String,
    /// git's two status letters: staged, then in the working tree.
    pub index: char,
    pub worktree: char,
}

impl GitFile {
    pub fn label(&self) -> &'static str {
        match (self.index, self.worktree) {
            ('?', '?') => "new",
            ('U', _) | (_, 'U') | ('A', 'A') | ('D', 'D') => "conflict",
            ('R', _) => "renamed",
            ('A', _) => "added",
            ('D', _) | (_, 'D') => "deleted",
            _ => "modified",
        }
    }

    /// The single letter shown next to the file.
    pub fn letter(&self) -> char {
        match self.label() {
            "new" => 'U',
            "conflict" => '!',
            "renamed" => 'R',
            "added" => 'A',
            "deleted" => 'D',
            _ => 'M',
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitCommit {
    pub hash: String,
    pub author: String,
    pub when: String,
    pub subject: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitStatus {
    /// The repository's top folder.
    pub root: String,
    /// The collection folder within it (`api/` or empty), with `/`.
    pub prefix: String,
    pub branch: String,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub files: Vec<GitFile>,
    /// Recent commits touching the folder, newest first.
    pub log: Vec<GitCommit>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum GitReply {
    /// git is there, but the folder isn't in a repository.
    NotRepo,
    Status(GitStatus),
    Diff {
        path: String,
        text: String,
    },
    /// A command finished; `message` is git's summary.
    Done {
        message: String,
    },
}

/// Reads `git status --porcelain=v1 -z` (run at the repository root).
/// `prefix` is the collection folder's path within the repository.
pub fn parse_porcelain(out: &str, prefix: &str) -> Vec<GitFile> {
    let mut files = Vec::new();
    let mut parts = out.split('\0');
    while let Some(entry) = parts.next() {
        if entry.len() < 4 {
            continue;
        }
        let mut chars = entry.chars();
        let index = chars.next().unwrap_or(' ');
        let worktree = chars.next().unwrap_or(' ');
        let path = entry[3..].to_string();
        // Renames and copies are followed by the original path.
        if matches!(index, 'R' | 'C') {
            parts.next();
        }
        let shown = path.strip_prefix(prefix).unwrap_or(&path).to_string();
        files.push(GitFile { path, shown, index, worktree });
    }
    files.sort_by(|a, b| a.shown.cmp(&b.shown));
    files
}

/// Reads `git log --pretty=format:%h%x1f%an%x1f%ar%x1f%s`.
pub fn parse_log(out: &str) -> Vec<GitCommit> {
    out.lines()
        .filter_map(|l| {
            let mut f = l.split('\u{1f}');
            Some(GitCommit { hash: f.next()?.into(), author: f.next()?.into(), when: f.next()?.into(), subject: f.next().unwrap_or("").into() })
        })
        .collect()
}

/// Reads `git rev-list --left-right --count HEAD...@{u}`: (ahead, behind).
pub fn parse_ahead_behind(out: &str) -> (u32, u32) {
    let mut n = out.split_whitespace().map(|x| x.parse().unwrap_or(0));
    (n.next().unwrap_or(0), n.next().unwrap_or(0))
}

/// Makes a path from git (always `/`) or the host comparable: `/` separators,
/// no trailing slash.
pub fn normalise(path: &str) -> String {
    path.replace('\\', "/").trim_end_matches('/').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_porcelain_status() {
        let out = " M api/Get.request.json\0?? api/New.request.json\0R  api/b.json\0api/a.json\0 D api/Old.request.json\0";
        let files = parse_porcelain(out, "api/");
        let got: Vec<(&str, &str)> = files.iter().map(|f| (f.shown.as_str(), f.label())).collect();
        assert_eq!(got, [("Get.request.json", "modified"), ("New.request.json", "new"), ("Old.request.json", "deleted"), ("b.json", "renamed")]);
        assert_eq!(files[0].path, "api/Get.request.json");
    }

    #[test]
    fn reads_log_and_counts() {
        let log = parse_log("abc123\u{1f}Sam\u{1f}2 days ago\u{1f}Add requests\n");
        assert_eq!(log[0].subject, "Add requests");
        assert_eq!(parse_ahead_behind("2\t5\n"), (2, 5));
        assert_eq!(normalise("C:\\x\\y\\"), "C:/x/y");
    }
}

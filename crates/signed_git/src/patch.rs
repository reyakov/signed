use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use diffy::patch_set::{FileOperation, FilePatch, ParseOptions, PatchSet};
use diffy::{Hunk, Line};

use crate::diff::{CommitDiff, DiffHunk, DiffLine, DiffLineKind, DiffStatus, FileDiff};
use crate::history::FileCommit;

/// Apply a `git format-patch` patch or series with `git am`.
///
/// Uses the git CLI because it handles the mbox format natively.
///
/// TODO: replace with a pure-Rust implementation later without changing callers.
pub fn apply_patch(repo_path: &Path, patch: &str) -> Result<()> {
    let mut child = Command::new("git")
        .arg("am")
        .current_dir(repo_path)
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("failed to spawn `git am`")?;

    child
        .stdin
        .as_mut()
        .expect("stdin piped")
        .write_all(patch.as_bytes())?;

    let output = child.wait_with_output()?;

    if !output.status.success() {
        bail!("git am failed: {}", String::from_utf8_lossy(&output.stderr));
    }

    Ok(())
}

/// The `git format-patch` mbox series of `base..tip`, like `git format-patch --stdout`.
/// Fails when the range has no commits.
///
/// The mbox is returned untrimmed. Trailing newlines are part of the format.
pub fn format_patch_between(repo_path: &Path, base: &str, tip: &str) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .args(["format-patch", "--stdout", &format!("{base}..{tip}")])
        .env("GIT_TERMINAL_PROMPT", "0")
        .stderr(Stdio::piped())
        .output()
        .context("failed to spawn `git format-patch`")?;

    if !output.status.success() {
        bail!(
            "git format-patch failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    let patch = String::from_utf8_lossy(&output.stdout).into_owned();

    if patch.trim().is_empty() {
        bail!("no commits between {base} and {tip}");
    }

    Ok(patch)
}

/// Split a `git format-patch` series into its individual patches, mbox messages.
///
/// A single patch yields one element.
/// A malformed input yields one element covering it.
pub fn split_patch_series(patch: &str) -> Vec<&str> {
    let mut starts = vec![0usize];
    let mut search_from = 1;

    while let Some(rel) = patch[search_from..].find("\nFrom ") {
        let ix = search_from + rel + 1;
        let hex = patch[ix + 5..]
            .split(|c: char| !c.is_ascii_hexdigit())
            .next()
            .unwrap_or("");
        if hex.len() == 40 {
            starts.push(ix);
        }
        search_from = ix + 1;
    }

    starts
        .iter()
        .enumerate()
        .map(|(i, &start)| {
            let end = starts.get(i + 1).copied().unwrap_or(patch.len());
            &patch[start..end]
        })
        .collect()
}

/// Parse `git format-patch` output, a single patch or a series.
///
/// Backed by [`diffy::patch_set`], which implements git's extended diff format:
/// `diff --git` headers, rename and copy detection, binary detection, and
/// C-style quoted or octal-escaped paths.
pub fn patch_diffs(patch: &str) -> Result<CommitDiff> {
    if !patch.lines().any(|line| line.starts_with("diff --git ")) {
        return Ok(CommitDiff { files: Vec::new() });
    }

    let mut files = Vec::new();

    for file in PatchSet::parse(patch, ParseOptions::gitdiff()) {
        files.push(file_diff(file?)?);
    }

    Ok(CommitDiff { files })
}

fn file_diff(file: FilePatch<'_, str>) -> Result<FileDiff> {
    // The `---`/`+++` paths carry the `a/`/`b/` prefix, so the first path
    // component is dropped, the same way `git apply -p1` does.
    // Rename and copy paths come from their own headers, unprefixed.
    let stripped;
    let operation = match file.operation() {
        operation @ (FileOperation::Rename { .. } | FileOperation::Copy { .. }) => operation,
        operation => {
            stripped = operation.strip_prefix(1);
            &stripped
        }
    };

    let (path, old_path, status) = match operation {
        FileOperation::Create(path) => (path.as_ref(), None, DiffStatus::Added),
        FileOperation::Delete(path) => (path.as_ref(), None, DiffStatus::Deleted),
        FileOperation::Modify { modified, .. } => (modified.as_ref(), None, DiffStatus::Modified),
        FileOperation::Rename { from, to } => {
            (to.as_ref(), Some(from.as_ref()), DiffStatus::Renamed)
        }
        FileOperation::Copy { from, to } => (to.as_ref(), Some(from.as_ref()), DiffStatus::Copied),
    };

    let mut insertions = 0usize;
    let mut deletions = 0usize;
    let mut hunks = Vec::new();

    let patch = file.patch();

    if let Some(text) = patch.as_text() {
        for hunk in text.hunks() {
            let hunk = hunk_diff(hunk);
            insertions += hunk
                .lines
                .iter()
                .filter(|line| line.kind == DiffLineKind::Addition)
                .count();
            deletions += hunk
                .lines
                .iter()
                .filter(|line| line.kind == DiffLineKind::Deletion)
                .count();
            hunks.push(hunk);
        }
    }

    Ok(FileDiff {
        path: path.to_owned(),
        old_path: old_path.map(str::to_owned),
        status,
        insertions,
        deletions,
        binary: patch.is_binary(),
        hunks,
    })
}

/// The [`DiffHunk`] of one parsed hunk, including the line number of every line.
///
/// `diffy` reports only the hunk header ranges. The per-line numbers are
/// counted from them the way the header encodes them: context lines advance
/// both sides, deletions only the old, insertions only the new.
fn hunk_diff(hunk: &Hunk<'_, str>) -> DiffHunk {
    let old_range = hunk.old_range();
    let new_range = hunk.new_range();

    let mut old = old_range.start() as u32;
    let mut new = new_range.start() as u32;
    let mut lines = Vec::with_capacity(hunk.lines().len());

    for line in hunk.lines() {
        let (kind, text) = match line {
            Line::Context(text) => (DiffLineKind::Context, *text),
            Line::Delete(text) => (DiffLineKind::Deletion, *text),
            Line::Insert(text) => (DiffLineKind::Addition, *text),
        };

        let (old_no, new_no) = match kind {
            DiffLineKind::Context => {
                let numbers = (Some(old), Some(new));
                old += 1;
                new += 1;
                numbers
            }
            DiffLineKind::Addition => {
                let number = Some(new);
                new += 1;
                (None, number)
            }
            DiffLineKind::Deletion => {
                let number = Some(old);
                old += 1;
                (number, None)
            }
        };

        lines.push(DiffLine {
            kind,
            old: old_no,
            new: new_no,
            text: line_text(text),
        });
    }

    DiffHunk {
        old_start: old_range.start() as u32,
        old_lines: old_range.len() as u32,
        new_start: new_range.start() as u32,
        new_lines: new_range.len() as u32,
        lines,
    }
}

/// The content of a parsed line without its line ending.
///
/// `diffy` keeps the trailing `\n`, the way `str::lines` splits it off.
fn line_text(text: &str) -> String {
    let text = text.strip_suffix('\n').unwrap_or(text);
    text.strip_suffix('\r').unwrap_or(text).to_owned()
}

/// Commits of a `git format-patch` output, a single patch or a series.
///
/// Entries appear in patch order, oldest first as `git format-patch` produces them.
pub fn patch_commits(patch: &str) -> Vec<FileCommit> {
    let lines: Vec<&str> = patch.lines().collect();

    let mut commits = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        // A patch starts with its `From <id> <date>` envelope line.
        let Some(rest) = lines[i].strip_prefix("From ") else {
            i += 1;
            continue;
        };

        let Some(id) = rest.split_whitespace().next() else {
            i += 1;
            continue;
        };

        if id.len() != 40 {
            i += 1;
            continue;
        }

        let mut author = String::new();
        let mut summary = String::new();
        let mut time = 0i64;

        // Envelope headers run up to the blank line before the commit message.
        i += 1;
        while i < lines.len() && !lines[i].is_empty() {
            let header = lines[i];
            if let Some(value) = header.strip_prefix("From: ") {
                author = name_from_address(value);
            } else if let Some(value) = header.strip_prefix("Subject: ") {
                summary = strip_patch_prefix(value);
            } else if let Some(value) = header.strip_prefix("Date: ") {
                time = gix::date::parse(value.trim(), None)
                    .map(|t| t.seconds)
                    .unwrap_or(0);
            }
            i += 1;
        }

        commits.push(FileCommit {
            id: id.to_string(),
            summary,
            description: None,
            author,
            time,
        });
    }

    commits
}

fn name_from_address(from: &str) -> String {
    match from.trim().find('<') {
        Some(ix) => from[..ix].trim().to_string(),
        None => from.trim().to_string(),
    }
}

/// Strip the patch prefix from a `Subject:` header.
///
/// Examples are `[PATCH]`, `[PATCH 1/2]` and `[RFC PATCH]`.
fn strip_patch_prefix(subject: &str) -> String {
    let trimmed = subject.trim();
    let Some(rest) = trimmed.strip_prefix('[') else {
        return trimmed.to_string();
    };
    let Some(end) = rest.find(']') else {
        return trimmed.to_string();
    };
    if rest[..end].to_ascii_lowercase().contains("patch") {
        rest[end + 1..].trim().to_string()
    } else {
        trimmed.to_string()
    }
}

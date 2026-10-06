use std::io::Write;
use std::process::Stdio;

use anyhow::{Context, Result, bail};
use diffy::patch_set::{FileOperation, FilePatch, ParseOptions, PatchSet};
use diffy::{Hunk, Line};

use crate::diff::{CommitDiff, DiffHunk, DiffLine, DiffLineKind, DiffStatus, FileDiff};
use crate::history::FileCommit;
use crate::repo::Repo;

pub struct PatchParser;

impl PatchParser {
    pub fn split_patch_series(patch: &str) -> Vec<&str> {
        Self::envelopes(patch)
            .into_iter()
            .map(|message| message.text)
            .collect()
    }

    pub fn patch_diffs(patch: &str) -> Result<CommitDiff> {
        if !patch.lines().any(|line| line.starts_with("diff --git ")) {
            return Ok(CommitDiff { files: Vec::new() });
        }

        let mut files = Vec::new();

        for file in PatchSet::parse(patch, ParseOptions::gitdiff()) {
            files.push(Self::file_diff(file?)?);
        }

        Ok(CommitDiff { files })
    }

    pub fn patch_commits(patch: &str) -> Vec<FileCommit> {
        Self::envelopes(patch)
            .into_iter()
            .filter(|message| !message.id.is_empty())
            .map(|message| FileCommit {
                id: message.id.to_string(),
                summary: message
                    .header("Subject")
                    .map(Self::strip_patch_prefix)
                    .unwrap_or_default(),
                description: None,
                author: message
                    .header("From")
                    .map(Self::name_from_address)
                    .unwrap_or_default(),
                time: message
                    .header("Date")
                    .and_then(|value| gix::date::parse(value.trim(), None).ok())
                    .map(|time| time.seconds)
                    .unwrap_or(0),
            })
            .collect()
    }

    fn envelopes(patch: &str) -> Vec<Envelope<'_>> {
        let mut messages: Vec<Envelope<'_>> = Vec::new();
        let mut current: Option<(usize, &str, Vec<&str>)> = None;
        let mut headers_closed = false;

        let mut offset = 0usize;
        for line in patch.lines() {
            let line_start = offset;
            offset += line.len() + 1;

            let is_envelope = line
                .strip_prefix("From ")
                .and_then(|rest| rest.split_whitespace().next())
                .is_some_and(|id| id.len() == 40);

            if is_envelope {
                if let Some((start, id, headers)) = current.take() {
                    messages.push(Envelope {
                        text: &patch[start..],
                        id,
                        headers,
                    });
                }

                let id = line
                    .strip_prefix("From ")
                    .and_then(|rest| rest.split_whitespace().next())
                    .unwrap_or("");

                current = Some((line_start, id, Vec::new()));
                headers_closed = false;
                continue;
            }

            if let Some((_, _, headers)) = &mut current {
                if headers_closed {
                    continue;
                }
                if line.is_empty() {
                    headers_closed = true;
                } else {
                    headers.push(line);
                }
            }
        }

        if let Some((start, id, headers)) = current.take() {
            messages.push(Envelope {
                text: &patch[start..],
                id,
                headers,
            });
        }

        if messages.is_empty() {
            messages.push(Envelope {
                text: patch,
                id: "",
                headers: Vec::new(),
            });
        }

        messages
    }

    fn name_from_address(from: &str) -> String {
        match from.trim().find('<') {
            Some(ix) => from[..ix].trim().to_string(),
            None => from.trim().to_string(),
        }
    }

    /// Matches `[PATCH]`, `[PATCH 1/2]`, `[RFC PATCH]`, etc.
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

    fn file_diff(file: FilePatch<'_, str>) -> Result<FileDiff> {
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
            FileOperation::Modify { modified, .. } => {
                (modified.as_ref(), None, DiffStatus::Modified)
            }
            FileOperation::Rename { from, to } => {
                (to.as_ref(), Some(from.as_ref()), DiffStatus::Renamed)
            }
            FileOperation::Copy { from, to } => {
                (to.as_ref(), Some(from.as_ref()), DiffStatus::Copied)
            }
        };

        let mut insertions = 0usize;
        let mut deletions = 0usize;
        let mut hunks = Vec::new();

        let patch = file.patch();

        if let Some(text) = patch.as_text() {
            for hunk in text.hunks() {
                let hunk = Self::hunk_diff(hunk);
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
                text: text
                    .strip_suffix('\n')
                    .unwrap_or(text)
                    .strip_suffix('\r')
                    .unwrap_or(text)
                    .to_owned(),
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
}

/// A `git format-patch` mbox message, split on its `From <40-hex> <date>` envelope.
struct Envelope<'a> {
    text: &'a str,
    id: &'a str,
    headers: Vec<&'a str>,
}

impl Envelope<'_> {
    fn header(&self, name: &str) -> Option<&str> {
        let prefix = format!("{name}: ");
        self.headers
            .iter()
            .find(|line| line.starts_with(&prefix))
            .map(|line| &line[prefix.len()..])
    }
}

impl Repo {
    pub fn apply_patch(&self, patch: &str) -> Result<()> {
        let workdir = self
            .inner
            .workdir()
            .context("repository has no worktree")?
            .to_path_buf();

        let mut child = Repo::git_command(&workdir)
            .args(["am"])
            .stdin(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("failed to spawn `git am`")?;

        child
            .stdin
            .as_mut()
            .context("git am has no stdin pipe")?
            .write_all(patch.as_bytes())?;

        let output = child.wait_with_output()?;

        if !output.status.success() {
            bail!("git am failed: {}", String::from_utf8_lossy(&output.stderr));
        }

        Ok(())
    }

    // Fails when the range has no commits. The mbox is returned untrimmed;
    // trailing newlines are part of the format.
    pub fn format_patch_between(&self, base: &str, tip: &str) -> Result<String> {
        let output = Repo::run_git(
            self.workdir_or_dot(),
            &["format-patch", "--stdout", &format!("{base}..{tip}")],
            "git format-patch",
        )?;

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
}

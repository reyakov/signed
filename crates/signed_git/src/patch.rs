use anyhow::Result;
use diffy::patch_set::{FileOperation, FilePatch, ParseOptions, PatchSet};
use diffy::{Hunk, Line};

use crate::diff::{CommitDiff, DiffHunk, DiffLineKind, DiffStatus, FileDiff, HunkBuilder};
use crate::history::FileCommit;

pub struct PatchParser;

impl PatchParser {
    /// Splits an mbox series into the raw text of each message.
    pub fn split_patch_series(patch: &str) -> Vec<&str> {
        Self::envelopes(patch)
            .into_iter()
            .map(|message| message.text)
            .collect()
    }

    /// Parses a git diff into file diffs, returning an empty diff without one.
    pub fn patch_diffs(patch: &str) -> Result<CommitDiff> {
        if !patch.lines().any(|line| line.starts_with("diff --git ")) {
            return Ok(CommitDiff::new(Vec::new()));
        }

        let mut files = Vec::new();

        for file in PatchSet::parse(patch, ParseOptions::gitdiff()) {
            files.push(Self::file_diff(file?));
        }

        Ok(CommitDiff::new(files))
    }

    /// Lists the commits represented by the mbox messages that carry an id.
    pub fn patch_commits(patch: &str) -> Vec<FileCommit> {
        Self::envelopes(patch)
            .into_iter()
            .filter(|message| !message.id.is_empty())
            .map(|message| {
                FileCommit::new(
                    message.id.to_string(),
                    message
                        .header("Subject")
                        .map(Self::strip_patch_prefix)
                        .unwrap_or_default(),
                    None,
                    message
                        .header("From")
                        .map(Self::name_from_address)
                        .unwrap_or_default(),
                    message
                        .header("Date")
                        .and_then(|value| gix::date::parse(value.trim(), None).ok())
                        .map(|time| time.seconds)
                        .unwrap_or(0),
                )
            })
            .collect()
    }

    /// Splits `patch` on `From <40-hex> <date>` envelopes, falling back to one message.
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
                    messages.push(Envelope::new(&patch[start..], id, headers));
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
            messages.push(Envelope::new(&patch[start..], id, headers));
        }

        if messages.is_empty() {
            messages.push(Envelope::new(patch, "", Vec::new()));
        }

        messages
    }

    /// Extracts the display name from a `From` header value.
    fn name_from_address(from: &str) -> String {
        match from.trim().find('<') {
            Some(ix) => from[..ix].trim().to_string(),
            None => from.trim().to_string(),
        }
    }

    /// Strips mailing list prefixes like `[PATCH]` or `[RFC PATCH 1/2]`.
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

    /// Converts one parsed file patch, keeping rename and copy paths unstripped.
    fn file_diff(file: FilePatch<'_, str>) -> FileDiff {
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
                let (hunk, hunk_insertions, hunk_deletions) = Self::hunk_diff(hunk);
                insertions += hunk_insertions;
                deletions += hunk_deletions;
                hunks.push(hunk);
            }
        }

        FileDiff::new(
            path.to_owned(),
            old_path.map(str::to_owned),
            status,
            insertions,
            deletions,
            patch.is_binary(),
            hunks,
        )
    }

    /// Converts a diffy hunk into a `DiffHunk` with its insertion and deletion counts.
    fn hunk_diff(hunk: &Hunk<'_, str>) -> (DiffHunk, usize, usize) {
        let old_range = hunk.old_range();
        let new_range = hunk.new_range();

        let mut builder = HunkBuilder::new(
            old_range.start() as u32,
            old_range.len() as u32,
            new_range.start() as u32,
            new_range.len() as u32,
        );

        for line in hunk.lines() {
            let (kind, text) = match line {
                Line::Context(text) => (DiffLineKind::Context, *text),
                Line::Delete(text) => (DiffLineKind::Deletion, *text),
                Line::Insert(text) => (DiffLineKind::Addition, *text),
            };

            let text = text.strip_suffix('\n').unwrap_or(text);
            let text = text.strip_suffix('\r').unwrap_or(text);
            builder.push(kind, text.to_owned());
        }

        builder.finish()
    }
}

struct Envelope<'a> {
    text: &'a str,
    id: &'a str,
    headers: Vec<&'a str>,
}

impl<'a> Envelope<'a> {
    /// Creates an envelope from its text, commit id, and header lines.
    fn new(text: &'a str, id: &'a str, headers: Vec<&'a str>) -> Self {
        Self { text, id, headers }
    }

    /// Returns the first header line matching `name: `.
    fn header(&self, name: &str) -> Option<&str> {
        let prefix = format!("{name}: ");
        self.headers
            .iter()
            .find(|line| line.starts_with(&prefix))
            .map(|line| &line[prefix.len()..])
    }
}

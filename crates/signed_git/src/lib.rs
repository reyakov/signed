//! Blocking local git operations against GRASP servers.
//!
//! All functions may block; call them inside `cx.background_spawn`.

use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use gix::diff::blob::unified_diff::{ConsumeHunk, DiffLineKind as GixLineKind, HunkHeader};
use gix::interrupt::IS_INTERRUPTED;
use gix::progress::Discard;
use signed_core::RepoAddr;

/// On-disk cache of cloned repositories, keyed by owner pubkey / repo id.
#[derive(Debug, Clone)]
pub struct GitCache {
    root: PathBuf,
}

impl GitCache {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Local path of the clone for a repository.
    pub fn repo_path(&self, addr: &RepoAddr) -> PathBuf {
        self.root
            .join(addr.public_key.to_hex())
            .join(sanitize_path_component(&addr.identifier))
    }

    /// Open an existing clone.
    pub fn open(&self, addr: &RepoAddr) -> Result<Option<gix::Repository>> {
        let path = self.repo_path(addr);
        match gix::open(&path) {
            Ok(repo) => Ok(Some(repo)),
            Err(gix::open::Error::NotARepository { .. }) => Ok(None),
            Err(gix::open::Error::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Open the local clone if it exists (fetching first), otherwise clone
    /// from the first working URL in `clone_urls` (the announcement's `clone` tag).
    pub fn ensure_clone(&self, addr: &RepoAddr, clone_urls: &[String]) -> Result<gix::Repository> {
        let path = self.repo_path(addr);

        if let Some(repo) = self.open(addr)? {
            fetch_all(&repo).ok();
            return Ok(repo);
        }

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }

        clone_repo(clone_urls, &path)?;
        self.open(addr)?
            .ok_or_else(|| anyhow::anyhow!("clone finished but the repository cannot be opened"))
    }
}

/// Clone a repository into `path` from the first working URL in
/// `clone_urls` (the announcement's `clone` tag), then fetch the
/// `refs/nostr/*` PR refs like the cache clone does. The destination must
/// not exist yet; it is created by the clone. The first URL that works
/// wins; when none do, the error of the last failing URL is returned.
///
/// Unlike [`GitCache::ensure_clone`], the clone is not kept in any cache;
/// callers open it themselves if they need a [`gix::Repository`].
pub fn clone_repo(clone_urls: &[String], path: &Path) -> Result<()> {
    if path.exists() {
        bail!("destination {} already exists", path.display());
    }

    let mut last_err: Option<anyhow::Error> = None;

    for url in clone_urls {
        match clone(url, path) {
            Ok(repo) => {
                // The initial clone uses the default refspecs; also
                // fetch the `refs/nostr/*` PR refs.
                fetch_all(&repo).ok();
                return Ok(());
            }
            Err(e) => last_err = Some(e),
        }
    }

    match last_err {
        Some(e) => Err(e).context("failed to clone from any mirror"),
        None => bail!("no clone URLs provided"),
    }
}

/// Fetch all configured refspecs from `origin`, plus the `refs/nostr/*`
/// namespace where GRASP mirrors serve pull request branches (one ref per
/// PR event id, as used by ngit).
pub fn fetch_all(repo: &gix::Repository) -> Result<()> {
    let options = gix::remote::ref_map::Options {
        extra_refspecs: vec![
            gix::refspec::parse(
                gix::bstr::BStr::new("+refs/nostr/*:refs/nostr/*"),
                gix::refspec::parse::Operation::Fetch,
            )?
            .to_owned(),
        ],
        ..Default::default()
    };
    repo.find_remote("origin")?
        .connect(gix::remote::Direction::Fetch)?
        .prepare_fetch(Discard, options)?
        .receive(Discard, &IS_INTERRUPTED)?;
    Ok(())
}

/// Apply a `git format-patch` patch (or series) with `git am`.
///
/// Uses the git CLI because it handles the mbox format natively; can be
/// replaced with a pure-Rust implementation later without changing callers.
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

fn clone(url: &str, path: &Path) -> Result<gix::Repository> {
    // GRASP servers announce `grasp://<host>/<owner>/<repo>` clone URLs;
    // the transport is git smart HTTP, so rewrite the scheme for gix.
    let url = url
        .strip_prefix("grasp://")
        .map(|rest| format!("https://{rest}"))
        .unwrap_or_else(|| url.to_owned());
    let url = gix::url::parse(url).context("invalid clone URL")?;

    let mut prepare = gix::prepare_clone(url, path)?;
    let (mut checkout, _fetch) = prepare.fetch_then_checkout(Discard, &IS_INTERRUPTED)?;
    let (repo, _checkout) = checkout.main_worktree(Discard, &IS_INTERRUPTED)?;

    Ok(repo)
}

/// Create a new repository at `path`: initialize a `main` branch, write a
/// `README.md` derived from `name`/`description`, and create the initial
/// commit. Returns the initial commit id.
///
/// Uses the git CLI (like [`apply_patch`]) because it handles the plumbing
/// (index writes, ref updates, default branch selection) natively.
pub fn init_repository(path: &Path, name: &str, description: &str) -> Result<String> {
    std::fs::create_dir_all(path)
        .with_context(|| format!("failed to create {}", path.display()))?;

    git_in(path, &["init", "-b", "main"])?;

    let readme = if description.trim().is_empty() {
        format!("# {name}\n")
    } else {
        format!("# {name}\n\n{description}\n")
    };
    std::fs::write(path.join("README.md"), readme).context("failed to write README.md")?;

    git_in(path, &["add", "README.md"])?;
    // Identity and signing are passed per-invocation so the repository is
    // commitable without a global git identity or signing setup.
    git_in(
        path,
        &[
            "-c",
            "user.name=Signed",
            "-c",
            "user.email=signed@localhost",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-m",
            "Initial commit",
        ],
    )?;

    let commit = git_in(path, &["rev-parse", "HEAD"])?;
    if commit.len() != 40 {
        bail!("unexpected initial commit id: {commit}");
    }
    Ok(commit)
}

/// Push the `main` branch of the repository at `repo_path` to a grasp
/// server. Grasp servers speak git smart HTTP; the repository lives at
/// `{base_url}/{owner}/{repo-id}.git` (the same path their `clone` URLs
/// announce, per the GRASP protocol).
pub fn push_main(repo_path: &Path, base_url: &str, owner: &str, repo_id: &str) -> Result<()> {
    let url = format!("{base_url}/{owner}/{repo_id}.git");

    let output = Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .args(["push"])
        .arg(&url)
        .args(["refs/heads/main:refs/heads/main"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .stderr(Stdio::piped())
        .output()
        .context("failed to spawn `git push`")?;

    if !output.status.success() {
        bail!(
            "git push to {base_url} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

/// Add `origin` pointing at `url` when the repository has no remote yet.
/// No-op if `origin` already exists.
pub fn ensure_origin(repo_path: &Path, url: &str) -> Result<()> {
    // `git remote get-url origin` exits non-zero when the remote is absent.
    if git_in(repo_path, &["remote", "get-url", "origin"]).is_ok() {
        return Ok(());
    }
    git_in(repo_path, &["remote", "add", "origin", url])?;
    Ok(())
}

/// Run a git command in `dir`, returning trimmed stdout. The terminal prompt
/// is disabled so a credential request fails instead of hanging.
fn git_in(dir: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stderr(Stdio::piped())
        .output()
        .context("failed to spawn `git`")?;

    if !output.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// Map an untrusted repository id (or display name) to a safe single path
/// component.
///
/// Replaces everything outside `[A-Za-z0-9._-]` with `_`, and rejects the
/// special components `.` and `..` so the id can't escape a directory it is
/// joined onto.
pub fn sanitize_path_component(id: &str) -> String {
    let sanitized: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();

    if sanitized == "." || sanitized == ".." {
        return "_".to_owned();
    }

    sanitized
}

/// In-memory object cache for history walks (see [`open_with_cache`]).
/// Without one, every walk re-decodes the same commit objects from the
/// object database.
const OBJECT_CACHE_BYTES: usize = 64 * 1024 * 1024;

/// Metadata of a commit, as shown in the repository browser's file header.
#[derive(Debug, Clone)]
pub struct FileCommit {
    /// Shortened commit id (7+ hex chars, disambiguated if needed).
    pub id: String,
    /// First line of the commit message.
    pub summary: String,
    /// Rest of the commit message after the title; `None` when there is no
    /// body (single-line commit messages).
    pub description: Option<String>,
    /// Author name.
    pub author: String,
    /// Author time, seconds since the Unix epoch.
    pub time: i64,
}

/// Relative paths of all entries in the worktree (files and directories),
/// directories first, then alphabetically within each group. The `.git`
/// directory is skipped.
pub fn worktree_entries(repo: &gix::Repository) -> Result<Vec<PathBuf>> {
    let workdir = repo.workdir().context("repository has no worktree")?;

    let mut entries: Vec<(PathBuf, bool)> = Vec::new();
    collect_entries(workdir, workdir, &mut entries)?;

    entries.sort_by(|(a, a_is_dir), (b, b_is_dir)| {
        b_is_dir
            .cmp(a_is_dir)
            .then_with(|| a.as_os_str().cmp(b.as_os_str()))
    });
    Ok(entries.into_iter().map(|(path, _)| path).collect())
}

/// Read a file from the worktree. Returns `Ok(None)` if the path is missing
/// or not a regular file.
pub fn worktree_read(repo: &gix::Repository, rel: &Path) -> Result<Option<Vec<u8>>> {
    let workdir = repo.workdir().context("repository has no worktree")?;
    let path = workdir.join(rel);

    match std::fs::read(&path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) if e.kind() == std::io::ErrorKind::IsADirectory => Ok(None),
        Err(e) => Err(e).with_context(|| format!("failed to read {}", path.display())),
    }
}

/// Find the README file in the repository root (returned as a path relative
/// to the worktree). Case-insensitive; prefers `README.md`, then `.markdown`,
/// `.mdown`, `.mkdn`, then any other file whose name starts with `readme`.
pub fn find_readme(repo: &gix::Repository) -> Result<Option<PathBuf>> {
    let Some(workdir) = repo.workdir() else {
        return Ok(None);
    };

    let mut candidates: Vec<PathBuf> = Vec::new();
    for entry in std::fs::read_dir(workdir)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if name.to_ascii_lowercase().starts_with("readme") {
            candidates.push(entry.path());
        }
    }

    candidates.sort_by_key(|path| {
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase());
        match ext.as_deref() {
            Some("md") => 0,
            Some("markdown") => 1,
            Some("mdown") => 2,
            Some("mkdn") => 3,
            Some(_) => 5,
            None => 4,
        }
    });

    Ok(candidates
        .into_iter()
        .next()
        .and_then(|path| path.strip_prefix(workdir).ok().map(Path::to_path_buf)))
}

/// Open the repository at `workdir` with an in-memory object cache sized for
/// history walks.
fn open_with_cache(workdir: &Path) -> Result<gix::Repository> {
    let mut repo = gix::open(workdir)?;
    repo.object_cache_size_if_unset(OBJECT_CACHE_BYTES);
    Ok(repo)
}

/// A [`FileCommit`] from a walk commit: author, message title and shortened
/// id. `include_description` controls whether the message body is copied;
/// history lists never display it, so skipping it saves a string allocation
/// per listed commit (the diff panel fetches the full commit on demand).
fn file_commit(commit: &gix::Commit<'_>, include_description: bool) -> Result<FileCommit> {
    let author = commit.author()?;
    let message = commit.message()?;
    Ok(FileCommit {
        id: commit.id().shorten_or_id().to_string(),
        summary: String::from_utf8_lossy(message.title).trim().to_string(),
        description: if include_description {
            message
                .body
                .map(|body| String::from_utf8_lossy(body).trim().to_string())
                .filter(|body| !body.is_empty())
        } else {
            None
        },
        author: String::from_utf8_lossy(author.name).trim().to_string(),
        time: author.time()?.seconds,
    })
}

/// Find the most recent commit that changed `rel` (a path relative to the
/// worktree), like `git log -1 -- <rel>` does for non-merge commits.
///
/// Walks history from `HEAD` newest-first and returns the first commit whose
/// tree entry for `rel` differs from its first parent's; a merge that only
/// changed the file through its second parent is therefore not reported.
/// Returns `Ok(None)` if no commit touched the file (e.g. untracked files).
pub fn last_commit(repo: &gix::Repository, rel: &Path) -> Result<Option<FileCommit>> {
    let rel = rel.to_path_buf();
    Ok(last_commits(repo, std::slice::from_ref(&rel))?
        .into_iter()
        .next()
        .map(|(_, commit)| commit))
}

/// Newest commit touching each of `rels` (relative to the worktree), like
/// `git log -1 -- <rel>` per path, found in a single history walk: every
/// commit is decoded once and shared across all paths. Paths without any
/// commit (e.g. untracked files) are absent from the result.
pub fn worktree_last_commits(
    workdir: &Path,
    rels: &[PathBuf],
) -> Result<Vec<(PathBuf, FileCommit)>> {
    last_commits(&open_with_cache(workdir)?, rels)
}

/// The walk behind [`last_commit`] and [`worktree_last_commits`], stopping as
/// soon as every pending path has its commit.
fn last_commits(repo: &gix::Repository, rels: &[PathBuf]) -> Result<Vec<(PathBuf, FileCommit)>> {
    use gix::traverse::commit::simple::CommitTimeOrder;

    let Some(head) = repo.head_id().ok() else {
        return Ok(Vec::new());
    };

    // De-duplicate while preserving order.
    let mut pending: Vec<PathBuf> = Vec::with_capacity(rels.len());
    let mut seen: HashSet<&Path> = HashSet::with_capacity(rels.len());
    for rel in rels {
        if seen.insert(rel.as_path()) {
            pending.push(rel.clone());
        }
    }

    let walk = repo
        .rev_walk([head])
        .sorting(gix::revision::walk::Sorting::ByCommitTime(
            CommitTimeOrder::NewestFirst,
        ));

    let mut found = Vec::new();
    for info in walk.all()? {
        if pending.is_empty() {
            break;
        }
        let info = info?;
        let commit = info.object()?;
        let tree = commit.tree()?;
        let parent_tree = match info.parent_ids().next() {
            Some(parent) => Some(parent.object()?.into_commit().tree()?),
            None => None,
        };

        // Compare each still-unresolved path against this commit and its
        // first parent; resolved paths leave the pending set.
        let mut ix = 0;
        while ix < pending.len() {
            let rel = &pending[ix];
            let blob = tree.lookup_entry_by_path(rel)?;
            let parent_blob = match &parent_tree {
                Some(tree) => tree.lookup_entry_by_path(rel)?,
                None => None,
            };

            if blob.map(|entry| entry.id().detach()) != parent_blob.map(|entry| entry.id().detach())
            {
                found.push((rel.clone(), file_commit(&commit, true)?));
                pending.swap_remove(ix);
            } else {
                ix += 1;
            }
        }
    }

    Ok(found)
}

/// Cap on [`CommitList::commits`]: the virtual list renders a window at a
/// time and the tab badge shows the real count, so a huge history is never
/// fully materialized in memory.
pub const MAX_LISTED_COMMITS: usize = 20_000;

/// Commits reachable from `HEAD`, newest first, possibly capped: `commits`
/// holds at most [`MAX_LISTED_COMMITS`] entries and `total` is the real
/// count (for the tab badge).
pub struct CommitList {
    /// Number of commits reachable from HEAD.
    pub total: usize,
    /// Newest commits, capped at [`MAX_LISTED_COMMITS`].
    pub commits: Vec<FileCommit>,
}

/// All commits reachable from `HEAD`, newest first, with author and summary.
/// Returns an empty list for a repository without any commits yet.
pub fn all_commits(repo: &gix::Repository) -> Result<CommitList> {
    use gix::traverse::commit::simple::CommitTimeOrder;

    let Some(head) = repo.head_id().ok() else {
        return Ok(CommitList {
            total: 0,
            commits: Vec::new(),
        });
    };
    let walk = repo
        .rev_walk([head])
        .sorting(gix::revision::walk::Sorting::ByCommitTime(
            CommitTimeOrder::NewestFirst,
        ));

    let mut commits = Vec::new();
    let mut total = 0;
    for info in walk.all()? {
        let info = info?;
        total += 1;
        if commits.len() < MAX_LISTED_COMMITS {
            commits.push(file_commit(&info.object()?, false)?);
        }
    }
    Ok(CommitList { total, commits })
}

/// Like [`all_commits`], but opens the repository located at `workdir`
/// (for non-bare clones the clone root is the worktree) first.
pub fn worktree_all_commits(workdir: &Path) -> Result<CommitList> {
    all_commits(&open_with_cache(workdir)?)
}

/// The kind of a [`DiffLine`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffLineKind {
    /// An unchanged context line, present on both sides.
    Context,
    /// A line added by the commit.
    Addition,
    /// A line removed by the commit.
    Deletion,
}

/// One line of a file diff.
#[derive(Debug, Clone)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    /// 1-based line number in the old version, if the line exists there.
    pub old: Option<u32>,
    /// 1-based line number in the new version, if the line exists there.
    pub new: Option<u32>,
    /// Line content without the trailing newline.
    pub text: String,
}

/// A hunk of a file diff, like `@@ -a,b +c,d @@`, with the lines between the
/// two headers (context around the change, then removals and additions).
#[derive(Debug, Clone)]
pub struct DiffHunk {
    /// 1-based start line in the old version.
    pub old_start: u32,
    /// Number of old lines covered by the hunk.
    pub old_lines: u32,
    /// 1-based start line in the new version.
    pub new_start: u32,
    /// Number of new lines covered by the hunk.
    pub new_lines: u32,
    pub lines: Vec<DiffLine>,
}

/// How a file changed in a commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
}

/// The diff of one file in a commit.
#[derive(Debug, Clone)]
pub struct FileDiff {
    /// Path of the file relative to the repo root (the destination path for
    /// renames and copies).
    pub path: String,
    /// Previous path, for renames and copies.
    pub old_path: Option<String>,
    pub status: DiffStatus,
    /// Number of added lines; 0 for binary files.
    pub insertions: usize,
    /// Number of removed lines; 0 for binary files.
    pub deletions: usize,
    /// True if either version is binary (then `hunks` is empty).
    pub binary: bool,
    pub hunks: Vec<DiffHunk>,
}

/// The changes of one commit: every file it added, modified, deleted or
/// renamed, with line-level hunks for text files.
#[derive(Debug, Clone)]
pub struct CommitDiff {
    pub files: Vec<FileDiff>,
}

/// The changes of the commit `id` (short or full) in the repository at
/// `workdir`, compared against its first parent (the empty tree for the root
/// commit), like `git show`. Directory entries and submodules are skipped;
/// their contents are reported as individual file changes. Files are sorted
/// by path.
pub fn worktree_commit_diff(workdir: &Path, id: &str) -> Result<CommitDiff> {
    commit_diff(&open_with_cache(workdir)?, id)
}

fn commit_diff(repo: &gix::Repository, id: &str) -> Result<CommitDiff> {
    let commit_id = repo.rev_parse_single(id.as_bytes())?;
    let commit = commit_id.object()?.into_commit();
    let new_tree = commit.tree()?;
    let old_tree = match commit.parent_ids().next() {
        Some(parent) => Some(parent.object()?.into_commit().tree()?),
        None => None,
    };
    tree_diff(repo, old_tree.as_ref(), &new_tree)
}

/// The changes between two commits (`base`..`tip`), like `git diff base tip`.
/// Same file handling as [`worktree_commit_diff`] (directories and
/// submodules are skipped, files are sorted by path).
pub fn worktree_commit_range_diff(workdir: &Path, base: &str, tip: &str) -> Result<CommitDiff> {
    let repo = open_with_cache(workdir)?;
    let base_tree = repo
        .rev_parse_single(base.as_bytes())?
        .object()?
        .into_commit()
        .tree()?;
    let tip_tree = repo
        .rev_parse_single(tip.as_bytes())?
        .object()?
        .into_commit()
        .tree()?;
    tree_diff(&repo, Some(&base_tree), &tip_tree)
}

/// Commits in the range `base`..`tip`, newest first, like `git log base..tip`.
pub fn worktree_commit_range_commits(
    workdir: &Path,
    base: &str,
    tip: &str,
) -> Result<Vec<FileCommit>> {
    use gix::traverse::commit::simple::CommitTimeOrder;

    let repo = open_with_cache(workdir)?;
    let base_id = repo.rev_parse_single(base.as_bytes())?;
    let tip_id = repo.rev_parse_single(tip.as_bytes())?;
    let walk = repo
        .rev_walk([tip_id])
        .sorting(gix::revision::walk::Sorting::ByCommitTime(
            CommitTimeOrder::NewestFirst,
        ))
        .with_hidden([base_id]);

    let mut commits = Vec::new();
    for info in walk.all()? {
        let info = info?;
        commits.push(file_commit(&info.object()?, false)?);
    }
    Ok(commits)
}

/// The changes between two trees, used by both [`commit_diff`] and
/// [`worktree_commit_range_diff`].
fn tree_diff(
    repo: &gix::Repository,
    old_tree: Option<&gix::Tree<'_>>,
    new_tree: &gix::Tree<'_>,
) -> Result<CommitDiff> {
    use gix::diff::blob::platform::prepare_diff::Operation;
    use gix::object::tree::diff::Change;
    use gix::objs::tree::EntryKind;

    let changes = repo.diff_tree_to_tree(old_tree, Some(new_tree), None)?;
    let mut cache = repo.diff_resource_cache_for_tree_diff()?;

    let mut files = Vec::new();
    for change in changes {
        let attached = Change::from_change_ref(change.to_ref(), repo, repo);

        // The tree diff also reports directory entries; only their contents
        // are listed, so skip trees and submodule gitlinks.
        let (path, old_path, status) = match attached {
            Change::Addition {
                location,
                entry_mode,
                ..
            } if !matches!(entry_mode.kind(), EntryKind::Tree | EntryKind::Commit) => {
                (location.to_owned(), None, DiffStatus::Added)
            }
            Change::Deletion {
                location,
                entry_mode,
                ..
            } if !matches!(entry_mode.kind(), EntryKind::Tree | EntryKind::Commit) => {
                (location.to_owned(), None, DiffStatus::Deleted)
            }
            Change::Modification {
                location,
                previous_entry_mode,
                entry_mode,
                ..
            } if !matches!(entry_mode.kind(), EntryKind::Tree | EntryKind::Commit)
                && !matches!(
                    previous_entry_mode.kind(),
                    EntryKind::Tree | EntryKind::Commit
                ) =>
            {
                (location.to_owned(), None, DiffStatus::Modified)
            }
            Change::Rewrite {
                location,
                source_location,
                source_entry_mode,
                entry_mode,
                copy,
                ..
            } if !matches!(entry_mode.kind(), EntryKind::Tree | EntryKind::Commit)
                && !matches!(
                    source_entry_mode.kind(),
                    EntryKind::Tree | EntryKind::Commit
                ) =>
            {
                let status = if copy {
                    DiffStatus::Copied
                } else {
                    DiffStatus::Renamed
                };
                (
                    location.to_owned(),
                    Some(source_location.to_owned()),
                    status,
                )
            }
            _ => continue,
        };

        // Always diff with the built-in algorithm: external diff drivers
        // would shell out, which is out of scope for a read-only viewer.
        let platform = attached.diff(&mut cache)?;
        platform
            .resource_cache
            .options
            .skip_internal_diff_if_external_is_configured = true;
        let outcome = platform.resource_cache.prepare_diff()?;

        let (binary, hunks, insertions, deletions) = match outcome.operation {
            Operation::InternalDiff { algorithm } => {
                let input = outcome.interned_input();
                let diff = gix::diff::blob::diff_with_slider_heuristics(algorithm, &input);

                let mut hunks = Vec::new();
                let mut insertions = 0usize;
                let mut deletions = 0usize;
                let collector = HunkCollector {
                    hunks: &mut hunks,
                    insertions: &mut insertions,
                    deletions: &mut deletions,
                };
                gix::diff::blob::UnifiedDiff::new(&diff, &input, collector, Default::default())
                    .consume()?;
                (false, hunks, insertions, deletions)
            }
            Operation::SourceOrDestinationIsBinary => (true, Vec::new(), 0, 0),
            Operation::ExternalCommand { .. } => unreachable!("external diff drivers are disabled"),
        };

        files.push(FileDiff {
            path: String::from_utf8_lossy(&path).into_owned(),
            old_path: old_path.map(|p| String::from_utf8_lossy(&p).into_owned()),
            status,
            insertions,
            deletions,
            binary,
            hunks,
        });
    }

    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(CommitDiff { files })
}

/// Parse a `git format-patch` output (a single patch or a patch series)
/// into the same [`CommitDiff`] structure used for commit diffs.
///
/// The mbox envelope (From/Subject/... headers, commit body and diffstat)
/// is skipped; every `diff --git` section becomes one [`FileDiff`]. Paths
/// are taken from the section headers, with git's C-style quoting undone.
/// Sections without hunks (pure renames, mode changes, binary files) are
/// reported without lines.
pub fn patch_diffs(patch: &str) -> Result<CommitDiff> {
    let lines: Vec<&str> = patch.lines().collect();
    let mut files = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        let Some(header) = lines[i].strip_prefix("diff --git ") else {
            i += 1;
            continue;
        };
        let (file, next) = parse_diff_section(header, &lines, i + 1)?;
        files.push(file);
        i = next;
    }

    Ok(CommitDiff { files })
}

/// Commits of a `git format-patch` output (a single patch or a patch
/// series), parsed from the mbox envelope headers of each patch: commit id,
/// author, summary and author time. Entries appear in patch order (oldest
/// first, as produced by `git format-patch`).
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

        // Envelope headers of this patch, up to the blank line separating
        // them from the commit message.
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

/// The name part of a `From: Name <email>` header value.
fn name_from_address(from: &str) -> String {
    match from.trim().find('<') {
        Some(ix) => from[..ix].trim().to_string(),
        None => from.trim().to_string(),
    }
}

/// Strip the `[PATCH]`, `[PATCH 1/2]`, `[RFC PATCH]` ... prefix from a patch
/// `Subject:` header.
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

/// Parse one file's diff section: everything after its `diff --git` header
/// up to the next section (or the end of the patch). Returns the section
/// and the index of the first unconsumed line.
fn parse_diff_section(header: &str, lines: &[&str], start: usize) -> Result<(FileDiff, usize)> {
    let (header_old, header_new) = header_paths(header)?;
    // The `---`/`+++` lines name the two sides unambiguously (the header
    // can't distinguish spaces); fall back to the header for sections
    // without them (pure renames, mode changes).
    let mut old_path = header_old;
    let mut new_path = header_new;

    let mut status = DiffStatus::Modified;
    let mut binary = false;
    let mut hunks = Vec::new();
    let mut insertions = 0usize;
    let mut deletions = 0usize;
    let mut i = start;

    while i < lines.len() {
        let line = lines[i];

        // The next file's section starts at this line.
        if line.starts_with("diff --git ") {
            break;
        }
        i += 1;

        if line.starts_with("@@ -") {
            let (hunk, next) = parse_hunk(lines, i - 1)?;
            i = next;
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
        } else if let Some(rest) = line.strip_prefix("--- ") {
            if rest == "/dev/null" {
                status = DiffStatus::Added;
            } else {
                old_path = diff_line_path(rest, "a/")?;
            }
        } else if let Some(rest) = line.strip_prefix("+++ ") {
            if rest == "/dev/null" {
                status = DiffStatus::Deleted;
            } else {
                new_path = diff_line_path(rest, "b/")?;
            }
        } else if line.starts_with("new file mode ") {
            status = DiffStatus::Added;
        } else if line.starts_with("deleted file mode ") {
            status = DiffStatus::Deleted;
        } else if line.starts_with("copy from ") {
            status = DiffStatus::Copied;
        } else if line.starts_with("rename from ") {
            status = DiffStatus::Renamed;
        } else if line.starts_with("Binary files ") || line.starts_with("GIT binary patch") {
            binary = true;
            // A literal binary patch may follow; skip it without consuming
            // the next section's header.
            while i < lines.len() && !lines[i].starts_with("diff --git ") {
                i += 1;
            }
            break;
        }
        // Everything else (index/mode/similarity lines) is ignored.
    }

    Ok((
        FileDiff {
            path: new_path,
            old_path: matches!(status, DiffStatus::Renamed | DiffStatus::Copied)
                .then_some(old_path),
            status,
            insertions,
            deletions,
            binary,
            hunks,
        },
        i,
    ))
}

/// Parse one hunk: the `@@ -a,b +c,d @@` header plus every body line up to
/// the next hunk header, the next `diff --git` section or the end of the
/// patch. Returns the hunk and the index of the first unconsumed line.
fn parse_hunk(lines: &[&str], start: usize) -> Result<(DiffHunk, usize)> {
    let (old_start, old_lines, new_start, new_lines) = hunk_header(lines[start])?;

    let mut diff_lines = Vec::new();
    let mut old = old_start;
    let mut new = new_start;
    let mut i = start + 1;

    while i < lines.len() {
        let line = lines[i];
        let Some(kind) = line_prefix_kind(line) else {
            break;
        };
        i += 1;

        // Context lines advance both counters, deletions only the old one
        // and additions only the new one, so every line ends up with its
        // real line number in both versions.
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
        diff_lines.push(DiffLine {
            kind,
            old: old_no,
            new: new_no,
            text: line[1..].to_owned(),
        });
    }

    Ok((
        DiffHunk {
            old_start,
            old_lines,
            new_start,
            new_lines,
            lines: diff_lines,
        },
        i,
    ))
}

/// The kind of a hunk body line, from its first character; lines that don't
/// belong to the hunk (headers, `\ No newline...`, the next section) yield
/// `None`.
fn line_prefix_kind(line: &str) -> Option<DiffLineKind> {
    match line.as_bytes().first()? {
        b' ' => Some(DiffLineKind::Context),
        b'+' => Some(DiffLineKind::Addition),
        b'-' => Some(DiffLineKind::Deletion),
        _ => None,
    }
}

/// Parse a unified-diff hunk header `@@ -a,b +c,d @@`; omitted line counts
/// default to 1.
fn hunk_header(header: &str) -> Result<(u32, u32, u32, u32)> {
    let rest = header
        .strip_prefix("@@ ")
        .context("malformed hunk header")?;
    let (old_spec, rest) = rest.split_once(' ').context("malformed hunk header")?;
    let new_spec = rest.split_once(' ').map(|(new, _)| new).unwrap_or(rest);

    fn parse(spec: &str) -> Result<(u32, u32)> {
        let spec = spec.strip_prefix(['-', '+']).unwrap_or(spec);
        let (start, count) = match spec.split_once(',') {
            Some((start, count)) => (start, count.parse::<u32>()?),
            None => (spec, 1),
        };
        Ok((start.parse::<u32>()?, count))
    }

    let (old_start, old_lines) = parse(old_spec)?;
    let (new_start, new_lines) = parse(new_spec)?;
    Ok((old_start, old_lines, new_start, new_lines))
}

/// The old and new paths of a `diff --git a/X b/Y` header, with git's
/// C-style quoting undone.
///
/// Git only quotes paths containing characters that need escaping (non-ASCII
/// bytes, `"`, `\`); plain spaces are left unquoted, so the two sides of an
/// unquoted header are split at the last ` b/`.
fn header_paths(header: &str) -> Result<(String, String)> {
    if header.starts_with('"') {
        // Quoted paths include the `a/` / `b/` prefix inside the quotes.
        let (old, rest) = take_quoted(header).context("unterminated quoted path")?;
        let rest = rest.trim_start();
        let new = if rest.starts_with('"') {
            take_quoted(rest).context("unterminated quoted path")?.0
        } else {
            rest.split_whitespace().next().unwrap_or(rest)
        };
        let old = old
            .strip_prefix("a/")
            .context("old path without `a/` prefix")?;
        let new = new
            .strip_prefix("b/")
            .context("new path without `b/` prefix")?;
        Ok((unquote_path(old)?, unquote_path(new)?))
    } else {
        let (old, rest) = header
            .rsplit_once(" b/")
            .context("malformed diff --git header")?;
        let old = old
            .strip_prefix("a/")
            .context("old path without `a/` prefix")?;
        Ok((old.to_owned(), rest.to_owned()))
    }
}

/// The path of a `--- a/X` / `+++ b/Y` line: the prefix stripped, git's
/// trailing padding tab (for paths containing spaces) removed and C-style
/// quoting undone. These lines name the two sides unambiguously, unlike the
/// `diff --git` header.
fn diff_line_path(line: &str, prefix: &str) -> Result<String> {
    let line = line.trim_end_matches('\t');
    if line.starts_with('"') {
        let (path, _) = take_quoted(line).context("unterminated quoted path")?;
        let path = path
            .strip_prefix(prefix)
            .context("diff line path without `a/` or `b/` prefix")?;
        unquote_path(path)
    } else {
        Ok(line
            .strip_prefix(prefix)
            .context("diff line path without `a/` or `b/` prefix")?
            .to_owned())
    }
}

/// The content of a git C-style quoted path (opening `"`, escaped content,
/// closing `"`) and the rest of the input; `None` if unterminated.
///
/// Iterates by character so the returned slices always land on UTF-8
/// boundaries, even for non-ASCII paths.
fn take_quoted(input: &str) -> Option<(&str, &str)> {
    let mut end = 1; // byte after the opening quote
    let mut rest = &input[1..];
    while let Some(ch) = rest.chars().next() {
        let len = ch.len_utf8();
        match ch {
            '\\' => {
                // Consume the escaped character too (it may be multi-byte).
                let escaped = rest[len..].chars().next()?;
                let consumed = len + escaped.len_utf8();
                end += consumed;
                rest = &rest[consumed..];
            }
            '"' => return Some((&input[1..end], &input[end + len..])),
            _ => {
                end += len;
                rest = &rest[len..];
            }
        }
    }
    None
}

/// Undo git's C-style path quoting (`\NNN` octal escapes, `\"`, `\\`).
fn unquote_path(path: &str) -> Result<String> {
    if !path.contains('\\') {
        return Ok(path.to_owned());
    }

    let mut out = Vec::with_capacity(path.len());
    let mut bytes = path.as_bytes();
    while let Some((&b, rest)) = bytes.split_first() {
        bytes = rest;
        if b == b'\\' {
            match bytes.split_first() {
                Some((&b'"', rest)) | Some((&b'\\', rest)) => {
                    out.push(b);
                    bytes = rest;
                }
                Some((&b'n', rest)) => {
                    out.push(b'\n');
                    bytes = rest;
                }
                Some((&b't', rest)) => {
                    out.push(b'\t');
                    bytes = rest;
                }
                Some((&d1, rest)) if (b'0'..=b'7').contains(&d1) => {
                    let Some((&d2, rest)) = rest.split_first() else {
                        bail!("malformed octal escape in quoted path");
                    };
                    let Some((&d3, rest)) = rest.split_first() else {
                        bail!("malformed octal escape in quoted path");
                    };
                    if !(b'0'..=b'7').contains(&d2) || !(b'0'..=b'7').contains(&d3) {
                        bail!("malformed octal escape in quoted path");
                    }
                    let code =
                        (d1 - b'0') as u16 * 64 + (d2 - b'0') as u16 * 8 + (d3 - b'0') as u16;
                    if code > u8::MAX as u16 {
                        bail!("octal escape out of range in quoted path");
                    }
                    out.push(code as u8);
                    bytes = rest;
                }
                _ => bail!("malformed escape in quoted path"),
            }
        } else {
            out.push(b);
        }
    }

    String::from_utf8(out).context("invalid UTF-8 in quoted path")
}

/// Collects the hunks of one blob diff while tracking per-line numbers.
///
/// The unified-diff headers give the 1-based start line of the hunk in each
/// file; context lines advance both counters, removals only the old one and
/// additions only the new one, so each line ends up with its real line
/// numbers in both versions.
struct HunkCollector<'a> {
    hunks: &'a mut Vec<DiffHunk>,
    insertions: &'a mut usize,
    deletions: &'a mut usize,
}

impl ConsumeHunk for HunkCollector<'_> {
    type Out = ();

    fn consume_hunk(
        &mut self,
        header: HunkHeader,
        lines: &[(GixLineKind, &[u8])],
    ) -> std::io::Result<()> {
        let mut old_ln = header.before_hunk_start;
        let mut new_ln = header.after_hunk_start;
        let mut out = Vec::with_capacity(lines.len());

        for (kind, content) in lines {
            let text = String::from_utf8_lossy(content).into_owned();
            let line = match kind {
                GixLineKind::Context => {
                    let line = DiffLine {
                        kind: DiffLineKind::Context,
                        old: Some(old_ln),
                        new: Some(new_ln),
                        text,
                    };
                    old_ln += 1;
                    new_ln += 1;
                    line
                }
                GixLineKind::Remove => {
                    *self.deletions += 1;
                    let line = DiffLine {
                        kind: DiffLineKind::Deletion,
                        old: Some(old_ln),
                        new: None,
                        text,
                    };
                    old_ln += 1;
                    line
                }
                GixLineKind::Add => {
                    *self.insertions += 1;
                    let line = DiffLine {
                        kind: DiffLineKind::Addition,
                        old: None,
                        new: Some(new_ln),
                        text,
                    };
                    new_ln += 1;
                    line
                }
            };
            out.push(line);
        }

        self.hunks.push(DiffHunk {
            old_start: header.before_hunk_start,
            old_lines: header.before_hunk_len,
            new_start: header.after_hunk_start,
            new_lines: header.after_hunk_len,
            lines: out,
        });
        Ok(())
    }

    fn finish(self) {}
}

/// The commit HEAD points to, like `git log -1`. Returns `Ok(None)` for a
/// repository without commits yet (unborn HEAD).
pub fn head_commit(repo: &gix::Repository) -> Result<Option<FileCommit>> {
    let Some(head) = repo.head_id().ok() else {
        return Ok(None);
    };
    let commit = head.object()?.into_commit();
    Ok(Some(file_commit(&commit, true)?))
}

/// Full metadata of the commit `id` (short or full) in the repository at
/// `workdir`, like [`head_commit`] for an arbitrary commit. Returns
/// `Ok(None)` when the id cannot be resolved.
///
/// The commit list ([`all_commits`]) omits message bodies to keep the walk
/// cheap; the diff panel uses this to fetch the full commit on demand.
pub fn worktree_commit(workdir: &Path, id: &str) -> Result<Option<FileCommit>> {
    let repo = open_with_cache(workdir)?;
    match repo.rev_parse_single(id.as_bytes()) {
        Ok(commit_id) => {
            let commit = commit_id.object()?.into_commit();
            Ok(Some(file_commit(&commit, true)?))
        }
        Err(_) => Ok(None),
    }
}

/// Short names of local branches (`refs/heads/*`) of `repo`, sorted
/// alphabetically.
pub fn repo_branches(repo: &gix::Repository) -> Result<Vec<String>> {
    let mut names = Vec::new();
    for reference in repo.references()?.local_branches()? {
        let reference = reference.map_err(|error| anyhow::anyhow!("{error}"))?;
        names.push(String::from_utf8_lossy(reference.name().shorten()).into_owned());
    }
    names.sort();
    Ok(names)
}

/// Short names of tags (`refs/tags/*`) of `repo`, sorted alphabetically.
pub fn repo_tags(repo: &gix::Repository) -> Result<Vec<String>> {
    let mut names = Vec::new();
    for reference in repo.references()?.tags()? {
        let reference = reference.map_err(|error| anyhow::anyhow!("{error}"))?;
        names.push(String::from_utf8_lossy(reference.name().shorten()).into_owned());
    }
    names.sort();
    Ok(names)
}

/// Short names of local branches (`refs/heads/*`), sorted alphabetically.
pub fn worktree_branches(workdir: &Path) -> Result<Vec<String>> {
    repo_branches(&open_with_cache(workdir)?)
}

/// Short names of tags (`refs/tags/*`), sorted alphabetically.
pub fn worktree_tags(workdir: &Path) -> Result<Vec<String>> {
    repo_tags(&open_with_cache(workdir)?)
}

/// Short name of the branch HEAD points to, or `None` when detached (e.g.
/// after checking out a tag or a commit directly).
pub fn current_branch(repo: &gix::Repository) -> Result<Option<String>> {
    let head = repo.head()?;
    let Some(name) = head.referent_name() else {
        return Ok(None);
    };
    Ok(Some(String::from_utf8_lossy(name.shorten()).into_owned()))
}

/// Branch, tag and HEAD refs of a repository, ready for a NIP-34 kind-30618
/// repository state announcement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoRefState {
    /// `(full refname, commit id)` pairs for heads and tags, sorted.
    pub refs: Vec<(String, String)>,
    /// Short branch name HEAD points to, or `None` when detached.
    pub head: Option<String>,
}

/// Collect the refs of `repo`: local branches and tags as
/// `(refname, commit-id)` pairs, plus the branch HEAD points to.
pub fn repo_ref_state(repo: &gix::Repository) -> Result<RepoRefState> {
    let mut refs = Vec::new();

    for reference in repo.references()?.local_branches()? {
        let reference = reference.map_err(|error| anyhow::anyhow!("{error}"))?;
        refs.push((
            String::from_utf8_lossy(reference.name().as_bstr()).into_owned(),
            reference.id().to_string(),
        ));
    }
    for reference in repo.references()?.tags()? {
        let reference = reference.map_err(|error| anyhow::anyhow!("{error}"))?;
        refs.push((
            String::from_utf8_lossy(reference.name().as_bstr()).into_owned(),
            reference.id().to_string(),
        ));
    }
    refs.sort();

    let head = match repo.head() {
        Ok(head) => head
            .referent_name()
            .filter(|name| name.as_bstr().starts_with(b"refs/heads/"))
            .map(|name| String::from_utf8_lossy(name.shorten()).into_owned()),
        Err(_) => None,
    };

    Ok(RepoRefState { refs, head })
}

/// [`repo_ref_state`] for the repository at `workdir`.
pub fn worktree_ref_state(workdir: &Path) -> Result<RepoRefState> {
    repo_ref_state(&open_with_cache(workdir)?)
}

/// Everything the browser needs to refresh after a branch or tag switch.
pub struct WorktreeSnapshot {
    /// Relative paths of all worktree entries, directories first.
    pub entries: Vec<PathBuf>,
    /// README path relative to the worktree, if any.
    pub readme_path: Option<PathBuf>,
    /// Contents of the README, if any.
    pub readme: Option<Vec<u8>>,
    /// Branch HEAD points to (`None` when detached, e.g. on a tag).
    pub current_branch: Option<String>,
    /// Commit HEAD points to, if any (see [`head_commit`]).
    pub head_commit: Option<FileCommit>,
}

/// Snapshot the worktree after a branch/tag switch: entries, README, the
/// branch HEAD points to and its commit, opening the repository once.
pub fn worktree_snapshot(workdir: &Path) -> Result<WorktreeSnapshot> {
    let repo = open_with_cache(workdir)?;
    let readme_path = find_readme(&repo)?;
    let readme = match &readme_path {
        Some(path) => worktree_read(&repo, path)?,
        None => None,
    };
    Ok(WorktreeSnapshot {
        entries: worktree_entries(&repo)?,
        readme_path,
        readme,
        current_branch: current_branch(&repo)?,
        head_commit: head_commit(&repo)?,
    })
}

/// Switch the checked-out ref and update the worktree to match, like
/// `git checkout --force`. Local modifications are discarded since these
/// clones are read-only browser copies.
fn checkout(workdir: &Path, args: &[&str]) -> Result<()> {
    let output = Command::new("git")
        .arg("checkout")
        .arg("--force")
        .args(args)
        .current_dir(workdir)
        .output()
        .context("failed to spawn `git checkout`")?;
    if !output.status.success() {
        bail!(
            "git checkout {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

/// Check out the local branch `name`; HEAD stays attached to it.
pub fn worktree_checkout_branch(workdir: &Path, name: &str) -> Result<()> {
    // The short name (not `refs/heads/<name>`) keeps HEAD attached; the
    // full ref name would be treated as a commit-ish and detach it.
    checkout(workdir, &[name])
}

/// Check out the tag `name`; HEAD becomes detached at the tagged commit,
/// which [`current_branch`] reports as `None`.
pub fn worktree_checkout_tag(workdir: &Path, name: &str) -> Result<()> {
    // `--detach` pins the full tag ref so HEAD always ends up detached.
    checkout(workdir, &["--detach", &format!("refs/tags/{name}")])
}

fn collect_entries(root: &Path, dir: &Path, out: &mut Vec<(PathBuf, bool)>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_name() == ".git" {
            continue;
        }

        let is_dir = entry.file_type()?.is_dir();
        let path = entry.path();
        let rel = path.strip_prefix(root)?.to_path_buf();
        out.push((rel, is_dir));

        if is_dir {
            collect_entries(root, &path, out)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use nostr::prelude::*;
    use signed_core::repo_addr;

    use super::*;

    #[test]
    fn keeps_plain_ids() {
        assert_eq!(sanitize_path_component("my-repo"), "my-repo");
        assert_eq!(sanitize_path_component("repo.v2"), "repo.v2");
        assert_eq!(sanitize_path_component("a_b-c"), "a_b-c");
    }

    #[test]
    fn replaces_unsafe_characters() {
        assert_eq!(sanitize_path_component("a/b\\c:d"), "a_b_c_d");
        assert_eq!(sanitize_path_component(""), "");
    }

    #[test]
    fn blocks_parent_components() {
        assert_eq!(sanitize_path_component(".."), "_");
        assert_eq!(sanitize_path_component("."), "_");
        // Separators are neutralized before the check, so these stay safe.
        assert_eq!(sanitize_path_component("../.."), ".._..");
        assert_eq!(sanitize_path_component("a/../b"), "a_.._b");
    }

    #[test]
    fn repo_path_stays_inside_root() {
        let cache = GitCache::new("/cache".into());
        let owner = Keys::generate().public_key();

        let path = cache.repo_path(&repo_addr(owner, ".."));
        assert!(path.starts_with("/cache"));
        assert_eq!(
            path.file_name().map(|n| n.to_string_lossy().into_owned()),
            Some("_".into())
        );
    }

    #[test]
    fn repo_ref_state_lists_branches_tags_and_head() {
        let (_dir, repo) = fixture(&[("a.txt", b"hello")]);
        commit_all(&repo, "initial");
        let workdir = repo.workdir().expect("workdir").to_path_buf();

        let state = repo_ref_state(&repo).expect("refs");

        let branch = current_branch(&repo).expect("branch").expect("on a branch");
        assert_eq!(state.head.as_deref(), Some(branch.as_str()));
        assert_eq!(state.refs.len(), 1);
        assert_eq!(state.refs[0].0, format!("refs/heads/{branch}"));
        assert_eq!(state.refs[0].1.len(), 40);

        // Additional branches and tags are listed alongside.
        git_run(&workdir, &["branch", "feature"]);
        git_run(&workdir, &["tag", "v1.0"]);

        let state = repo_ref_state(&repo).expect("refs");
        let mut expected: Vec<String> = vec![
            format!("refs/heads/{branch}"),
            "refs/heads/feature".to_owned(),
            "refs/tags/v1.0".to_owned(),
        ];
        expected.sort();
        assert_eq!(
            state
                .refs
                .iter()
                .map(|(name, _)| name.clone())
                .collect::<Vec<_>>(),
            expected
        );

        // A detached HEAD yields no head branch.
        git_run(&workdir, &["checkout", "--detach"]);
        let state = repo_ref_state(&repo).expect("refs");
        assert!(state.head.is_none());
        assert_eq!(state.refs.len(), 3);
    }

    /// Build a throwaway non-bare repository with the given files (rel → bytes).
    fn fixture(files: &[(&str, &[u8])]) -> (tempfile::TempDir, gix::Repository) {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = gix::init(&dir).expect("init");

        for (rel, bytes) in files {
            let path = dir.path().join(rel);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
            std::fs::write(&path, bytes).expect("write");
        }

        (dir, repo)
    }

    #[test]
    fn worktree_entries_lists_all_files_and_dirs() {
        let (_dir, repo) = fixture(&[
            ("README.md", b"# Hi"),
            ("src/main.rs", b"fn main() {}"),
            ("src/lib.rs", b""),
            ("docs/guide.md", b"guide"),
        ]);

        let entries = worktree_entries(&repo).expect("entries");
        let entries: Vec<String> = entries
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();

        assert_eq!(
            entries,
            vec![
                "docs",
                "src",
                "README.md",
                "docs/guide.md",
                "src/lib.rs",
                "src/main.rs"
            ]
        );
    }

    #[test]
    fn worktree_read_returns_bytes_or_none() {
        let (_dir, repo) = fixture(&[("a.txt", b"hello"), ("sub/b.bin", b"\x00\x01")]);

        assert_eq!(
            worktree_read(&repo, Path::new("a.txt")).expect("read"),
            Some(b"hello".to_vec())
        );
        assert_eq!(
            worktree_read(&repo, Path::new("sub/b.bin")).expect("read"),
            Some(vec![0x00, 0x01])
        );
        assert_eq!(
            worktree_read(&repo, Path::new("missing.txt")).expect("read"),
            None
        );
    }

    /// Stage everything and create a commit with the git CLI (like
    /// [`apply_patch`], the crate already shells out to the CLI).
    fn commit_all(repo: &gix::Repository, message: &str) {
        git_run(repo.workdir().expect("workdir"), &["add", "-A"]);
        git_run(repo.workdir().expect("workdir"), &["commit", "-m", message]);
    }

    #[test]
    fn init_repository_creates_main_branch_and_readme() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("my-repo");

        let commit = init_repository(&path, "My Repo", "Does things.\n\nCool.").expect("init");
        assert_eq!(commit.len(), 40);

        let repo = gix::open(&path).expect("open");
        let workdir = repo.workdir().expect("workdir");

        assert_eq!(
            std::fs::read_to_string(workdir.join("README.md")).expect("read"),
            "# My Repo\n\nDoes things.\n\nCool.\n"
        );

        let branch = current_branch(&repo).expect("branch").expect("on a branch");
        assert_eq!(branch, "main");
        // [`FileCommit`] carries the short id; the full id is 40 chars.
        assert_eq!(
            head_commit(&repo).expect("head").expect("commit").id,
            &commit[..7]
        );

        let state = repo_ref_state(&repo).expect("refs");
        assert_eq!(state.head.as_deref(), Some("main"));
        assert_eq!(state.refs, vec![("refs/heads/main".to_owned(), commit)]);
    }

    #[test]
    fn init_repository_omits_description_when_empty() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("my-repo");

        init_repository(&path, "My Repo", "   ").expect("init");
        let repo = gix::open(&path).expect("open");
        let workdir = repo.workdir().expect("workdir");

        assert_eq!(
            std::fs::read_to_string(workdir.join("README.md")).expect("read"),
            "# My Repo\n"
        );
    }

    #[test]
    fn ensure_origin_adds_remote_only_once() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("my-repo");
        init_repository(&path, "My Repo", "").expect("init");

        ensure_origin(&path, "https://gitnostr.com/npub1test/repo.git").expect("add");
        assert_eq!(
            git_in(&path, &["remote", "get-url", "origin"]).expect("url"),
            "https://gitnostr.com/npub1test/repo.git"
        );

        // A second call must not override the existing remote.
        ensure_origin(&path, "https://other.example/repo.git").expect("keep");
        assert_eq!(
            git_in(&path, &["remote", "get-url", "origin"]).expect("url"),
            "https://gitnostr.com/npub1test/repo.git"
        );
    }

    /// Run a git command in `dir`, asserting success.
    fn git_run(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "Test Author")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test Author")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .env("GIT_EDITOR", "true")
            .args(args)
            .status()
            .expect("spawn git");
        assert!(status.success(), "git {args:?} failed");
    }

    #[test]
    fn last_commit_returns_most_recent_change() {
        let (dir, repo) = fixture(&[("a.txt", b"one")]);
        commit_all(&repo, "initial");

        std::fs::write(dir.path().join("a.txt"), b"two").expect("write");
        commit_all(&repo, "change a");

        // A commit touching another file must not be reported for a.txt.
        std::fs::write(dir.path().join("b.txt"), b"other").expect("write");
        commit_all(&repo, "add b");

        let commit = last_commit(&repo, Path::new("a.txt"))
            .expect("lookup")
            .expect("found");
        assert_eq!(commit.summary, "change a");
        assert_eq!(commit.author, "Test Author");
        assert!(!commit.id.is_empty());
        assert!(commit.time > 0);
    }

    #[test]
    fn all_commits_lists_every_commit() {
        let (dir, repo) = fixture(&[("a.txt", b"one")]);
        commit_all(&repo, "initial");

        std::fs::write(dir.path().join("a.txt"), b"two").expect("write");
        commit_all(&repo, "second");
        std::fs::write(dir.path().join("b.txt"), b"b").expect("write");
        commit_all(&repo, "third");

        let list = all_commits(&repo).expect("commits");
        assert_eq!(list.total, 3);
        let mut summaries: Vec<&str> = list.commits.iter().map(|c| c.summary.as_str()).collect();
        summaries.sort();
        assert_eq!(summaries, vec!["initial", "second", "third"]);
        assert!(
            list.commits
                .iter()
                .all(|c| c.author == "Test Author" && !c.id.is_empty() && c.time > 0)
        );
    }

    #[test]
    fn all_commits_returns_empty_without_head() {
        let (_dir, repo) = fixture(&[("a.txt", b"one")]);

        let list = all_commits(&repo).expect("commits");
        assert!(list.commits.is_empty());
        assert_eq!(list.total, 0);
    }

    #[test]
    fn last_commit_returns_none_for_untracked_files() {
        let (dir, repo) = fixture(&[("a.txt", b"one")]);
        commit_all(&repo, "initial");
        std::fs::write(dir.path().join("untracked.txt"), b"x").expect("write");

        let commit = last_commit(&repo, Path::new("untracked.txt")).expect("lookup");
        assert!(commit.is_none());
    }

    #[test]
    fn last_commit_reports_merge_commits() {
        let (dir, repo) = fixture(&[("a.txt", b"base")]);
        commit_all(&repo, "initial");

        let run = |args: &[&str]| {
            let status = Command::new("git")
                .current_dir(dir.path())
                .env("GIT_AUTHOR_NAME", "Test Author")
                .env("GIT_AUTHOR_EMAIL", "test@example.com")
                .env("GIT_COMMITTER_NAME", "Test Author")
                .env("GIT_COMMITTER_EMAIL", "test@example.com")
                .env("GIT_EDITOR", "true")
                .args(args)
                .status()
                .expect("spawn git");
            assert!(status.success(), "git {args:?} failed");
        };
        run(&["checkout", "-b", "feature"]);
        std::fs::write(dir.path().join("a.txt"), b"feature").expect("write");
        commit_all(&repo, "feature change");
        run(&["checkout", "-"]);
        // --no-ff forces a merge commit; it is the latest commit changing a.txt.
        run(&["merge", "--no-ff", "--no-edit", "feature"]);

        let commit = last_commit(&repo, Path::new("a.txt"))
            .expect("lookup")
            .expect("found");
        assert_eq!(
            commit.id,
            repo.head_id().expect("head").shorten_or_id().to_string()
        );
        assert!(commit.summary.starts_with("Merge branch"));
    }

    #[test]
    fn last_commits_batches_multiple_paths() {
        let (dir, repo) = fixture(&[("a.txt", b"one"), ("b.txt", b"b")]);
        commit_all(&repo, "initial");

        std::fs::write(dir.path().join("a.txt"), b"two").expect("write");
        commit_all(&repo, "change a");
        std::fs::write(dir.path().join("b.txt"), b"bb").expect("write");
        commit_all(&repo, "change b");

        let found = worktree_last_commits(
            dir.path(),
            &[
                PathBuf::from("a.txt"),
                PathBuf::from("b.txt"),
                // Untracked paths are simply absent from the result.
                PathBuf::from("missing.txt"),
            ],
        )
        .expect("commits");
        let by_path: HashMap<&Path, &FileCommit> = found
            .iter()
            .map(|(path, commit)| (path.as_path(), commit))
            .collect();
        assert_eq!(by_path.len(), 2);
        assert_eq!(by_path[Path::new("a.txt")].summary, "change a");
        assert_eq!(by_path[Path::new("b.txt")].summary, "change b");
    }

    #[test]
    fn find_readme_prefers_markdown() {
        let (_dir, repo) = fixture(&[("readme.txt", b"txt"), ("README.md", b"md")]);

        let readme = find_readme(&repo).expect("find");
        assert_eq!(
            readme.map(|p| p.to_string_lossy().into_owned()),
            Some("README.md".into())
        );
    }

    #[test]
    fn find_readme_falls_back_to_any_readme() {
        let (_dir, repo) = fixture(&[("README.rst", b"rst")]);

        let readme = find_readme(&repo).expect("find");
        assert_eq!(
            readme.map(|p| p.to_string_lossy().into_owned()),
            Some("README.rst".into())
        );
    }

    #[test]
    fn find_readme_returns_none_without_one() {
        let (_dir, repo) = fixture(&[("main.rs", b"")]);
        assert!(find_readme(&repo).expect("find").is_none());
    }

    #[test]
    fn head_commit_reports_head() {
        let (_dir, repo) = fixture(&[("a.txt", b"one")]);

        // Unborn HEAD: no commit yet.
        assert!(head_commit(&repo).expect("head").is_none());

        commit_all(&repo, "initial");
        let head = head_commit(&repo).expect("head").expect("commit");
        assert_eq!(
            head.id,
            repo.head_id().expect("head id").shorten_or_id().to_string()
        );
        assert_eq!(head.summary, "initial");
        assert_eq!(head.author, "Test Author");
    }

    #[test]
    fn worktree_branches_and_tags_list_short_names() {
        let (dir, repo) = fixture(&[("a.txt", b"one")]);
        commit_all(&repo, "initial");
        let dir = dir.path();

        git_run(dir, &["checkout", "-b", "feature"]);
        git_run(dir, &["tag", "v0.9"]);
        git_run(dir, &["tag", "v1.0"]);

        // The initial branch name depends on git configuration; only the
        // branch we created is fixed.
        let branches = worktree_branches(dir).expect("branches");
        assert_eq!(branches.len(), 2);
        assert!(branches.contains(&"feature".to_string()));
        assert!(branches.windows(2).all(|pair| pair[0] <= pair[1]), "sorted");

        assert_eq!(
            worktree_tags(dir).expect("tags"),
            vec!["v0.9".to_string(), "v1.0".to_string()]
        );
    }

    #[test]
    fn current_branch_tracks_checkout() {
        let (dir, repo) = fixture(&[("a.txt", b"one")]);
        commit_all(&repo, "initial");
        let dir = dir.path();

        let default = worktree_branches(dir)
            .expect("branches")
            .into_iter()
            .next()
            .expect("default branch");
        assert_eq!(
            current_branch(&repo).expect("branch").as_deref(),
            Some(default.as_str())
        );

        git_run(dir, &["checkout", "-b", "feature"]);
        assert_eq!(
            current_branch(&repo).expect("branch").as_deref(),
            Some("feature")
        );

        // Tags detach HEAD.
        git_run(dir, &["tag", "v1.0"]);
        worktree_checkout_tag(dir, "v1.0").expect("checkout tag");
        assert_eq!(current_branch(&repo).expect("branch"), None);

        // Branches re-attach HEAD.
        worktree_checkout_branch(dir, &default).expect("checkout branch");
        assert_eq!(
            current_branch(&repo).expect("branch").as_deref(),
            Some(default.as_str())
        );
    }

    #[test]
    fn worktree_snapshot_reflects_checked_out_ref() {
        let (dir, repo) = fixture(&[("README.md", b"# main"), ("a.txt", b"one")]);
        commit_all(&repo, "initial");
        let dir = dir.path();

        git_run(dir, &["checkout", "-b", "feature"]);
        std::fs::write(dir.join("README.md"), b"# feature").expect("write");
        std::fs::write(dir.join("b.txt"), b"b").expect("write");
        commit_all(&repo, "feature work");

        let snapshot = worktree_snapshot(dir).expect("snapshot");
        assert_eq!(snapshot.current_branch.as_deref(), Some("feature"));
        assert_eq!(
            snapshot.head_commit.as_ref().expect("head commit").summary,
            "feature work"
        );
        assert_eq!(
            String::from_utf8(snapshot.readme.expect("readme")).expect("utf8"),
            "# feature"
        );
        let entries: Vec<String> = snapshot
            .entries
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        assert!(entries.contains(&"b.txt".to_string()));

        let default = worktree_branches(dir)
            .expect("branches")
            .into_iter()
            .find(|name| name != "feature")
            .expect("default branch");
        worktree_checkout_branch(dir, &default).expect("checkout");

        let snapshot = worktree_snapshot(dir).expect("snapshot");
        assert_eq!(snapshot.current_branch.as_deref(), Some(default.as_str()));
        assert_eq!(
            snapshot.head_commit.as_ref().expect("head commit").summary,
            "initial"
        );
        assert_eq!(
            String::from_utf8(snapshot.readme.expect("readme")).expect("utf8"),
            "# main"
        );
        assert!(
            !snapshot
                .entries
                .iter()
                .any(|p| p.to_string_lossy() == "b.txt")
        );
    }

    #[test]
    fn commit_diff_lists_added_modified_and_deleted_files() {
        let (dir, repo) = fixture(&[("keep.txt", b"keep"), ("mod.txt", b"one\ntwo\nthree\n")]);
        commit_all(&repo, "initial");

        std::fs::write(dir.path().join("mod.txt"), b"one\ntwo!\nthree\n").expect("write");
        std::fs::write(dir.path().join("new.txt"), b"hello\n").expect("write");
        std::fs::remove_file(dir.path().join("keep.txt")).expect("remove");
        commit_all(&repo, "changes");

        let head = repo.head_id().expect("head").shorten_or_id().to_string();
        let diff = worktree_commit_diff(dir.path(), &head).expect("diff");

        let by_path: HashMap<&str, &FileDiff> = diff
            .files
            .iter()
            .map(|file| (file.path.as_str(), file))
            .collect();
        assert_eq!(by_path.len(), 3);

        let added = by_path["new.txt"];
        assert_eq!(added.status, DiffStatus::Added);
        assert_eq!(added.insertions, 1);
        assert_eq!(added.deletions, 0);
        assert_eq!(added.hunks.len(), 1);
        assert_eq!(added.hunks[0].lines.len(), 1);
        assert_eq!(added.hunks[0].lines[0].kind, DiffLineKind::Addition);
        assert_eq!(added.hunks[0].lines[0].old, None);
        assert_eq!(added.hunks[0].lines[0].new, Some(1));
        assert_eq!(added.hunks[0].lines[0].text, "hello");

        let modified = by_path["mod.txt"];
        assert_eq!(modified.status, DiffStatus::Modified);
        assert_eq!(modified.insertions, 1);
        assert_eq!(modified.deletions, 1);
        assert!(!modified.binary);
        let lines = &modified.hunks[0].lines;
        // One hunk with context around the single-line change: the removed
        // line is old 2, the added line is new 2.
        assert!(lines.iter().any(|line| {
            line.kind == DiffLineKind::Deletion
                && line.old == Some(2)
                && line.new.is_none()
                && line.text == "two"
        }));
        assert!(lines.iter().any(|line| {
            line.kind == DiffLineKind::Addition
                && line.old.is_none()
                && line.new == Some(2)
                && line.text == "two!"
        }));
        assert!(lines.iter().any(|line| {
            line.kind == DiffLineKind::Context && line.old == Some(1) && line.new == Some(1)
        }));

        let deleted = by_path["keep.txt"];
        assert_eq!(deleted.status, DiffStatus::Deleted);
        assert_eq!(deleted.deletions, 1);
        assert_eq!(deleted.hunks[0].lines[0].kind, DiffLineKind::Deletion);
        assert_eq!(deleted.hunks[0].lines[0].old, Some(1));
        assert_eq!(deleted.hunks[0].lines[0].new, None);
    }

    #[test]
    fn commit_range_diff_lists_changes_between_two_commits() {
        let (dir, repo) = fixture(&[("a.txt", b"a\n"), ("b.txt", b"b\n")]);
        commit_all(&repo, "first");
        let base = repo.head_id().expect("head").to_string();

        std::fs::write(dir.path().join("a.txt"), b"changed\n").expect("write");
        std::fs::write(dir.path().join("c.txt"), b"new\n").expect("write");
        commit_all(&repo, "second");
        let tip = repo.head_id().expect("head").to_string();

        let diff = worktree_commit_range_diff(dir.path(), &base, &tip).expect("diff");

        let by_path: HashMap<&str, &FileDiff> = diff
            .files
            .iter()
            .map(|file| (file.path.as_str(), file))
            .collect();
        assert_eq!(by_path.len(), 2);
        assert_eq!(by_path["a.txt"].status, DiffStatus::Modified);
        assert_eq!(by_path["a.txt"].insertions, 1);
        assert_eq!(by_path["a.txt"].deletions, 1);
        assert_eq!(by_path["c.txt"].status, DiffStatus::Added);
        // b.txt is unchanged between the two commits.
        assert!(diff.files.iter().all(|file| file.path != "b.txt"));
    }

    #[test]
    fn commit_range_commits_lists_only_new_commits_newest_first() {
        let (dir, repo) = fixture(&[("a.txt", b"one\n")]);
        commit_all(&repo, "one");
        let base = repo.head_id().expect("head").to_string();

        std::fs::write(dir.path().join("a.txt"), b"two\n").expect("write");
        commit_all(&repo, "two");
        std::fs::write(dir.path().join("a.txt"), b"three\n").expect("write");
        commit_all(&repo, "three");
        let tip = repo.head_id().expect("head").to_string();

        let commits = worktree_commit_range_commits(dir.path(), &base, &tip).expect("commits");

        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].summary, "three");
        assert_eq!(commits[1].summary, "two");
    }

    #[test]
    fn commit_diff_reports_binary_files_without_hunks() {
        let (_dir, repo) = fixture(&[("blob.bin", b"\x00\x01\x02")]);
        commit_all(&repo, "initial");

        std::fs::write(_dir.path().join("blob.bin"), b"\x00\x03").expect("write");
        commit_all(&repo, "binary change");

        let head = repo.head_id().expect("head").shorten_or_id().to_string();
        let diff = worktree_commit_diff(_dir.path(), &head).expect("diff");
        let file = diff
            .files
            .iter()
            .find(|f| f.path == "blob.bin")
            .expect("file");
        assert!(file.binary);
        assert!(file.hunks.is_empty());
        assert_eq!(file.insertions, 0);
        assert_eq!(file.deletions, 0);
    }

    #[test]
    fn commit_diff_resolves_short_ids_and_root_commit() {
        let (dir, repo) = fixture(&[("a.txt", b"one\n")]);
        commit_all(&repo, "initial");

        // The root commit diffs against the empty tree: everything is added.
        let head = repo.head_id().expect("head").shorten_or_id().to_string();
        let diff = worktree_commit_diff(dir.path(), &head).expect("diff");
        assert_eq!(diff.files.len(), 1);
        assert_eq!(diff.files[0].path, "a.txt");
        assert_eq!(diff.files[0].status, DiffStatus::Added);
        assert_eq!(diff.files[0].insertions, 1);
    }

    #[test]
    fn file_commit_includes_message_body() {
        let (_dir, repo) = fixture(&[("a.txt", b"one")]);
        commit_all(&repo, "title");

        // A single-line message has no body.
        let head = head_commit(&repo).expect("head").expect("commit");
        assert_eq!(head.summary, "title");
        assert_eq!(head.description, None);

        // A message with a body exposes it, trimmed.
        let dir = _dir.path();
        let status = Command::new("git")
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "Test Author")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test Author")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .env("GIT_EDITOR", "true")
            .args([
                "commit",
                "--allow-empty",
                "-m",
                "title two",
                "-m",
                "line one\n\nline two",
            ])
            .status()
            .expect("spawn git");
        assert!(status.success(), "git commit failed");

        let head = head_commit(&repo).expect("head").expect("commit");
        assert_eq!(head.summary, "title two");
        assert_eq!(head.description.as_deref(), Some("line one\n\nline two"));
    }

    #[test]
    fn commit_diff_reports_renames() {
        let (_dir, repo) = fixture(&[("old.txt", b"same content\n")]);
        commit_all(&repo, "initial");

        std::fs::rename(_dir.path().join("old.txt"), _dir.path().join("new.txt")).expect("rename");
        commit_all(&repo, "rename");

        let head = repo.head_id().expect("head").shorten_or_id().to_string();
        let diff = worktree_commit_diff(_dir.path(), &head).expect("diff");
        let file = diff
            .files
            .iter()
            .find(|f| f.path == "new.txt")
            .expect("file");
        assert_eq!(file.status, DiffStatus::Renamed);
        assert_eq!(file.old_path.as_deref(), Some("old.txt"));
        // A pure rename has no content change; the file is still listed.
        assert!(file.hunks.is_empty());
        assert_eq!(file.insertions, 0);
        assert_eq!(file.deletions, 0);
    }

    #[test]
    fn parses_format_patch_output() {
        let patch = r#"From 1f6c0c5f3f1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a Mon Sep 17 00:00:00 2001
From: A <a@b.c>
Subject: [PATCH] fix

fix the thing

---
 src/lib.rs | 2 +-
 1 file changed, 1 insertion(+), 1 deletion(-)

diff --git a/src/lib.rs b/src/lib.rs
index 1234567..89abcde 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,3 +1,3 @@
 fn main() {
-    println!("old");
+    println!("new");
 }
"#;
        let diff = patch_diffs(patch).expect("parse");

        assert_eq!(diff.files.len(), 1);
        let file = &diff.files[0];
        assert_eq!(file.path, "src/lib.rs");
        assert_eq!(file.old_path, None);
        assert_eq!(file.status, DiffStatus::Modified);
        assert_eq!(file.insertions, 1);
        assert_eq!(file.deletions, 1);

        let hunk = &file.hunks[0];
        assert_eq!(hunk.old_start, 1);
        assert_eq!(hunk.old_lines, 3);
        assert_eq!(hunk.new_start, 1);
        assert_eq!(hunk.new_lines, 3);
        assert_eq!(hunk.lines.len(), 4);
        assert_eq!(hunk.lines[0].kind, DiffLineKind::Context);
        assert_eq!(hunk.lines[0].old, Some(1));
        assert_eq!(hunk.lines[0].new, Some(1));
        assert_eq!(hunk.lines[1].kind, DiffLineKind::Deletion);
        assert_eq!(hunk.lines[1].old, Some(2));
        assert_eq!(hunk.lines[1].new, None);
        assert_eq!(hunk.lines[2].kind, DiffLineKind::Addition);
        assert_eq!(hunk.lines[2].old, None);
        assert_eq!(hunk.lines[2].new, Some(2));
        assert_eq!(hunk.lines[3].kind, DiffLineKind::Context);
        assert_eq!(hunk.lines[3].old, Some(3));
        assert_eq!(hunk.lines[3].new, Some(3));
    }

    #[test]
    fn parses_new_file_as_added() {
        let patch = r#"diff --git a/README.md b/README.md
new file mode 100644
index 0000000..1234567
--- /dev/null
+++ b/README.md
@@ -0,0 +1 @@
+# hello
"#;
        let diff = patch_diffs(patch).expect("parse");

        let file = &diff.files[0];
        assert_eq!(file.path, "README.md");
        assert_eq!(file.status, DiffStatus::Added);
        assert_eq!(file.old_path, None);
        assert_eq!(file.insertions, 1);
        assert_eq!(file.deletions, 0);
        assert_eq!(file.hunks[0].old_start, 0);
        assert_eq!(file.hunks[0].old_lines, 0);
        assert_eq!(file.hunks[0].new_start, 1);
    }

    #[test]
    fn parses_renames_with_old_path() {
        let patch = r#"diff --git a/old.rs b/new.rs
similarity index 85%
rename from old.rs
rename to new.rs
index 123..456 100644
--- a/old.rs
+++ b/new.rs
@@ -1 +1 @@
-fn main() {}
+fn main() { println!("hi"); }
"#;
        let diff = patch_diffs(patch).expect("parse");

        let file = &diff.files[0];
        assert_eq!(file.path, "new.rs");
        assert_eq!(file.old_path.as_deref(), Some("old.rs"));
        assert_eq!(file.status, DiffStatus::Renamed);
        assert_eq!(file.insertions, 1);
        assert_eq!(file.deletions, 1);
    }

    #[test]
    fn parses_patch_series_and_skips_envelope() {
        let patch = r#"From aaaa Mon Sep 17 00:00:00 2001
From: A <a@b.c>
Subject: [PATCH 1/2] one

---
 a.txt | 1 +
 1 file changed, 1 insertion(+)

diff --git a/a.txt b/a.txt
index 1..2 100644
--- a/a.txt
+++ b/a.txt
@@ -1 +1,2 @@
 a
+b

From bbbb Mon Sep 17 00:00:00 2001
From: A <a@b.c>
Subject: [PATCH 2/2] two

diff --git a/b.txt b/b.txt
index 3..4 100644
--- a/b.txt
+++ b/b.txt
@@ -1 +1 @@
-x
+y
"#;
        let diff = patch_diffs(patch).expect("parse");

        assert_eq!(diff.files.len(), 2);
        assert_eq!(diff.files[0].path, "a.txt");
        assert_eq!(diff.files[0].insertions, 1);
        assert_eq!(diff.files[1].path, "b.txt");
        assert_eq!(diff.files[1].deletions, 1);
    }

    #[test]
    fn patch_commits_lists_every_patch_in_order() {
        let patch = r#"From 1111111111111111111111111111111111111111 Mon Sep 17 00:00:00 2001
From: Alice <alice@example.com>
Date: Tue, 1 Aug 2023 10:00:00 +0200
Subject: [PATCH 1/2] first

body one
---
 a.txt | 1 +
 1 file changed, 1 insertion(+)

diff --git a/a.txt b/a.txt
@@ -1 +1,2 @@
 a
+b

From 2222222222222222222222222222222222222222 Mon Sep 17 00:00:00 2001
From: Bob <bob@example.com>
Date: Wed, 2 Aug 2023 11:30:00 +0000
Subject: [PATCH 2/2] second

body two
---
 b.txt | 1 +
 1 file changed, 1 insertion(+)

diff --git a/b.txt b/b.txt
@@ -1 +1,2 @@
 x
+y
"#;

        let commits = patch_commits(patch);
        assert_eq!(commits.len(), 2);

        assert_eq!(commits[0].id, "1111111111111111111111111111111111111111");
        assert_eq!(commits[0].summary, "first");
        assert_eq!(commits[0].author, "Alice");
        assert_eq!(commits[0].time, 1690876800);

        assert_eq!(commits[1].id, "2222222222222222222222222222222222222222");
        assert_eq!(commits[1].summary, "second");
        assert_eq!(commits[1].author, "Bob");
        assert_eq!(commits[1].time, 1690975800);
    }

    #[test]
    fn patch_commits_strips_patch_subject_prefixes() {
        let patch = r#"From 1111111111111111111111111111111111111111 Mon Sep 17 00:00:00 2001
From: A <a@b.c>
Subject: [RFC PATCH v3 4/7] the real title

---
"#;

        let commits = patch_commits(patch);
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].summary, "the real title");
    }

    #[test]
    fn patch_commits_handles_missing_headers() {
        // A hand-written patch without author/date headers still lists a
        // commit; time stays 0 and the author falls back to the raw value.
        let patch = r#"From 1111111111111111111111111111111111111111 Mon Sep 17 00:00:00 2001
Subject: [PATCH] plain

---
"#;

        let commits = patch_commits(patch);
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].summary, "plain");
        assert_eq!(commits[0].author, "");
        assert_eq!(commits[0].time, 0);
    }

    #[test]
    fn patch_commits_ignores_non_patch_lines() {
        assert!(patch_commits("").is_empty());
        assert!(patch_commits("just some text\nFrom 123\n").is_empty());
        // A diff-only body (no mbox envelope) has no commits.
        let patch = "diff --git a/x b/x\n--- a/x\n+++ b/x\n";
        assert!(patch_commits(patch).is_empty());
    }

    #[test]
    fn marks_binary_sections() {
        let patch = r#"diff --git a/img.png b/img.png
index 123..456 100644
Binary files a/img.png and b/img.png differ
"#;
        let diff = patch_diffs(patch).expect("parse");

        assert!(diff.files[0].binary);
        assert!(diff.files[0].hunks.is_empty());
    }

    #[test]
    fn unquotes_quoted_paths() {
        let patch = r#"diff --git "a/weird file.rs" "b/weird file.rs"
index 123..456 100644
--- "a/weird file.rs"
+++ "b/weird file.rs"
@@ -1 +1 @@
-x
+y
"#;
        let diff = patch_diffs(patch).expect("parse");

        assert_eq!(diff.files[0].path, "weird file.rs");
        assert_eq!(diff.files[0].status, DiffStatus::Modified);
    }

    #[test]
    fn unquotes_non_ascii_quoted_paths() {
        let patch = r#"diff --git "a/说明.md" "b/说明.md"
index 123..456 100644
--- "a/说明.md"
+++ "b/说明.md"
@@ -1 +1 @@
-x
+y
"#;
        let diff = patch_diffs(patch).expect("parse");

        assert_eq!(diff.files[0].path, "说明.md");
        assert_eq!(diff.files[0].status, DiffStatus::Modified);
    }

    #[test]
    fn unquotes_octal_escaped_paths() {
        let patch = r#"diff --git "a/\345\270\226.md" "b/\345\270\226.md"
index 123..456 100644
--- "a/\345\270\226.md"
+++ "b/\345\270\226.md"
@@ -1 +1 @@
-x
+y
"#;
        let diff = patch_diffs(patch).expect("parse");

        assert_eq!(diff.files[0].path, "帖.md");
        assert_eq!(diff.files[0].status, DiffStatus::Modified);
    }

    #[test]
    fn empty_or_unparseable_patch_yields_no_files() {
        assert_eq!(patch_diffs("").expect("parse").files.len(), 0);
        assert_eq!(patch_diffs("just some text").expect("parse").files.len(), 0);
        assert_eq!(
            patch_diffs("---\nnot a patch\n")
                .expect("parse")
                .files
                .len(),
            0
        );
    }

    #[test]
    fn parses_real_format_patch_output() {
        // Build a commit touching a mix of file kinds, then feed genuine
        // `git format-patch` output through the parser: quoted paths (space
        // in the name), octal-escaped paths (UTF-8 name), a rename-free
        // modification, an addition and a binary deletion.
        let (dir, repo) = fixture(&[
            ("src/main.rs", b"fn main() {\n    println!(\"one\");\n}\n"),
            ("my file.txt", b"hello\n"),
            ("\u{8bf4}\u{660e}.md", "# \u{8bf4}\u{660e}\n".as_bytes()),
            ("img.png", b"\x89PNG\r\n\x1a\n\x00binary"),
        ]);
        commit_all(&repo, "initial");

        std::fs::write(
            dir.path().join("src/main.rs"),
            b"fn main() {\n    println!(\"two\");\n    println!(\"three\");\n}\n",
        )
        .expect("write");
        std::fs::write(dir.path().join("my file.txt"), b"hello world\n").expect("write");
        std::fs::write(
            dir.path().join("\u{8bf4}\u{660e}.md"),
            "# \u{8bf4}\u{660e}\nupdated\n",
        )
        .expect("write");
        std::fs::remove_file(dir.path().join("img.png")).expect("remove");
        std::fs::write(dir.path().join("new file.md"), b"# new\n").expect("write");
        commit_all(&repo, "changes");

        let output = Command::new("git")
            .current_dir(dir.path())
            .env("GIT_AUTHOR_NAME", "Test Author")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test Author")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .args(["format-patch", "-1", "--stdout"])
            .output()
            .expect("spawn git format-patch");
        assert!(output.status.success(), "git format-patch failed");
        let patch = String::from_utf8(output.stdout).expect("patch is utf-8");

        let diff = patch_diffs(&patch).expect("parse real format-patch output");

        let by_path = |path: &str| {
            diff.files
                .iter()
                .find(|file| file.path == path)
                .unwrap_or_else(|| panic!("missing file {path:?}"))
        };

        // Space in the name: git quotes the path in the header.
        let file = by_path("my file.txt");
        assert_eq!(file.status, DiffStatus::Modified);
        assert_eq!(file.insertions, 1);

        // UTF-8 name: git emits the path as octal escapes.
        let file = by_path("\u{8bf4}\u{660e}.md");
        assert_eq!(file.status, DiffStatus::Modified);
        assert_eq!(file.insertions, 1);

        let file = by_path("src/main.rs");
        assert_eq!(file.status, DiffStatus::Modified);
        assert_eq!(file.insertions, 2);
        assert_eq!(file.deletions, 1);
        assert!(!file.hunks.is_empty());

        let file = by_path("new file.md");
        assert_eq!(file.status, DiffStatus::Added);
        assert_eq!(file.insertions, 1);

        // Binary deletion: git emits no ---/+++ lines, only the mode and
        // the "Binary files" marker.
        let file = by_path("img.png");
        assert_eq!(file.status, DiffStatus::Deleted);
        assert!(file.binary);
        assert!(file.hunks.is_empty());
    }
}

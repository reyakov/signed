use std::collections::{BTreeSet, HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{Context, Result, bail};
use gix::bstr::ByteSlice;
use gix::interrupt::IS_INTERRUPTED;
use gix::progress::Discard;
use nostr::prelude::*;

use crate::GixResultExt as _;
use crate::diff::{CommitDiff, DiffStatus, FileDiff, HunkCollector};
use crate::history::{CommitList, FileCommit, MAX_LISTED_COMMITS};
use crate::nip34::{GraspSignals, Nip34Binding, Nip34Json, Nip34Kind};
use crate::sync::{RefSync, RepoSyncStatus};
use crate::worktree::WorktreeSnapshot;

const OBJECT_CACHE_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoRefState {
    pub refs: Vec<(String, String)>,
    pub head: Option<String>,
}

impl RepoRefState {
    /// Creates a ref state from its refs and current branch.
    fn new(refs: Vec<(String, String)>, head: Option<String>) -> Self {
        Self { refs, head }
    }
}

pub struct Repo {
    pub(crate) inner: gix::Repository,
}

impl Repo {
    /// Wraps an already opened gix repository.
    pub(crate) fn new(inner: gix::Repository) -> Self {
        Self { inner }
    }

    /// Opens the repository at `workdir`.
    pub fn open(workdir: &Path) -> Result<Self> {
        Ok(Self::new(gix::open(workdir)?))
    }

    /// Opens the repository at `workdir`, or `None` on failure.
    pub fn try_open(workdir: &Path) -> Option<Self> {
        Self::open(workdir).ok()
    }

    /// Opens the repository at `workdir` with a larger object cache for big diffs.
    pub fn open_cached(workdir: &Path) -> Result<Self> {
        let mut repo = gix::open(workdir)?;
        repo.object_cache_size_if_unset(OBJECT_CACHE_BYTES);
        Ok(Self::new(repo))
    }

    /// Initializes a repository at `path` with a README and an initial commit.
    pub fn init(path: &Path, name: &str, description: &str) -> Result<String> {
        use gix::refs::transaction::{Change, LogChange, PreviousValue, RefEdit, RefLog};

        std::fs::create_dir_all(path)
            .with_context(|| format!("failed to create {}", path.display()))?;

        let repo = gix::init(path)?;

        let (signature, mut time_buf) = Self::repository_signature();
        let signature = signature.to_ref(&mut time_buf);

        let head = gix::refs::FullName::try_from("HEAD")
            .map_err(|e| anyhow::anyhow!("invalid ref name: {e}"))?;

        repo.edit_references_as(
            [RefEdit {
                change: Change::Update {
                    log: LogChange {
                        mode: RefLog::AndReference,
                        force_create_reflog: false,
                        message: "checkout: moving to main".into(),
                    },
                    expected: PreviousValue::Any,
                    new: gix::refs::Target::Symbolic(
                        gix::refs::FullName::try_from("refs/heads/main")
                            .map_err(|e| anyhow::anyhow!("invalid ref name: {e}"))?,
                    ),
                },
                name: head,
                deref: false,
            }],
            Some(signature),
        )?;

        let readme = if description.trim().is_empty() {
            format!("# {name}\n")
        } else {
            format!("# {name}\n\n{description}\n")
        };

        std::fs::write(path.join("README.md"), &readme).context("failed to write README.md")?;

        let blob = repo.write_object(gix::objs::Blob {
            data: readme.into_bytes(),
        })?;

        let tree = repo.write_object(gix::objs::Tree {
            entries: vec![gix::objs::tree::Entry {
                mode: gix::objs::tree::EntryKind::Blob.into(),
                filename: gix::bstr::BString::from("README.md"),
                oid: blob.into(),
            }],
        })?;

        let commit = repo.commit_as(
            signature,
            signature,
            "HEAD",
            "Initial commit",
            tree,
            Vec::<gix::ObjectId>::new(),
        )?;

        let mut index = repo.index_from_tree(&tree)?;
        index
            .write(gix::index::write::Options::default())
            .into_anyhow()?;

        Ok(commit.to_string())
    }

    /// Clones the first working URL into `path` and fetches the NIP-34 refs.
    pub fn clone<U: AsRef<str>>(clone_urls: &[U], path: &Path) -> Result<Self> {
        if path.exists() {
            anyhow::bail!("destination {} already exists", path.display());
        }

        let mut last_error = None;

        for url in clone_urls {
            match Self::clone_from(url.as_ref(), path) {
                Ok(repo) => {
                    if let Err(error) = repo.fetch() {
                        log::warn!("failed to fetch NIP-34 refs after cloning: {error:#}");
                    }
                    return Ok(repo);
                }
                Err(error) => last_error = Some(error),
            }
        }

        match last_error {
            Some(error) => Err(error).context("failed to clone from any mirror"),
            None => anyhow::bail!("no clone URLs provided"),
        }
    }

    /// Clones a single URL into `path`.
    fn clone_from(url: &str, path: &Path) -> Result<Self> {
        let url = Self::transport_url(url);
        let url = gix::url::parse(url)
            .into_anyhow()
            .context("invalid clone URL")?;

        let mut prepare = gix::prepare_clone(url, path)?;
        let (mut checkout, _fetch) = prepare.fetch_then_checkout(Discard, &IS_INTERRUPTED)?;
        let (repo, _checkout) = checkout.main_worktree(Discard, &IS_INTERRUPTED)?;

        Ok(Self::new(repo))
    }

    /// Returns the underlying gix repository.
    pub fn inner(&self) -> &gix::Repository {
        &self.inner
    }

    /// Returns the worktree directory, or `None` for a bare repository.
    pub fn workdir(&self) -> Option<&Path> {
        self.inner.workdir()
    }

    /// Returns the worktree directory, or `.` for a bare repository.
    pub(crate) fn workdir_or_dot(&self) -> &Path {
        self.inner.workdir().unwrap_or_else(|| Path::new("."))
    }

    /// Returns the HEAD commit id, or `None` for an unborn HEAD.
    pub fn head(&self) -> Option<String> {
        self.inner.head_id().ok().map(|id| id.to_string())
    }

    /// Returns the merge base of `a` and `b`, or `None` for unrelated histories.
    pub fn merge_base(&self, a: &str, b: &str) -> Result<Option<String>> {
        let a = self.inner.rev_parse_single(a.as_bytes())?;
        let b = self.inner.rev_parse_single(b.as_bytes())?;
        let bases = self.inner.merge_bases_many(a.detach(), &[b.detach()])?;
        Ok(bases.first().map(|id| id.to_string()))
    }

    /// Lists commit ids reachable from HEAD but not `base`, oldest first.
    pub fn commits_since(&self, base: Option<&str>) -> Result<Vec<String>> {
        let head = match self.inner.head_id() {
            Ok(head) => head,
            Err(_) if base.is_none() => return Ok(Vec::new()),
            Err(e) => return Err(e).context("repository has no commits"),
        };

        let Some(base) = base else {
            return Ok(vec![head.to_string()]);
        };

        let base = self.inner.rev_parse_single(base.as_bytes())?;
        let mut commits = Vec::new();

        for info in self
            .inner
            .rev_walk([head])
            .sorting(gix::revision::walk::Sorting::ByCommitTime(
                gix::traverse::commit::simple::CommitTimeOrder::NewestFirst,
            ))
            .with_hidden([base])
            .all()?
        {
            commits.push(info?.id().to_string());
        }

        commits.reverse();

        Ok(commits)
    }

    /// Returns the first commit of the history, or `None` without commits.
    pub fn root_commit(&self) -> Result<Option<String>> {
        let Ok(head) = self.inner.head_id() else {
            return Ok(None);
        };

        for info in self
            .inner
            .rev_walk([head])
            .sorting(gix::revision::walk::Sorting::ByCommitTime(
                gix::traverse::commit::simple::CommitTimeOrder::NewestFirst,
            ))
            .all()?
        {
            let info = info?;
            if info.parent_ids().next().is_none() {
                return Ok(Some(info.id().to_string()));
            }
        }

        Ok(None)
    }

    /// Lists sorted ref names equal to or nested under `prefix`.
    pub fn refs_with_prefix(&self, prefix: &str) -> Result<Vec<String>> {
        let pattern = prefix.trim_end_matches('/');
        let mut names = Vec::new();

        for reference in self.inner.references()?.all()? {
            let reference = reference.map_err(|error| anyhow::anyhow!("{error}"))?;
            let name = String::from_utf8_lossy(reference.name().as_bstr()).into_owned();

            let under_pattern = name
                .strip_prefix(pattern)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'));

            if under_pattern {
                names.push(name);
            }
        }

        names.sort();

        Ok(names)
    }

    /// Deletes every ref under `prefix`.
    pub fn delete_refs_with_prefix(&self, prefix: &str) -> Result<()> {
        use gix::refs::transaction::{Change, PreviousValue, RefEdit, RefLog};

        let refs = self.refs_with_prefix(prefix)?;
        if refs.is_empty() {
            return Ok(());
        }

        let edits: Vec<RefEdit> = refs
            .iter()
            .map(|name| {
                let full = gix::refs::FullName::try_from(name.as_str())
                    .map_err(|e| anyhow::anyhow!("invalid ref name {name}: {e}"))?;
                Ok(RefEdit {
                    change: Change::Delete {
                        expected: PreviousValue::Any,
                        log: RefLog::AndReference,
                    },
                    name: full,
                    deref: false,
                })
            })
            .collect::<Result<Vec<_>>>()?;

        self.inner.edit_references(edits)?;

        Ok(())
    }

    /// Returns the checked out branch name, or `None` when detached.
    pub fn current_branch(&self) -> Option<String> {
        let head = self.inner.head().ok()?;
        let name = head.referent_name()?;
        Some(String::from_utf8_lossy(name.shorten()).into_owned())
    }

    /// Returns whether a ref named `name` exists.
    pub fn ref_exists(&self, name: &str) -> bool {
        self.inner.find_reference(name).is_ok()
    }

    /// Lists sorted local branch names.
    pub fn branches(&self) -> Result<Vec<String>> {
        let mut names = Vec::new();
        for reference in self.inner.references()?.local_branches()? {
            let reference = reference.map_err(|error| anyhow::anyhow!("{error}"))?;
            names.push(String::from_utf8_lossy(reference.name().shorten()).into_owned());
        }
        names.sort();
        Ok(names)
    }

    /// Lists sorted tag names.
    pub fn tags(&self) -> Result<Vec<String>> {
        let mut names = Vec::new();
        for reference in self.inner.references()?.tags()? {
            let reference = reference.map_err(|error| anyhow::anyhow!("{error}"))?;
            names.push(String::from_utf8_lossy(reference.name().shorten()).into_owned());
        }
        names.sort();
        Ok(names)
    }

    /// Snapshots branch and tag refs with their oids plus the current branch.
    pub fn ref_state(&self) -> Result<RepoRefState> {
        let mut refs = Vec::new();

        for reference in self.inner.references()?.local_branches()? {
            let reference = reference.map_err(|error| anyhow::anyhow!("{error}"))?;
            refs.push((
                String::from_utf8_lossy(reference.name().as_bstr()).into_owned(),
                reference.id().to_string(),
            ));
        }

        for reference in self.inner.references()?.tags()? {
            let reference = reference.map_err(|error| anyhow::anyhow!("{error}"))?;
            refs.push((
                String::from_utf8_lossy(reference.name().as_bstr()).into_owned(),
                reference.id().to_string(),
            ));
        }
        refs.sort();

        let head = match self.inner.head() {
            Ok(head) => head
                .referent_name()
                .filter(|name| name.as_bstr().starts_with(b"refs/heads/"))
                .map(|name| String::from_utf8_lossy(name.shorten()).into_owned()),
            Err(_) => None,
        };

        Ok(RepoRefState::new(refs, head))
    }

    /// Fast-forwards local branches to their origin counterparts, reporting any move.
    pub fn fast_forward_branches(&self) -> Result<bool> {
        use gix::refs::transaction::{Change, LogChange, PreviousValue, RefEdit, RefLog};

        if self.workdir().is_none() {
            return Ok(false);
        }

        let current = self.current_branch();
        let heads = self.refs_with_prefix("refs/heads")?;

        let (signature, mut time_buf) = Self::repository_signature();
        let signature = signature.to_ref(&mut time_buf);

        let mut moved = false;

        for head in heads {
            let Some(branch) = head.strip_prefix("refs/heads/") else {
                continue;
            };

            let remote = format!("refs/remotes/origin/{branch}");
            let Ok(mut remote_reference) = self.inner.find_reference(&remote) else {
                continue;
            };

            let Ok(mut local_reference) = self.inner.find_reference(&head) else {
                continue;
            };

            let Ok(remote_oid) = remote_reference.peel_to_id() else {
                continue;
            };

            let Ok(local_oid) = local_reference.peel_to_id() else {
                continue;
            };

            let remote_oid = remote_oid.detach();
            let local_oid = local_oid.detach();

            if local_oid == remote_oid {
                continue;
            }

            let Ok(base) = self.inner.merge_base(local_oid, remote_oid) else {
                continue;
            };

            if base != local_oid {
                continue;
            }

            let full = gix::refs::FullName::try_from(head.as_str())
                .map_err(|e| anyhow::anyhow!("invalid ref name: {e}"))?;

            let edit = |new: gix::refs::Target| RefEdit {
                change: Change::Update {
                    log: LogChange {
                        mode: RefLog::AndReference,
                        force_create_reflog: false,
                        message: format!("merge {remote}: Fast-forward").into(),
                    },
                    expected: PreviousValue::ExistingMustMatch(gix::refs::Target::Object(
                        local_oid,
                    )),
                    new,
                },
                name: full.clone(),
                deref: false,
            };

            if current.as_deref() == Some(branch) {
                if self.is_dirty() {
                    continue;
                }

                let tree = self.inner.find_object(remote_oid)?.peel_to_tree()?.id;

                self.force_checkout(&tree)?;

                self.inner.edit_references_as(
                    [edit(gix::refs::Target::Object(remote_oid))],
                    Some(signature),
                )?;

                moved = true;
            } else {
                self.inner.edit_references_as(
                    [edit(gix::refs::Target::Object(remote_oid))],
                    Some(signature),
                )?;

                moved = true;
            }
        }

        Ok(moved)
    }

    /// Diffs a commit against its first parent, skipping trees and submodules.
    pub fn commit_diff(&self, id: &str) -> Result<CommitDiff> {
        let commit_id = self.inner.rev_parse_single(id.as_bytes())?;
        let commit = commit_id.object()?.into_commit();
        let new_tree = commit.tree()?;
        let old_tree = match commit.parent_ids().next() {
            Some(parent) => Some(parent.object()?.into_commit().tree()?),
            None => None,
        };
        Self::tree_diff(self, old_tree.as_ref(), &new_tree)
    }

    /// Diffs the trees of `base` and `tip` directly, files sorted by path.
    pub fn range_diff(&self, base: &str, tip: &str) -> Result<CommitDiff> {
        let base_tree = self
            .inner
            .rev_parse_single(base.as_bytes())?
            .object()?
            .into_commit()
            .tree()?;
        let tip_tree = self
            .inner
            .rev_parse_single(tip.as_bytes())?
            .object()?
            .into_commit()
            .tree()?;
        Self::tree_diff(self, Some(&base_tree), &tip_tree)
    }

    /// Produces file diffs between two trees, skipping trees and submodules.
    fn tree_diff(
        repo: &Repo,
        old_tree: Option<&gix::Tree<'_>>,
        new_tree: &gix::Tree<'_>,
    ) -> Result<CommitDiff> {
        use gix::diff::blob::platform::prepare_diff::Operation;
        use gix::object::tree::diff::Change;
        use gix::objs::tree::EntryKind;

        let changes = repo
            .inner
            .diff_tree_to_tree(old_tree, Some(new_tree), None)?;

        let mut cache = repo.inner.diff_resource_cache_for_tree_diff()?;
        let mut files = Vec::new();

        for change in changes {
            let attached = Change::from_change_ref(change.to_ref(), &repo.inner, &repo.inner);

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

            let platform = attached.diff(&mut cache)?;
            platform
                .resource_cache
                .options
                .skip_internal_diff_if_external_is_configured = true;
            let outcome = platform.resource_cache.prepare_diff().into_anyhow()?;

            let (binary, hunks, insertions, deletions) = match outcome.operation {
                Operation::InternalDiff { algorithm } => {
                    let input = outcome.interned_input();
                    let diff = gix::diff::blob::diff_with_slider_heuristics(algorithm, &input);

                    let mut hunks = Vec::new();
                    let mut insertions = 0usize;
                    let mut deletions = 0usize;
                    let collector = HunkCollector::new(&mut hunks, &mut insertions, &mut deletions);
                    gix::diff::blob::UnifiedDiff::new(&diff, &input, collector, Default::default())
                        .consume()?;
                    (false, hunks, insertions, deletions)
                }
                Operation::SourceOrDestinationIsBinary => (true, Vec::new(), 0, 0),
                Operation::ExternalCommand { .. } => {
                    unreachable!("external diff drivers are disabled")
                }
            };

            files.push(FileDiff::new(
                String::from_utf8_lossy(&path).into_owned(),
                old_path.map(|p| String::from_utf8_lossy(&p).into_owned()),
                status,
                insertions,
                deletions,
                binary,
                hunks,
            ));
        }

        files.sort_by(|a, b| a.path.cmp(&b.path));

        Ok(CommitDiff::new(files))
    }

    /// Finds the newest commit touching each path, skipping paths never committed.
    pub fn last_commits(&self, rels: &[PathBuf]) -> Result<Vec<(PathBuf, FileCommit)>> {
        use gix::traverse::commit::simple::CommitTimeOrder;

        let Some(head) = self.inner.head_id().ok() else {
            return Ok(Vec::new());
        };

        let mut pending: Vec<PathBuf> = Vec::with_capacity(rels.len());
        let mut seen: HashSet<&Path> = HashSet::with_capacity(rels.len());

        for rel in rels {
            if seen.insert(rel.as_path()) {
                pending.push(rel.clone());
            }
        }

        let walk = self
            .inner
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

            let mut ix = 0;
            while ix < pending.len() {
                let rel = &pending[ix];
                let blob = tree.lookup_entry_by_path(rel)?;
                let parent_blob = match &parent_tree {
                    Some(tree) => tree.lookup_entry_by_path(rel)?,
                    None => None,
                };

                if blob.map(|entry| entry.id().detach())
                    != parent_blob.map(|entry| entry.id().detach())
                {
                    found.push((rel.clone(), FileCommit::from_commit(&commit)?));
                    pending.swap_remove(ix);
                } else {
                    ix += 1;
                }
            }
        }

        Ok(found)
    }

    /// Lists at most `MAX_LISTED_COMMITS` commits plus the real total.
    pub fn all_commits(&self) -> Result<CommitList> {
        use gix::traverse::commit::simple::CommitTimeOrder;

        let Some(head) = self.inner.head_id().ok() else {
            return Ok(CommitList::new(0, Vec::new()));
        };

        let walk = self
            .inner
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
                commits.push(FileCommit::from_commit_summary(&info.object()?)?);
            }
        }

        Ok(CommitList::new(total, commits))
    }

    /// Lists commits in `base..tip`, newest first.
    pub fn commit_range(&self, base: &str, tip: &str) -> Result<Vec<FileCommit>> {
        use gix::traverse::commit::simple::CommitTimeOrder;

        let base_id = self.inner.rev_parse_single(base.as_bytes())?;
        let tip_id = self.inner.rev_parse_single(tip.as_bytes())?;
        let walk = self
            .inner
            .rev_walk([tip_id])
            .sorting(gix::revision::walk::Sorting::ByCommitTime(
                CommitTimeOrder::NewestFirst,
            ))
            .with_hidden([base_id]);

        let mut commits = Vec::new();

        for info in walk.all()? {
            let info = info?;
            commits.push(FileCommit::from_commit_summary(&info.object()?)?);
        }

        Ok(commits)
    }

    /// Returns the HEAD commit, or `None` for an unborn HEAD.
    pub fn head_commit(&self) -> Result<Option<FileCommit>> {
        let Some(head) = self.inner.head_id().ok() else {
            return Ok(None);
        };
        let commit = head.object()?.into_commit();
        Ok(Some(FileCommit::from_commit(&commit)?))
    }

    /// Resolves `id` to a commit, or `None` when unknown.
    pub fn commit(&self, id: &str) -> Result<Option<FileCommit>> {
        match self.inner.rev_parse_single(id.as_bytes()) {
            Ok(commit_id) => {
                let commit = commit_id.object()?.into_commit();
                Ok(Some(FileCommit::from_commit(&commit)?))
            }
            Err(_) => Ok(None),
        }
    }

    /// Returns whether the worktree has index or untracked changes.
    pub fn is_dirty(&self) -> bool {
        match self.inner.is_dirty() {
            Ok(true) => return true,
            Ok(false) => {}
            Err(_) => return false,
        }

        let Ok(platform) = self.inner.status(Discard) else {
            return false;
        };

        let Ok(mut changes) = platform.into_index_worktree_iter(Vec::<gix::bstr::BString>::new())
        else {
            return false;
        };

        for change in changes.by_ref() {
            match change {
                Ok(gix::status::index_worktree::Item::DirectoryContents { .. }) => return true,
                Ok(_) => {}
                Err(_) => return false,
            }
        }

        false
    }

    /// Counts commits in `branch` since `base`, or 0 when either is unknown.
    pub fn commits_ahead(&self, base: &str, branch: &str) -> u32 {
        let (Some(base), Some(branch)) = (self.resolve_commit(base), self.resolve_commit(branch))
        else {
            return 0;
        };

        let Ok(walk) = self.inner.rev_walk([branch]).with_hidden([base]).all() else {
            return 0;
        };

        walk.filter_map(Result::ok).count().min(u32::MAX as usize) as u32
    }

    /// Resolves a revision to a gix id.
    fn resolve_commit<'a>(&'a self, rev: &str) -> Option<gix::Id<'a>> {
        self.inner.rev_parse_single(rev.as_bytes()).ok()
    }

    /// Classifies every local branch against the Nostr state refs, ignoring tags.
    pub fn sync_status(&self, remote_refs: &[(String, String)]) -> Result<RepoSyncStatus> {
        let local_refs = self.ref_state()?;
        let local = RepoSyncStatus::branches(&local_refs.refs);
        let remote = RepoSyncStatus::branches(remote_refs);
        let names: BTreeSet<&str> = local.keys().chain(remote.keys()).copied().collect();

        let mut refs = Vec::new();
        let mut ahead_total = 0;
        let mut behind_total = 0;

        for name in names {
            let sync = match (local.get(name), remote.get(name)) {
                (Some(local_commit), Some(remote_commit)) => {
                    self.branch_sync(local_commit, remote_commit)
                }
                (Some(_), None) => RefSync::LocalOnly,
                (None, Some(_)) => RefSync::RemoteOnly,
                (None, None) => continue,
            };

            match &sync {
                RefSync::LocalAhead { ahead } => ahead_total += ahead,
                RefSync::RemoteAhead { behind } => behind_total += behind,
                RefSync::Diverged { ahead, behind } => {
                    ahead_total += ahead;
                    behind_total += behind;
                }
                RefSync::InSync | RefSync::LocalOnly | RefSync::RemoteOnly => {}
            }

            refs.push((name.to_owned(), sync));
        }

        Ok(RepoSyncStatus {
            refs,
            ahead_total,
            behind_total,
        })
    }

    /// Classifies a branch present on both sides.
    fn branch_sync(&self, local_commit: &str, remote_commit: &str) -> RefSync {
        if local_commit == remote_commit {
            return RefSync::InSync;
        }

        match self.merge_base(local_commit, remote_commit) {
            Ok(Some(base)) if base == remote_commit => RefSync::LocalAhead {
                ahead: self.commits_ahead(&base, local_commit) as usize,
            },
            Ok(Some(base)) if base == local_commit => RefSync::RemoteAhead {
                behind: self.commits_ahead(&base, remote_commit) as usize,
            },
            Ok(Some(base)) => RefSync::Diverged {
                ahead: self.commits_ahead(&base, local_commit) as usize,
                behind: self.commits_ahead(&base, remote_commit) as usize,
            },
            // Unrelated histories share no base, so every commit counts.
            Ok(None) => RefSync::Diverged {
                ahead: self.count_reachable(local_commit),
                behind: self.count_reachable(remote_commit),
            },
            // Nostr tip missing locally, so at least that commit differs.
            Err(_) => RefSync::RemoteAhead { behind: 1 },
        }
    }

    /// Counts commits reachable from a commit id.
    fn count_reachable(&self, commit: &str) -> usize {
        let Ok(commit_id) = self.inner.rev_parse_single(commit.as_bytes()) else {
            return 0;
        };

        match self.inner.rev_walk([commit_id]).all() {
            Ok(walk) => walk.filter_map(Result::ok).count(),
            Err(_) => 0,
        }
    }

    /// Lists every worktree path relative to the root, directories first.
    pub fn entries(&self) -> Result<Vec<PathBuf>> {
        let workdir = self.inner.workdir().context("repository has no worktree")?;

        let mut entries: Vec<(PathBuf, bool)> = Vec::new();
        Self::collect_entries(workdir, workdir, &mut entries)?;

        entries.sort_by(|(a, a_is_dir), (b, b_is_dir)| {
            b_is_dir
                .cmp(a_is_dir)
                .then_with(|| a.as_os_str().cmp(b.as_os_str()))
        });
        Ok(entries.into_iter().map(|(path, _)| path).collect())
    }

    /// Reads a worktree file, or `None` when missing.
    pub fn read(&self, rel: &Path) -> Result<Option<Vec<u8>>> {
        let workdir = self.inner.workdir().context("repository has no worktree")?;
        let path = workdir.join(rel);

        match std::fs::read(&path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) if e.kind() == std::io::ErrorKind::IsADirectory => Ok(None),
            Err(e) => Err(e).with_context(|| format!("failed to read {}", path.display())),
        }
    }

    /// Finds the top-level README, preferring markdown extensions.
    pub fn find_readme(&self) -> Result<Option<PathBuf>> {
        let Some(workdir) = self.inner.workdir() else {
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

    /// Captures worktree entries, README, and ref state in one pass.
    pub fn snapshot(&self) -> Result<WorktreeSnapshot> {
        let readme_path = self.find_readme()?;
        let readme = match &readme_path {
            Some(path) => self.read(path)?,
            None => None,
        };
        Ok(WorktreeSnapshot::new(
            self.entries()?,
            readme_path,
            readme,
            self.current_branch(),
            self.head_commit()?,
            self.branches()?,
            self.tags()?,
        ))
    }

    /// Checks out a local branch, replacing worktree contents.
    pub fn checkout_branch(&self, name: &str) -> Result<()> {
        let full = format!("refs/heads/{name}");

        let branch = gix::refs::FullName::try_from(full.as_str())
            .map_err(|e| anyhow::anyhow!("invalid ref name: {e}"))?;

        let mut reference = self.inner.find_reference(&full)?;
        let tree = reference.peel_to_tree()?.id;

        let (signature, mut time_buf) = Self::repository_signature();
        let signature = signature.to_ref(&mut time_buf);

        self.move_head(
            signature,
            gix::refs::Target::Symbolic(branch),
            &format!("checkout: moving to {name}"),
        )?;

        self.force_checkout(&tree)?;

        Ok(())
    }

    /// Detaches HEAD onto a tag, replacing worktree contents.
    pub fn checkout_tag(&self, name: &str) -> Result<()> {
        let full = format!("refs/tags/{name}");

        let mut reference = self.inner.find_reference(&full)?;

        let commit = reference.peel_to_id()?;
        let tree = reference.peel_to_tree()?.id;

        let (signature, mut time_buf) = Self::repository_signature();
        let signature = signature.to_ref(&mut time_buf);

        self.move_head(
            signature,
            gix::refs::Target::Object(commit.detach()),
            &format!("checkout: moving to {name}"),
        )?;

        self.force_checkout(&tree)?;

        Ok(())
    }

    /// Writes `tree` over the worktree and removes files absent from it.
    pub(crate) fn force_checkout(&self, tree: &gix::hash::oid) -> Result<()> {
        let workdir = self
            .inner
            .workdir()
            .context("repository has no worktree")?
            .to_path_buf();

        let mut index = self.inner.index_from_tree(tree)?;

        if let Ok(previous) = self.inner.index_or_empty() {
            let keep: HashSet<PathBuf> = index
                .entries()
                .iter()
                .map(|entry| {
                    PathBuf::from(String::from_utf8_lossy(entry.path(&index)).into_owned())
                })
                .collect();
            for entry in previous.entries() {
                let rel = entry.path(&previous);
                let rel = PathBuf::from(String::from_utf8_lossy(rel).into_owned());

                if keep.contains(&rel) {
                    continue;
                }

                let path = workdir.join(&rel);

                match std::fs::remove_file(&path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => {
                        return Err(error)
                            .with_context(|| format!("failed to remove {}", path.display()));
                    }
                }
            }
        }

        let mut options = self
            .inner
            .checkout_options(gix_worktree::stack::state::attributes::Source::IdMapping)?;
        options.overwrite_existing = true;

        let objects = self.inner.objects.clone().into_arc()?;
        let files = gix::progress::Discard;
        let bytes = gix::progress::Discard;

        gix_worktree_state::checkout(
            &mut index,
            workdir,
            objects,
            &files,
            &bytes,
            &gix::interrupt::IS_INTERRUPTED,
            options,
        )
        .into_anyhow()?;

        index
            .write(gix::index::write::Options::default())
            .into_anyhow()?;

        Ok(())
    }

    /// Moves HEAD with a reflog entry.
    fn move_head(
        &self,
        signature: gix::actor::SignatureRef<'_>,
        target: gix::refs::Target,
        message: &str,
    ) -> Result<()> {
        use gix::refs::transaction::{Change, LogChange, PreviousValue, RefEdit, RefLog};

        let head = gix::refs::FullName::try_from("HEAD")
            .map_err(|e| anyhow::anyhow!("invalid ref name: {e}"))?;

        self.inner.edit_references_as(
            [RefEdit {
                change: Change::Update {
                    log: LogChange {
                        mode: RefLog::AndReference,
                        force_create_reflog: false,
                        message: message.into(),
                    },
                    expected: PreviousValue::Any,
                    new: target,
                },
                name: head,
                deref: false,
            }],
            Some(signature),
        )?;

        Ok(())
    }

    /// Recursively collects paths under `dir` relative to `root`, skipping `.git`.
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
                Self::collect_entries(root, &path, out)?;
            }
        }
        Ok(())
    }

    /// Applies an mbox patch series with `git am`.
    pub fn apply_patch(&self, patch: &str) -> Result<()> {
        let workdir = self
            .inner
            .workdir()
            .context("repository has no worktree")?
            .to_path_buf();

        let mut child = Self::git_command(&workdir)
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

    /// Formats `base..tip` as an mbox, failing when the range is empty.
    pub fn format_patch_between(&self, base: &str, tip: &str) -> Result<String> {
        let output = Self::run_git(
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

    /// Fetches `origin` including the `refs/nostr/*` PR refs.
    pub fn fetch(&self) -> Result<()> {
        let options = gix::remote::ref_map::Options {
            extra_refspecs: vec![
                gix::refspec::parse(
                    gix::bstr::BStr::new("+refs/nostr/*:refs/nostr/*"),
                    gix::refspec::parse::Operation::Fetch,
                )
                .into_anyhow()?
                .to_owned(),
            ],
            ..Default::default()
        };
        self.inner
            .find_remote("origin")?
            .connect(gix::remote::Direction::Fetch)?
            .prepare_fetch(Discard, options)?
            .receive(Discard, &IS_INTERRUPTED)?;
        Ok(())
    }

    /// Pushes `commit` to `reference` at `url`.
    pub fn push_ref(&self, url: &str, commit: &str, reference: &str) -> Result<()> {
        let output = Self::run_git(
            self.workdir_or_dot(),
            &["push", url, &format!("{commit}:{reference}")],
            "git push",
        )?;

        if !output.status.success() {
            bail!(
                "git push failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(())
    }

    /// Pushes `main` to the grasp repository at `base_url`.
    pub fn push_main(&self, base_url: &str, owner: &str, repo_id: &str) -> Result<()> {
        self.push_refspecs(
            base_url,
            owner,
            repo_id,
            &["refs/heads/main:refs/heads/main"],
        )
    }

    /// Mirrors all branches and tags to the grasp repository at `base_url`.
    pub fn push_all(&self, base_url: &str, owner: &str, repo_id: &str) -> Result<()> {
        self.push_refspecs(
            base_url,
            owner,
            repo_id,
            &["refs/heads/*:refs/heads/*", "refs/tags/*:refs/tags/*"],
        )
    }

    /// Pushes explicit refspecs to the grasp repository at `base_url`.
    fn push_refspecs(
        &self,
        base_url: &str,
        owner: &str,
        repo_id: &str,
        refspecs: &[&str],
    ) -> Result<()> {
        let url = format!("{base_url}/{owner}/{repo_id}.git");

        let mut args: Vec<&str> = Vec::with_capacity(refspecs.len() + 2);
        args.push("push");
        args.push(&url);
        args.extend_from_slice(refspecs);

        let output = Self::run_git(self.workdir_or_dot(), &args, "git push")?;

        if !output.status.success() {
            bail!(
                "git push to {base_url} failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(())
    }

    /// Checks that `url` advertises exactly the expected ref oids.
    pub fn remote_has_refs(&self, url: &str, expected: &[(String, String)]) -> Result<bool> {
        if expected.is_empty() {
            return Ok(true);
        }

        let url = Self::transport_url(url);

        let refspecs = expected
            .iter()
            .map(|(name, _)| {
                gix::refspec::parse(
                    gix::bstr::BStr::new(format!("+{name}:{name}").as_bytes()),
                    gix::refspec::parse::Operation::Fetch,
                )
                .map(|spec| spec.to_owned())
            })
            .collect::<Result<Vec<_>, _>>()
            .into_anyhow()
            .context("invalid refspec")?;

        let options = gix::remote::ref_map::Options {
            extra_refspecs: refspecs,
            ..Default::default()
        };

        let (refs, _) = self
            .inner
            .remote_at(url.as_str())
            .with_context(|| format!("cannot use remote {url}"))?
            .connect(gix::remote::Direction::Fetch)
            .with_context(|| format!("cannot connect to {url}"))?
            .ref_map(Discard, options)
            .with_context(|| format!("listing refs of {url} failed"))?;

        let advertised: HashMap<String, String> = refs
            .remote_refs
            .iter()
            .filter_map(|reference| {
                let (name, object, _peeled) = reference.unpack();
                object.map(|oid| (String::from_utf8_lossy(name).into_owned(), oid.to_string()))
            })
            .collect();

        Ok(expected
            .iter()
            .all(|(name, oid)| advertised.get(name.as_str()) == Some(oid)))
    }

    /// Sets the `origin` URL when the repository has no remote yet.
    pub fn ensure_origin(&self, url: &str) -> Result<()> {
        if self.inner.find_remote("origin").is_ok() {
            return Ok(());
        }

        self.edit_local_config(|config| {
            config
                .set_raw_value("remote.origin.url", url)
                .into_anyhow()?;
            config
                .set_raw_value("remote.origin.fetch", "+refs/heads/*:refs/remotes/origin/*")
                .into_anyhow()?;
            Ok(())
        })
    }

    /// Repoints `origin` at `url`, keeping any existing fetch refspec.
    pub fn set_origin(&self, url: &str) -> Result<()> {
        let had_origin = self.inner.find_remote("origin").is_ok();

        self.edit_local_config(|config| {
            config
                .set_raw_value("remote.origin.url", url)
                .into_anyhow()?;

            if !had_origin {
                config
                    .set_raw_value("remote.origin.fetch", "+refs/heads/*:refs/remotes/origin/*")
                    .into_anyhow()?;
            }

            Ok(())
        })
    }

    /// Returns the `origin` fetch URL, or `None` without an origin.
    pub fn origin_url(&self) -> Result<Option<String>> {
        let Ok(remote) = self.inner.find_remote("origin") else {
            return Ok(None);
        };

        Ok(remote
            .url(gix::remote::Direction::Fetch)
            .map(|url| url.to_string()))
    }

    /// Fetches `refspec` from the first working URL.
    pub fn fetch_refs<U: AsRef<str>>(&self, urls: &[U], refspec: &str) -> Result<()> {
        let refspec = gix::refspec::parse(
            gix::bstr::BStr::new(refspec),
            gix::refspec::parse::Operation::Fetch,
        )
        .into_anyhow()
        .context("invalid fetch refspec")?
        .to_owned();

        let mut last_error = None;

        for url in urls {
            let url = Self::transport_url(url.as_ref());
            let outcome = (|| -> Result<()> {
                let options = gix::remote::ref_map::Options {
                    extra_refspecs: vec![refspec.clone()],
                    ..Default::default()
                };
                self.inner
                    .remote_at(url.as_str())
                    .with_context(|| format!("fetch from {url} failed"))?
                    .connect(gix::remote::Direction::Fetch)
                    .with_context(|| format!("fetch from {url} failed"))?
                    .prepare_fetch(Discard, options)
                    .with_context(|| format!("fetch from {url} failed"))?
                    .receive(Discard, &IS_INTERRUPTED)
                    .with_context(|| format!("fetch from {url} failed"))?;
                Ok(())
            })();

            match outcome {
                Ok(()) => return Ok(()),
                Err(error) => last_error = Some(error),
            }
        }

        match last_error {
            Some(error) => Err(error).context("failed to fetch from any mirror"),
            None => bail!("no fetch URLs provided"),
        }
    }

    /// Edits the repository config under a lock and saves it.
    pub(crate) fn edit_local_config(
        &self,
        edit: impl FnOnce(&mut gix::config::File) -> Result<()>,
    ) -> Result<()> {
        let config_path = self.inner.common_dir().join("config");

        let mut lock = gix::lock::File::acquire_to_update_resource(
            &config_path,
            gix::lock::acquire::Fail::Immediately,
            None,
        )
        .into_anyhow()
        .context("failed to lock repository config")?;

        let mut config =
            match gix::config::File::from_path_no_includes(config_path, gix::config::Source::Local)
            {
                Ok(config) => config,
                Err(error) if error.is_not_found() => gix::config::File::default(),
                Err(error) => {
                    return Err(error)
                        .into_anyhow()
                        .context("failed to read repository config");
                }
            };

        edit(&mut config)?;

        config
            .write_to(&mut lock)
            .context("failed to write repository config")?;

        lock.commit().context("failed to save repository config")?;

        Ok(())
    }

    /// Runs `git` in `dir` and returns its output.
    pub(crate) fn run_git(dir: &Path, args: &[&str], what: &str) -> Result<std::process::Output> {
        Self::git_command(dir)
            .args(args)
            .stderr(Stdio::piped())
            .output()
            .with_context(|| format!("failed to spawn `{what}`"))
    }

    /// Builds a `git` command working in `dir` without terminal prompts.
    pub(crate) fn git_command(dir: &Path) -> std::process::Command {
        let mut command = std::process::Command::new("git");
        command.arg("-C").arg(dir).env("GIT_TERMINAL_PROMPT", "0");
        command
    }

    /// Detects the repository's NIP-34 binding, or `None` without Nostr markers.
    pub fn nip34_binding(&self) -> Option<Nip34Binding> {
        let common_dir = self.inner.common_dir().to_path_buf();
        let workdir = self.inner.workdir().map(std::path::Path::to_path_buf);

        let mut signals = GraspSignals::default();
        let mut owner: Option<PublicKey> = None;
        let mut identifier: Option<String> = None;
        let mut grasp_urls: Vec<String> = Vec::new();

        if let Some(workdir) = &workdir {
            if let Ok(bytes) = std::fs::read(workdir.join("nip34.json"))
                && let Ok(config) = serde_json::from_slice::<Nip34Json>(&bytes)
            {
                signals.nip34_json = true;
                identifier = config.identifier.and_then(Self::non_empty);
                owner = config
                    .owner
                    .as_deref()
                    .and_then(|value| PublicKey::parse(value).ok());
            }

            if workdir.join("maintainers.yaml").is_file() {
                signals.maintainers_yaml = true;
            }
        }

        if let Ok(exclude) = std::fs::read_to_string(common_dir.join("info/exclude"))
            && exclude.contains("nip34.json")
        {
            signals.nip34_excluded = true;
        }

        if common_dir.join("nostr-cache.lmdb").is_file() {
            signals.nostr_cache = true;
        }

        if let Ok(config) = gix::config::File::from_path_no_includes(
            common_dir.join("config"),
            gix::config::Source::Local,
        ) {
            if let Some(value) = config.string("nostr.repo")
                && let Some((key, id)) = Self::coordinate_from_naddr(&value.to_str_lossy())
            {
                signals.nostr_repo_config = true;
                owner = Some(key);
                identifier = Some(id);
            }

            for key in ["nostr.repo-relay-only", "nostr.nostate", "nostr.private"] {
                if config.string(key).is_some() {
                    signals.nostr_aux_config = true;
                }
            }

            if let Some(sections) = config.sections_by_name("remote") {
                for section in sections {
                    let Some(name) = section.header().subsection_name() else {
                        continue;
                    };
                    let nak_grasp_remote = name.to_str_lossy().starts_with("nip34/grasp/");

                    for url in section.values("url") {
                        let url = url.to_str_lossy();

                        if url.starts_with("nostr://") {
                            signals.nostr_remote = true;
                            if owner.is_none()
                                && identifier.is_none()
                                && let Some((key, id)) = Self::parse_nostr_url(&url)
                            {
                                owner = Some(key);
                                identifier = Some(id);
                            }
                        }

                        if Self::is_grasp_url(&url) {
                            signals.grasp_remote = true;
                            signals.nip34_grasp_remote |= nak_grasp_remote;
                            grasp_urls.push(url.to_string());

                            if owner.is_none()
                                && identifier.is_none()
                                && let Some((key, id)) = Self::grasp_parts(&url)
                            {
                                owner = Some(key);
                                identifier = Some(id);
                            }
                        }
                    }
                }
            }
        }

        if let Ok(platform) = self.inner.references()
            && let Ok(mut refs) = platform.prefixed(b"refs/heads/nip34/state/")
            && refs.next().is_some()
        {
            signals.nip34_state_refs = true;
        }

        if !signals.any() {
            return None;
        }

        let kind = if signals.nip34_json
            || signals.nostr_repo_config
            || signals.nip34_grasp_remote
            || signals.nip34_state_refs
        {
            Nip34Kind::Initialized
        } else if signals.nostr_remote {
            Nip34Kind::Cloned
        } else {
            Nip34Kind::ToolingOnly
        };

        Some(Nip34Binding::new(
            kind, signals, owner, identifier, grasp_urls,
        ))
    }

    /// Stores a NIP-34 coordinate in the `nostr.repo` config.
    pub fn set_nostr_repo(&self, naddr: &str) -> Result<()> {
        self.edit_local_config(|config| {
            config.set_raw_value("nostr.repo", naddr).into_anyhow()?;
            Ok(())
        })
    }

    /// Matches grasp URLs by shape: two path segments with an npub owner.
    pub(crate) fn is_grasp_url(url: &str) -> bool {
        let Ok(parsed) = Url::parse(url) else {
            return false;
        };

        if !matches!(parsed.scheme(), "http" | "https" | "grasp") {
            return false;
        }

        let path = parsed.path();
        if path.matches('/').count() != 2 || path.len() < 65 {
            return false;
        }

        Self::grasp_parts(url).is_some()
    }

    /// Splits a grasp URL into its owner and identifier.
    fn grasp_parts(url: &str) -> Option<(PublicKey, String)> {
        let parsed = Url::parse(url).ok()?;
        let mut segments = parsed.path_segments()?.filter(|part| !part.is_empty());

        let owner = PublicKey::parse(segments.next()?).ok()?;
        let identifier = Self::non_empty(segments.next()?.trim_end_matches(".git"))?;

        Some((owner, identifier))
    }

    /// Decodes an `naddr` coordinate of a git repo announcement.
    fn coordinate_from_naddr(value: &str) -> Option<(PublicKey, String)> {
        let coordinate = Nip19Coordinate::from_bech32(value).ok()?;
        if coordinate.kind != Kind::GitRepoAnnouncement {
            return None;
        }

        let identifier = Self::non_empty(coordinate.identifier.clone())?;
        Some((coordinate.public_key, identifier))
    }

    /// Extracts owner and identifier from a `nostr://` URL.
    fn parse_nostr_url(url: &str) -> Option<(PublicKey, String)> {
        let rest = url.strip_prefix("nostr://")?;

        if rest.starts_with("naddr1") {
            return Self::coordinate_from_naddr(rest);
        }

        let rest = rest.rsplit_once('@').map_or(rest, |(_, after)| after);
        let mut parts: Vec<&str> = rest.split('/').filter(|part| !part.is_empty()).collect();

        if parts
            .first()
            .is_some_and(|first| matches!(*first, "ssh" | "https" | "http"))
        {
            parts.remove(0);
        }

        if parts.len() < 2 {
            return None;
        }

        let owner = PublicKey::parse(parts[0]).ok()?;
        let identifier = Self::non_empty(parts.last()?.trim_end_matches(".git"))?;

        Some((owner, identifier))
    }

    /// Returns `value` unless it is empty.
    fn non_empty(value: impl Into<String>) -> Option<String> {
        let value = value.into();
        (!value.is_empty()).then_some(value)
    }

    /// Returns the fixed author signature used for repository writes.
    pub(crate) fn repository_signature() -> (gix::actor::Signature, gix::date::parse::TimeBuf) {
        let seconds = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_secs() as i64)
            .unwrap_or_default();

        let signature = gix::actor::Signature {
            name: gix::bstr::BString::from("Signed"),
            email: gix::bstr::BString::from("signed@localhost"),
            time: gix::date::Time { seconds, offset: 0 },
        };

        (signature, gix::date::parse::TimeBuf::default())
    }

    /// Rewrites `grasp://` URLs to `https://`, leaving others untouched.
    pub(crate) fn transport_url(url: &str) -> String {
        url.strip_prefix("grasp://")
            .map(|rest| format!("https://{rest}"))
            .unwrap_or_else(|| url.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::process::Command;

    use super::*;

    /// Creates a fresh repository in a temp directory.
    fn init_repo() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("repo");
        std::fs::create_dir_all(&path).expect("mkdir");
        git(&path, &["init", "-q"]);
        (dir, path)
    }

    /// Runs `git` in `dir`, failing the test on a non-zero exit.
    fn git(dir: &Path, args: &[&str]) {
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

    /// Generates a fresh public key.
    fn key() -> PublicKey {
        Keys::generate().public_key()
    }

    /// Encodes a NIP-34 coordinate as an `naddr` string.
    fn naddr(kind: Kind, owner: PublicKey, identifier: &str) -> String {
        let coordinate = Coordinate::new(kind, owner).identifier(identifier);
        Nip19Coordinate::new(coordinate, Vec::<RelayUrl>::new())
            .to_bech32()
            .expect("naddr")
    }

    /// Opens the repository at `path` and returns its NIP-34 binding.
    fn binding_of(path: &Path) -> Option<Nip34Binding> {
        Repo::open(path).ok()?.nip34_binding()
    }

    #[test]
    fn plain_repository_has_no_binding() {
        let (_dir, path) = init_repo();
        assert!(binding_of(&path).is_none());
    }

    #[test]
    fn nip34_json_marks_a_repository_initialized() {
        let (_dir, path) = init_repo();
        let owner = key();
        let npub = owner.to_bech32().expect("npub");
        std::fs::write(
            path.join("nip34.json"),
            format!(r#"{{"identifier":"my-repo","owner":"{npub}"}}"#),
        )
        .expect("write");

        let binding = binding_of(&path).expect("binding");
        assert_eq!(binding.kind, Nip34Kind::Initialized);
        assert!(binding.signals.nip34_json);
        assert_eq!(binding.owner, Some(owner));
        assert_eq!(binding.identifier.as_deref(), Some("my-repo"));
    }

    #[test]
    fn malformed_nip34_json_is_ignored() {
        let (_dir, path) = init_repo();
        std::fs::write(path.join("nip34.json"), b"not json").expect("write");

        assert!(binding_of(&path).is_none());
    }

    #[test]
    fn nak_exclude_and_state_refs_are_detected() {
        let (_dir, path) = init_repo();

        std::fs::create_dir_all(path.join(".git/info")).expect("mkdir");
        std::fs::write(path.join(".git/info/exclude"), "nip34.json\n").expect("write");

        git(&path, &["commit", "-q", "--allow-empty", "-m", "initial"]);
        git(
            &path,
            &["update-ref", "refs/heads/nip34/state/HEAD", "HEAD"],
        );

        let binding = binding_of(&path).expect("binding");
        assert_eq!(binding.kind, Nip34Kind::Initialized);
        assert!(binding.signals.nip34_excluded);
        assert!(binding.signals.nip34_state_refs);
    }

    #[test]
    fn nostr_repo_config_marks_a_repository_initialized() {
        let (_dir, path) = init_repo();
        let owner = key();
        let naddr = naddr(Kind::GitRepoAnnouncement, owner, "my-repo");
        git(&path, &["config", "nostr.repo", &naddr]);

        let binding = binding_of(&path).expect("binding");
        assert_eq!(binding.kind, Nip34Kind::Initialized);
        assert!(binding.signals.nostr_repo_config);
        assert_eq!(binding.owner, Some(owner));
        assert_eq!(binding.identifier.as_deref(), Some("my-repo"));
    }

    #[test]
    fn the_written_nostr_repo_marker_is_detected() {
        let (_dir, path) = init_repo();
        let owner = key();
        let naddr = naddr(Kind::GitRepoAnnouncement, owner, "my-repo");

        Repo::open(&path)
            .expect("open")
            .set_nostr_repo(&naddr)
            .expect("write marker");

        let binding = binding_of(&path).expect("binding");
        assert_eq!(binding.kind, Nip34Kind::Initialized);
        assert!(binding.signals.nostr_repo_config);
        assert_eq!(binding.owner, Some(owner));
        assert_eq!(binding.identifier.as_deref(), Some("my-repo"));
    }

    #[test]
    fn nostr_remote_is_a_nip34_clone() {
        let (_dir, path) = init_repo();
        let owner = key();
        let npub = owner.to_bech32().expect("npub");
        let url = format!("nostr://{npub}/relay.ngit.dev/my-repo");
        git(&path, &["remote", "add", "origin", &url]);

        let binding = binding_of(&path).expect("binding");
        assert_eq!(binding.kind, Nip34Kind::Cloned);
        assert!(binding.signals.nostr_remote);
        assert_eq!(binding.owner, Some(owner));
        assert_eq!(binding.identifier.as_deref(), Some("my-repo"));
    }

    #[test]
    fn nak_grasp_remote_marks_a_repository_initialized() {
        let (_dir, path) = init_repo();
        let owner = key();
        let npub = owner.to_bech32().expect("npub");
        let url = format!("https://gitnostr.com/{npub}/my-repo.git");
        git(
            &path,
            &["config", "remote.nip34/grasp/gitnostr.com.url", &url],
        );

        let binding = binding_of(&path).expect("binding");
        assert_eq!(binding.kind, Nip34Kind::Initialized);
        assert!(binding.signals.nip34_grasp_remote);
        assert!(binding.signals.grasp_remote);
        assert_eq!(binding.grasp_urls, vec![url]);
        assert_eq!(binding.owner, Some(owner));
        assert_eq!(binding.identifier.as_deref(), Some("my-repo"));
    }

    #[test]
    fn nostr_cache_alone_is_tooling_only() {
        let (_dir, path) = init_repo();
        std::fs::write(path.join(".git/nostr-cache.lmdb"), b"cache").expect("write");

        let binding = binding_of(&path).expect("binding");
        assert_eq!(binding.kind, Nip34Kind::ToolingOnly);
        assert!(binding.signals.nostr_cache);
    }

    #[test]
    fn grasp_urls_are_recognised_by_shape() {
        let owner = key();
        let npub = owner.to_bech32().expect("npub");

        assert!(Repo::is_grasp_url(&format!(
            "https://gitnostr.com/{npub}/my-repo.git"
        )));
        assert!(Repo::is_grasp_url(&format!(
            "grasp://gitnostr.com/{npub}/my-repo.git"
        )));

        assert!(!Repo::is_grasp_url("https://gitnostr.com/my-repo.git"));
        assert!(!Repo::is_grasp_url(
            "https://gitnostr.com/not-a-pubkey/my-repo.git"
        ));
        assert!(!Repo::is_grasp_url(&format!(
            "ssh://gitnostr.com/{npub}/my-repo.git"
        )));
    }
}

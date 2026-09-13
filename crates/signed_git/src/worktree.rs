use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use gix::progress::Discard;

use crate::history::{FileCommit, head_commit};
use crate::repo::{current_branch, repository_signature};

/// Whether the worktree of `workdir` has uncommitted changes.
///
/// Best-effort: any read failure is reported as clean.
pub fn worktree_dirty(workdir: &Path) -> bool {
    let Ok(repo) = gix::open(workdir) else {
        return false;
    };

    // Changes to tracked files, staged or not; untracked files are excluded.
    match repo.is_dirty() {
        Ok(true) => return true,
        Ok(false) => {}
        Err(_) => return false,
    }

    // Untracked files surface as `DirectoryContents` items of the index-vs-worktree walk,
    // tracked files only appear there when modified.
    let Ok(platform) = repo.status(Discard) else {
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

/// Commits in `base..branch` of the checkout at `workdir`.
///
/// Best-effort: 0 when the range cannot be computed.
pub fn worktree_commits_ahead(workdir: &Path, base: &str, branch: &str) -> u32 {
    let Ok(repo) = gix::open(workdir) else {
        return 0;
    };

    let (Some(base), Some(branch)) = (resolve_commit(&repo, base), resolve_commit(&repo, branch))
    else {
        return 0;
    };

    let Ok(walk) = repo.rev_walk([branch]).with_hidden([base]).all() else {
        return 0;
    };

    walk.filter_map(Result::ok).count().min(u32::MAX as usize) as u32
}

/// Resolve `rev` to a commit id, accepting full refs or the bare branch names
/// callers pass. `gix`'s revision parser already applies git's ref DWIM.
fn resolve_commit<'a>(repo: &'a gix::Repository, rev: &str) -> Option<gix::Id<'a>> {
    repo.rev_parse_single(rev.as_bytes()).ok()
}

/// Relative paths of all entries in the worktree, files and directories.
///
/// The `.git` directory is skipped.
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

/// Read a file from the worktree.
///
/// Returns `Ok(None)` if the path is missing or not a regular file.
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

/// Find the README file in the repository root.
///
/// Falls back to any other file whose name starts with `readme`.
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

/// Everything the browser needs to refresh after a branch or tag switch.
pub struct WorktreeSnapshot {
    /// Relative paths of all worktree entries, directories first.
    pub entries: Vec<PathBuf>,
    /// README path relative to the worktree, if any.
    pub readme_path: Option<PathBuf>,
    /// Contents of the README, if any.
    pub readme: Option<Vec<u8>>,
    /// Branch HEAD points to, `None` when detached, for example on a tag.
    pub current_branch: Option<String>,
    /// Commit HEAD points to, if any, see [`head_commit`].
    pub head_commit: Option<FileCommit>,
}

/// Collects entries, the README, the branch HEAD points to and its commit.
pub fn worktree_snapshot(workdir: &Path) -> Result<WorktreeSnapshot> {
    let repo = gix::open(workdir)?;
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

pub(crate) fn force_checkout(repo: &gix::Repository, tree: &gix::hash::oid) -> Result<()> {
    let workdir = repo
        .workdir()
        .context("repository has no worktree")?
        .to_path_buf();

    let mut index = repo.index_from_tree(tree)?;

    // Files the previous index tracked but `tree` no longer contains are removed,
    // like git deleting files that vanish between branches.
    if let Ok(previous) = repo.index_or_empty() {
        let keep: HashSet<PathBuf> = index
            .entries()
            .iter()
            .map(|entry| PathBuf::from(String::from_utf8_lossy(entry.path(&index)).into_owned()))
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

    let mut options =
        repo.checkout_options(gix_worktree::stack::state::attributes::Source::IdMapping)?;
    options.overwrite_existing = true;

    let objects = repo.objects.clone().into_arc()?;
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
    )?;

    index.write(gix::index::write::Options::default())?;

    Ok(())
}

/// Point `HEAD` at `target` and record the switch in the reflog.
fn move_head(
    repo: &gix::Repository,
    signature: gix::actor::SignatureRef<'_>,
    target: gix::refs::Target,
    message: &str,
) -> Result<()> {
    use gix::refs::transaction::{Change, LogChange, PreviousValue, RefEdit, RefLog};

    let head = gix::refs::FullName::try_from("HEAD")
        .map_err(|e| anyhow::anyhow!("invalid ref name: {e}"))?;

    repo.edit_references_as(
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

/// Check out the local branch `name`, HEAD stays attached to it.
pub fn worktree_checkout_branch(workdir: &Path, name: &str) -> Result<()> {
    let repo = gix::open(workdir)?;
    let full = format!("refs/heads/{name}");

    let branch = gix::refs::FullName::try_from(full.as_str())
        .map_err(|e| anyhow::anyhow!("invalid ref name: {e}"))?;

    let mut reference = repo.find_reference(&full)?;
    let tree = reference.peel_to_tree()?.id;

    let (signature, mut time_buf) = repository_signature();
    let signature = signature.to_ref(&mut time_buf);

    move_head(
        &repo,
        signature,
        gix::refs::Target::Symbolic(branch),
        &format!("checkout: moving to {name}"),
    )?;

    force_checkout(&repo, &tree)?;

    Ok(())
}

/// Check out the tag `name`, HEAD becomes detached at the tagged commit.
pub fn worktree_checkout_tag(workdir: &Path, name: &str) -> Result<()> {
    let repo = gix::open(workdir)?;
    let full = format!("refs/tags/{name}");

    let mut reference = repo.find_reference(&full)?;

    let commit = reference.peel_to_id()?;
    let tree = reference.peel_to_tree()?.id;

    let (signature, mut time_buf) = repository_signature();
    let signature = signature.to_ref(&mut time_buf);

    move_head(
        &repo,
        signature,
        gix::refs::Target::Object(commit.detach()),
        &format!("checkout: moving to {name}"),
    )?;

    force_checkout(&repo, &tree)?;

    Ok(())
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

use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::history::open_with_cache;
use crate::worktree::{force_checkout, worktree_dirty};

/// The merge base of two revisions in the repository at `repo_path`,
/// revisions may be branch names, remote-tracking refs or commit ids.
///
/// `Ok(None)` when the revisions share no common ancestor.
///
/// Unresolvable revisions are errors.
pub fn merge_base(repo_path: &Path, a: &str, b: &str) -> Result<Option<String>> {
    let repo = open_with_cache(repo_path)?;
    let a = repo.rev_parse_single(a.as_bytes())?;
    let b = repo.rev_parse_single(b.as_bytes())?;
    match repo.merge_base(a, b) {
        Ok(id) => Ok(Some(id.to_string())),
        // No common ancestor, a valid outcome for a proposal.
        Err(gix::repository::merge_base::Error::NotFound { .. }) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// The commit HEAD points to in the repository at `repo_path`.
///
/// `None` when the repository has no commits yet, an unborn HEAD.
pub fn head_commit_id(repo_path: &Path) -> Result<Option<String>> {
    let Ok(repo) = gix::open(repo_path) else {
        return Ok(None);
    };

    match repo.head_id() {
        Ok(id) => Ok(Some(id.to_string())),
        Err(_) => Ok(None),
    }
}

/// The commits in `base..HEAD` of the repository at `repo_path`, oldest first.
/// This is the order `git am` creates them.
///
/// `HEAD` alone when `base` is `None`.
pub fn commits_since(repo_path: &Path, base: Option<&str>) -> Result<Vec<String>> {
    let repo = match gix::open(repo_path) {
        Ok(repo) => repo,
        Err(_) if base.is_none() => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };

    let head = match repo.head_id() {
        Ok(head) => head,
        Err(_) if base.is_none() => return Ok(Vec::new()),
        Err(e) => return Err(e).context("repository has no commits"),
    };

    let Some(base) = base else {
        // `HEAD` alone when no base is given.
        return Ok(vec![head.to_string()]);
    };

    let base = repo.rev_parse_single(base.as_bytes())?;
    let mut commits = Vec::new();

    for info in repo
        .rev_walk([head])
        .sorting(gix::revision::walk::Sorting::ByCommitTime(
            gix::traverse::commit::simple::CommitTimeOrder::NewestFirst,
        ))
        .with_hidden([base])
        .all()?
    {
        commits.push(info?.id().to_string());
    }

    // Oldest first, like `git rev-list --reverse`, the order `git am` creates them.
    commits.reverse();

    Ok(commits)
}

/// The identity written to reflogs and commits created by this crate itself.
///
/// Like `git -c user.name=… -c user.email=…` per invocation: the repository works
/// without a global git identity, and `gix` runs no hooks and never signs.
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

/// Create a repository at `path` with an initial `main` branch.
/// Write a `README.md` from `name` and `description`, then create the initial commit.
///
/// Returns the initial commit id.
pub fn init_repository(path: &Path, name: &str, description: &str) -> Result<String> {
    use gix::refs::transaction::{Change, LogChange, PreviousValue, RefEdit, RefLog};

    std::fs::create_dir_all(path)
        .with_context(|| format!("failed to create {}", path.display()))?;

    let repo = gix::init(path)?;

    let (signature, mut time_buf) = repository_signature();
    let signature = signature.to_ref(&mut time_buf);

    // The initial branch is `main`, regardless of `init.defaultBranch` in
    // the user's git configuration: point the unborn HEAD there.
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

    // Populate the index so the fresh repository is clean,
    // as `git add` and`git commit` would leave it.
    let mut index = repo.index_from_tree(&tree)?;
    index.write(gix::index::write::Options::default())?;

    let commit = commit.to_string();
    if commit.len() != 40 {
        bail!("unexpected initial commit id: {commit}");
    }

    Ok(commit)
}

/// The earliest unique commit of the repository at `repo_path`.
/// Used as the NIP-34 announcement's `euc` marker.
///
/// `None` for a repository without commits.
pub fn root_commit(repo_path: &Path) -> Result<Option<String>> {
    let Ok(repo) = gix::open(repo_path) else {
        return Ok(None);
    };

    let Ok(head) = repo.head_id() else {
        // An unborn HEAD with no commits yet has no root commit.
        return Ok(None);
    };

    for info in repo
        .rev_walk([head])
        .sorting(gix::revision::walk::Sorting::ByCommitTime(
            gix::traverse::commit::simple::CommitTimeOrder::NewestFirst,
        ))
        .all()?
    {
        let info = info?;
        if info.parent_ids().next().is_none() {
            let id = info.id().to_string();
            return Ok((id.len() == 40).then_some(id));
        }
    }

    Ok(None)
}

/// Full ref names under `prefix`, sorted lexicographically, like `git for-each-ref`.
/// `prefix` is a ref namespace like `refs/fork/<owner>/<id>`.
///
/// Returns an empty list when nothing matches.
pub fn refs_with_prefix(repo_path: &Path, prefix: &str) -> Result<Vec<String>> {
    let pattern = prefix.trim_end_matches('/');
    let repo = gix::open(repo_path)?;
    let mut names = Vec::new();

    for reference in repo.references()?.all()? {
        let reference = reference.map_err(|error| anyhow::anyhow!("{error}"))?;
        let name = String::from_utf8_lossy(reference.name().as_bstr()).into_owned();

        // Match the pattern itself and everything beneath it, like `git for-each-ref`.
        let under_pattern = name
            .strip_prefix(pattern)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'));

        if under_pattern {
            names.push(name);
        }
    }

    // Sort lexicographically, like `git for-each-ref`.
    names.sort();

    Ok(names)
}

/// Delete every ref under `prefix` of the repository at `repo_path`.
/// `prefix` is a ref namespace like `refs/fork/<owner>/<id>`.
pub fn delete_refs_with_prefix(repo_path: &Path, prefix: &str) -> Result<()> {
    use gix::refs::transaction::{Change, PreviousValue, RefEdit, RefLog};

    let refs = refs_with_prefix(repo_path, prefix)?;
    if refs.is_empty() {
        return Ok(());
    }

    let repo = gix::open(repo_path)?;
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

    // Delete all refs with the given prefix.
    repo.edit_references(edits)?;

    Ok(())
}

/// Short name of the branch HEAD points to at `workdir`,
/// `None` when detached or unreadable, like `git branch --show-current`.
pub fn worktree_current_branch(workdir: &Path) -> Option<String> {
    let repo = gix::open(workdir).ok()?;
    let head = repo.head().ok()?;
    let name = head.referent_name()?;
    Some(String::from_utf8_lossy(name.shorten()).into_owned())
}

/// Whether the reference `name` exists in the repository at `workdir`.
pub fn worktree_ref_exists(workdir: &Path, name: &str) -> bool {
    let Ok(repo) = gix::open(workdir) else {
        return false;
    };
    repo.find_reference(name).is_ok()
}

/// Fast-forward local branches that trail their remote-tracking counterpart.
///
/// Returns whether any branch moved.
pub fn fast_forward_branches(workdir: &Path) -> Result<bool> {
    let repo = gix::open(workdir)?;
    let current = worktree_current_branch(workdir);
    let heads = refs_with_prefix(workdir, "refs/heads")?;

    let (signature, mut time_buf) = repository_signature();
    let signature = signature.to_ref(&mut time_buf);

    let mut moved = false;

    for head in heads {
        let Some(branch) = head.strip_prefix("refs/heads/") else {
            continue;
        };

        let remote = format!("refs/remotes/origin/{branch}");
        // No remote-tracking counterpart means the remote lacks this branch.
        let Ok(mut remote_reference) = repo.find_reference(&remote) else {
            continue;
        };

        let Ok(mut local_reference) = repo.find_reference(&head) else {
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

        // Only fast-forward.
        // Local-only commits or diverged history must never be rewritten by a refresh.
        let Ok(base) = repo.merge_base(local_oid, remote_oid) else {
            continue;
        };

        if base != local_oid {
            continue;
        }

        let full = gix::refs::FullName::try_from(head.as_str())
            .map_err(|e| anyhow::anyhow!("invalid ref name: {e}"))?;

        let edit = |new: gix::refs::Target| {
            use gix::refs::transaction::{Change, LogChange, PreviousValue, RefEdit, RefLog};
            RefEdit {
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
            }
        };

        if current.as_deref() == Some(branch) {
            // Merge so the checked-out worktree follows the branch.
            // Only proceed on a clean worktree, like `git merge --ff-only`.
            if worktree_dirty(workdir) {
                continue;
            }

            let tree = repo.find_object(remote_oid)?.peel_to_tree()?.id;

            // Check out the remote tree, discarding local changes.
            force_checkout(&repo, &tree)?;

            // Update the branch reference to point to the remote tree.
            repo.edit_references_as(
                [edit(gix::refs::Target::Object(remote_oid))],
                Some(signature),
            )?;

            moved = true;
        } else {
            // Update the branch reference to point to the remote tree.
            repo.edit_references_as(
                [edit(gix::refs::Target::Object(remote_oid))],
                Some(signature),
            )?;

            moved = true;
        }
    }

    Ok(moved)
}

/// Short names of local branches, `refs/heads/*`, of `repo`, sorted alphabetically.
pub fn repo_branches(repo: &gix::Repository) -> Result<Vec<String>> {
    let mut names = Vec::new();
    for reference in repo.references()?.local_branches()? {
        let reference = reference.map_err(|error| anyhow::anyhow!("{error}"))?;
        names.push(String::from_utf8_lossy(reference.name().shorten()).into_owned());
    }
    names.sort();
    Ok(names)
}

/// Short names of tags, `refs/tags/*`, of `repo`, sorted alphabetically.
pub fn repo_tags(repo: &gix::Repository) -> Result<Vec<String>> {
    let mut names = Vec::new();
    for reference in repo.references()?.tags()? {
        let reference = reference.map_err(|error| anyhow::anyhow!("{error}"))?;
        names.push(String::from_utf8_lossy(reference.name().shorten()).into_owned());
    }
    names.sort();
    Ok(names)
}

/// Short names of local branches, `refs/heads/*`, sorted alphabetically.
pub fn worktree_branches(workdir: &Path) -> Result<Vec<String>> {
    repo_branches(&gix::open(workdir)?)
}

/// Short name of the branch HEAD points to, or `None` when detached.
///
/// Detached after checking out a tag or a commit directly.
pub fn current_branch(repo: &gix::Repository) -> Result<Option<String>> {
    let head = repo.head()?;
    let Some(name) = head.referent_name() else {
        return Ok(None);
    };
    Ok(Some(String::from_utf8_lossy(name.shorten()).into_owned()))
}

/// Branch, tag and HEAD refs of a repository.
///
/// Ready for a NIP-34 kind-30618 repository state announcement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoRefState {
    /// `(full refname, commit id)` pairs for heads and tags, sorted.
    pub refs: Vec<(String, String)>,
    /// Short branch name HEAD points to, or `None` when detached.
    pub head: Option<String>,
}

/// Collect the refs of `repo`.
///
/// Local branches and tags become `(refname, commit-id)` pairs.
/// Also reports the branch HEAD points to.
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
    repo_ref_state(&gix::open(workdir)?)
}

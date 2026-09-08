use std::collections::{HashMap, HashSet};
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

    /// The root directory holding the mirror clones.
    pub fn root(&self) -> &Path {
        &self.root
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

    /// Open the existing clone, fetching it first.
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

/// Maximum directory nesting depth when scanning for local repositories.
///
/// Pathological trees can't stall the scan.
const SCAN_MAX_DEPTH: usize = 12;

/// Directories never descended into during a scan.
///
/// Dependency caches can be enormous without ever containing user repositories.
const SCAN_SKIPPED_DIR: &str = "node_modules";

/// Walk `root` recursively and collect the paths of git repositories below it.
pub fn find_git_repos(root: &Path) -> Vec<PathBuf> {
    let mut repos = Vec::new();
    if !root.is_dir() {
        return repos;
    }

    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        if depth > SCAN_MAX_DEPTH {
            continue;
        }
        // A directory containing a `.git` entry is a repository.
        // A linked worktree has a `.git` file instead of a directory.
        // Don't descend into repositories.
        if dir.join(".git").exists() {
            if let Ok(path) = dir.canonicalize() {
                repos.push(path);
            }
            continue;
        }

        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if !file_type.is_dir() || file_type.is_symlink() {
                continue;
            }
            let file_name = entry.file_name();
            let name = file_name.to_string_lossy();
            if name.starts_with('.') || name == SCAN_SKIPPED_DIR {
                continue;
            }
            stack.push((entry.path(), depth + 1));
        }
    }

    repos.sort();
    repos.dedup();
    repos
}

/// Clone into `path` from the first working URL in `clone_urls`.
///
/// Unlike [`GitCache::ensure_clone`], the clone is not kept in any cache.
pub fn clone_repo(clone_urls: &[String], path: &Path) -> Result<()> {
    if path.exists() {
        bail!("destination {} already exists", path.display());
    }

    try_each_url(clone_urls, "clone", |url| {
        let repo = clone(url, path)?;
        // The initial clone uses the default refspecs.
        // Also fetch the `refs/nostr/*` PR refs.
        fetch_all(&repo).ok();
        Ok(())
    })
}

/// Fetch all configured refspecs from `origin`, plus the `refs/nostr/*` namespace.
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

/// Apply a `git format-patch` patch or series with `git am`,
/// uses the git CLI because it handles the mbox format natively.
///
/// TODO: Replaced with a pure-Rust implementation later without changing callers.
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

/// Push `commit` to `reference` on the server at `url`, from `repo_path`.
pub fn push_commit_ref(repo_path: &Path, url: &str, commit: &str, reference: &str) -> Result<()> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .args(["push"])
        .arg(url)
        .arg(format!("{commit}:{reference}"))
        .env("GIT_TERMINAL_PROMPT", "0")
        .stderr(Stdio::piped())
        .output()
        .context("failed to spawn `git push`")?;

    if !output.status.success() {
        bail!(
            "git push failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
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

/// Rewrite a grasp server URL to the https URL the git transport actually uses.
///
/// GRASP servers announce `grasp://<host>/<owner>/<repo>` clone URLs.
/// The transport is git smart HTTP, so the scheme is rewritten for gix.
fn transport_url(url: &str) -> String {
    url.strip_prefix("grasp://")
        .map(|rest| format!("https://{rest}"))
        .unwrap_or_else(|| url.to_owned())
}

/// Run `attempt` against each URL in `urls` until one succeeds.
///
/// Returns the last error wrapped in `failed to {verb} from any mirror`,
/// or `no clone URLs provided` when the list is empty.
fn try_each_url<F>(urls: &[String], verb: &str, mut attempt: F) -> Result<()>
where
    F: FnMut(&str) -> Result<()>,
{
    let mut last_err: Option<anyhow::Error> = None;

    for url in urls {
        match attempt(url) {
            Ok(()) => return Ok(()),
            Err(e) => last_err = Some(e),
        }
    }

    match last_err {
        Some(e) => Err(e).context(format!("failed to {verb} from any mirror")),
        None => bail!("no clone URLs provided"),
    }
}

fn clone(url: &str, path: &Path) -> Result<gix::Repository> {
    let url = transport_url(url);
    let url = gix::url::parse(url).context("invalid clone URL")?;

    let mut prepare = gix::prepare_clone(url, path)?;
    let (mut checkout, _fetch) = prepare.fetch_then_checkout(Discard, &IS_INTERRUPTED)?;
    let (repo, _checkout) = checkout.main_worktree(Discard, &IS_INTERRUPTED)?;

    Ok(repo)
}

/// The identity written to reflogs and commits created by this crate itself.
///
/// Like `git -c user.name=… -c user.email=…` per invocation: the repository works
/// without a global git identity, and `gix` runs no hooks and never signs.
fn repository_signature() -> (gix::actor::Signature, gix::date::parse::TimeBuf) {
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

/// Push the `main` branch of the repository at `repo_path` to a grasp server.
pub fn push_main(repo_path: &Path, base_url: &str, owner: &str, repo_id: &str) -> Result<()> {
    push_refspecs(
        repo_path,
        base_url,
        owner,
        repo_id,
        &["refs/heads/main:refs/heads/main"],
    )
}

/// Push every local branch and tag of the repository at `repo_path` to a grasp server.
///
/// This mirrors an initialized repository's whole history.
pub fn push_all(repo_path: &Path, base_url: &str, owner: &str, repo_id: &str) -> Result<()> {
    push_refspecs(
        repo_path,
        base_url,
        owner,
        repo_id,
        &["refs/heads/*:refs/heads/*", "refs/tags/*:refs/tags/*"],
    )
}

/// Push `refspecs` to the grasp server URL derived from `base_url`, `owner` and `repo_id`.
fn push_refspecs(
    repo_path: &Path,
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

    let output = git_output(repo_path, &args, "git push")?;

    if !output.status.success() {
        bail!(
            "git push to {base_url} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

/// Whether `url` advertises every ref in `expected` at the given commit.
///
/// Extra advertised refs are ignored: the question is whether the data this
/// push wanted to land is already there, not whether the remote is an exact mirror.
/// This is the convergence probe for a push that lost the compare-and-swap race
/// to the grasp server's own background ref alignment.
pub fn remote_has_refs(repo_path: &Path, url: &str, expected: &[(String, String)]) -> Result<bool> {
    if expected.is_empty() {
        return Ok(true);
    }

    let repo = gix::open(repo_path)?;
    let url = transport_url(url);

    // A URL-created remote has no configured fetch refspecs, and `ref_map` only
    // keeps refs that match one. Match each expected ref by its exact name, like
    // `git ls-remote <url> <name>` would; ref maps never write to the repository.
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
        .context("invalid refspec")?;

    let options = gix::remote::ref_map::Options {
        extra_refspecs: refspecs,
        ..Default::default()
    };

    let (refs, _) = repo
        .remote_at(url.as_str())
        .with_context(|| format!("cannot use remote {url}"))?
        .connect(gix::remote::Direction::Fetch)
        .with_context(|| format!("cannot connect to {url}"))?
        .ref_map(Discard, options)
        .with_context(|| format!("listing refs of {url} failed"))?;

    // Peeled tag entries carry the tag object in their direct oid, so mapping
    // each advertised ref to its direct oid matches `git ls-remote` while
    // skipping the duplicated `^{}` lines.
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

/// Add `origin` pointing at `url` when the repository has no remote yet.
///
/// No-op if `origin` already exists.
pub fn ensure_origin(repo_path: &Path, url: &str) -> Result<()> {
    let repo = gix::open(repo_path)?;
    if repo.find_remote("origin").is_ok() {
        return Ok(());
    }

    // `git remote add` also configures the default fetch refspec.
    edit_local_config(&repo, |config| {
        config.set_raw_value("remote.origin.url", url)?;
        config.set_raw_value("remote.origin.fetch", "+refs/heads/*:refs/remotes/origin/*")?;
        Ok(())
    })
}

/// Point `origin` at `url`, replacing an existing remote,
/// used after a clone whose `origin` points at the cloned-from path.
///
/// A working copy cloned from a local mirror is re-targeted at the grasp server.
pub fn set_origin(repo_path: &Path, url: &str) -> Result<()> {
    let repo = gix::open(repo_path)?;
    let had_origin = repo.find_remote("origin").is_ok();

    edit_local_config(&repo, |config| {
        // Replaces the existing url, like `git remote set-url origin <url>`.
        // A pre-existing fetch refspec is left untouched.
        config.set_raw_value("remote.origin.url", url)?;

        if !had_origin {
            config.set_raw_value("remote.origin.fetch", "+refs/heads/*:refs/remotes/origin/*")?;
        }

        Ok(())
    })
}

/// Apply `edit` to the repository-local configuration and persist it.
///
/// The config file is locked while it is read, edited and written back,
/// like git would when running `git config` or `git remote`.
fn edit_local_config(
    repo: &gix::Repository,
    edit: impl FnOnce(&mut gix::config::File) -> Result<()>,
) -> Result<()> {
    let config_path = repo.common_dir().join("config");

    let mut lock = gix::lock::File::acquire_to_update_resource(
        &config_path,
        gix::lock::acquire::Fail::Immediately,
        None,
    )
    .context("failed to lock repository config")?;

    let mut config =
        match gix::config::File::from_path_no_includes(config_path, gix::config::Source::Local) {
            Ok(config) => config,
            // A repository without a config file yet starts from scratch.
            Err(gix::config::file::init::from_paths::Error::Io { source, .. })
                if source.kind() == std::io::ErrorKind::NotFound =>
            {
                gix::config::File::default()
            }
            Err(error) => return Err(error).context("failed to read repository config"),
        };

    edit(&mut config)?;

    config
        .write_to(&mut lock)
        .context("failed to write repository config")?;

    lock.commit().context("failed to save repository config")?;

    Ok(())
}

/// Fetch `refspec` into `repo_path` from the first working URL in `urls`.
/// When no URL works, the last error is returned.
///
/// Never touches the checked-out refs or the worktree.
pub fn fetch_repo_refs(repo_path: &Path, urls: &[String], refspec: &str) -> Result<()> {
    let repo = gix::open(repo_path)?;
    let refspec = gix::refspec::parse(
        gix::bstr::BStr::new(refspec),
        gix::refspec::parse::Operation::Fetch,
    )
    .context("invalid fetch refspec")?
    .to_owned();

    try_each_url(urls, "fetch", |url| {
        let url = transport_url(url);
        let options = gix::remote::ref_map::Options {
            extra_refspecs: vec![refspec.clone()],
            ..Default::default()
        };
        repo.remote_at(url.as_str())
            .with_context(|| format!("fetch from {url} failed"))?
            .connect(gix::remote::Direction::Fetch)
            .with_context(|| format!("fetch from {url} failed"))?
            .prepare_fetch(Discard, options)
            .with_context(|| format!("fetch from {url} failed"))?
            .receive(Discard, &IS_INTERRUPTED)
            .with_context(|| format!("fetch from {url} failed"))?;
        Ok(())
    })
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

/// The URL of the `origin` remote of the repository at `workdir`.
///
/// `None` when it has no `origin` yet.
pub fn origin_url(workdir: &Path) -> Result<Option<String>> {
    let Ok(repo) = gix::open(workdir) else {
        return Ok(None);
    };

    let Ok(remote) = repo.find_remote("origin") else {
        return Ok(None);
    };

    Ok(remote
        .url(gix::remote::Direction::Fetch)
        .map(|url| url.to_string()))
}

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

/// Resolve `rev` to a commit id, accepting full refs,
/// symbolic refs and the bare branch names callers pass, like git's DWIM.
fn resolve_commit<'a>(repo: &'a gix::Repository, rev: &str) -> Option<gix::Id<'a>> {
    if let Ok(id) = repo.rev_parse_single(rev.as_bytes()) {
        return Some(id);
    }

    // Branch names arrive bare, like git resolving `main`.
    if rev.contains('/') {
        return None;
    }

    repo.rev_parse_single(format!("refs/heads/{rev}").as_bytes())
        .ok()
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

/// Run `git -C dir args`, disabling the terminal prompt and capturing stderr.
///
/// `what` names the command in the spawn error.
fn git_output(dir: &Path, args: &[&str], what: &str) -> Result<std::process::Output> {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stderr(Stdio::piped())
        .output()
        .with_context(|| format!("failed to spawn `{what}`"))
}

/// Run a git command in `dir`, returning trimmed stdout.
///
/// The terminal prompt is disabled so a credential request fails instead of hanging.
#[cfg(test)]
fn git_in(dir: &Path, args: &[&str]) -> Result<String> {
    let output = git_output(dir, args, "git")?;

    if !output.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// Map an untrusted repository id or display name to a safe single path component.
///
/// Everything outside `[A-Za-z0-9._-]` becomes `_`.
/// An id that maps to exactly `.` or `..` becomes `_`.
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

/// In-memory object cache for history walks, see [`open_with_cache`].
///
/// Without one, a walk re-decodes the same commit objects from the object database.
/// Sized generously: a walk can cover a large portion of the repository's history.
const OBJECT_CACHE_BYTES: usize = 64 * 1024 * 1024;

/// Metadata of a commit, as shown in the repository browser's file header.
#[derive(Debug, Clone)]
pub struct FileCommit {
    /// Shortened commit id, 7+ hex chars, disambiguated if needed.
    pub id: String,
    /// First line of the commit message.
    pub summary: String,
    /// Rest of the commit message after the title.
    /// `None` for single-line commit messages.
    pub description: Option<String>,
    /// Author name.
    pub author: String,
    /// Author time, seconds since the Unix epoch.
    pub time: i64,
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

/// Open the repository at `workdir` with an in-memory object cache.
///
/// Only history walks use it, they re-decode the same commit objects repeatedly.
/// Single-object reads open the repository plain.
fn open_with_cache(workdir: &Path) -> Result<gix::Repository> {
    let mut repo = gix::open(workdir)?;
    repo.object_cache_size_if_unset(OBJECT_CACHE_BYTES);
    Ok(repo)
}

/// A [`FileCommit`] from a commit, with author, message title, body and shortened id.
///
/// The diff panel fetches the full commit on demand.
fn file_commit(commit: &gix::Commit<'_>) -> Result<FileCommit> {
    file_commit_with_description(commit, true)
}

/// A [`FileCommit`] without the message body, for history lists that never display it.
///
/// Skipping the body saves an allocation per listed commit.
fn file_commit_summary(commit: &gix::Commit<'_>) -> Result<FileCommit> {
    file_commit_with_description(commit, false)
}

/// [`file_commit`] and [`file_commit_summary`], `include_description` picks the body.
fn file_commit_with_description(
    commit: &gix::Commit<'_>,
    include_description: bool,
) -> Result<FileCommit> {
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

/// Find the most recent commit that changed `rel`, a path relative to the worktree.
///
/// `Ok(None)` when no commit touched the file, e.g. an untracked file.
pub fn last_commit(repo: &gix::Repository, rel: &Path) -> Result<Option<FileCommit>> {
    let rel = rel.to_path_buf();
    Ok(last_commits(repo, std::slice::from_ref(&rel))?
        .into_iter()
        .next()
        .map(|(_, commit)| commit))
}

/// Newest commit touching each of `rels`, like `git log -1 -- <rel>` per path.
/// `rels` are paths relative to the worktree.
///
/// Paths without any commit, like untracked files, are absent from the result.
pub fn worktree_last_commits(
    workdir: &Path,
    rels: &[PathBuf],
) -> Result<Vec<(PathBuf, FileCommit)>> {
    last_commits(&open_with_cache(workdir)?, rels)
}

/// The walk behind [`last_commit`] and [`worktree_last_commits`].
///
/// Stops as soon as every pending path has its commit.
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

        // Compare each unresolved path against this commit and its first parent.
        // Resolved paths leave the pending set.
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
                found.push((rel.clone(), file_commit(&commit)?));
                pending.swap_remove(ix);
            } else {
                ix += 1;
            }
        }
    }

    Ok(found)
}

/// Cap on [`CommitList::commits`]. The virtual list renders a window at a time,
/// the tab badge shows the real count.
///
/// A huge history is never fully materialized in memory.
pub const MAX_LISTED_COMMITS: usize = 20_000;

/// Commits reachable from `HEAD`, newest first, possibly capped.
pub struct CommitList {
    /// Number of commits reachable from HEAD.
    pub total: usize,
    /// Newest commits, capped at [`MAX_LISTED_COMMITS`].
    pub commits: Vec<FileCommit>,
}

/// All commits reachable from `HEAD`, newest first, with author and summary.
///
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
            commits.push(file_commit_summary(&info.object()?)?);
        }
    }
    Ok(CommitList { total, commits })
}

/// Like [`all_commits`], but opens the repository at `workdir` first.
///
/// For non-bare clones the clone root is the worktree.
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

/// A hunk of a file diff, like `@@ -a,b +c,d @@`.
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
    /// Path of the file relative to the repo root.
    /// For renames and copies, this is the destination path.
    pub path: String,
    /// Previous path, for renames and copies.
    pub old_path: Option<String>,
    pub status: DiffStatus,
    /// Number of added lines, 0 for binary files.
    pub insertions: usize,
    /// Number of removed lines, 0 for binary files.
    pub deletions: usize,
    /// True if either version is binary, then `hunks` is empty.
    pub binary: bool,
    pub hunks: Vec<DiffHunk>,
}

/// The changes of one commit.
#[derive(Debug, Clone)]
pub struct CommitDiff {
    pub files: Vec<FileDiff>,
}

/// The changes of the commit `id`, short or full, in the repository at `workdir`.
///
/// Compared against its first parent, the empty tree for the root commit.
pub fn worktree_commit_diff(workdir: &Path, id: &str) -> Result<CommitDiff> {
    commit_diff(&gix::open(workdir)?, id)
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

/// The changes between two commits, `base`..`tip`, like `git diff base tip`.
///
/// Directories and submodules are skipped, files are sorted by path.
pub fn worktree_commit_range_diff(workdir: &Path, base: &str, tip: &str) -> Result<CommitDiff> {
    let repo = gix::open(workdir)?;
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
        commits.push(file_commit_summary(&info.object()?)?);
    }
    Ok(commits)
}

/// The changes between two trees. Used by both [`commit_diff`] and [`worktree_commit_range_diff`].
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

        // Skip directory trees and submodule gitlinks, only files are listed.
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

        // Always diff with the built-in algorithm.
        // External diff drivers would shell out, out of scope for a read-only viewer.
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

/// Parse `git format-patch` output, a single patch or a series.
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

/// The name part of a `From: Name <email>` header value.
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

/// Parse one file's diff section.
///
/// Returns the section and the index of the first unconsumed line.
fn parse_diff_section(header: &str, lines: &[&str], start: usize) -> Result<(FileDiff, usize)> {
    let (header_old, header_new) = header_paths(header)?;
    // The `---` and `+++` lines name the two sides unambiguously.
    // The `diff --git` header cannot distinguish spaces in paths.
    // Fall back to the header for sections without them, pure renames and mode changes.
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
            // A literal binary patch may follow.
            // Skip it without consuming the next section's header.
            while i < lines.len() && !lines[i].starts_with("diff --git ") {
                i += 1;
            }
            break;
        }
        // Everything else, index, mode and similarity lines, is ignored.
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

/// Parse one hunk, the `@@ -a,b +c,d @@` header plus every body line.
///
/// Returns the hunk and the index of the first unconsumed line.
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

        // Context lines advance both counters.
        // Deletions advance only the old counter, additions only the new one.
        // Every line then carries its real number in both versions.
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

/// The kind of a hunk body line, from its first character.
///
/// Lines outside a hunk, headers, `\ No newline...` and the next section, yield `None`.
fn line_prefix_kind(line: &str) -> Option<DiffLineKind> {
    match line.as_bytes().first()? {
        b' ' => Some(DiffLineKind::Context),
        b'+' => Some(DiffLineKind::Addition),
        b'-' => Some(DiffLineKind::Deletion),
        _ => None,
    }
}

/// Parse a unified-diff hunk header, `@@ -a,b +c,d @@`.
///
/// Omitted line counts default to 1.
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

/// The old and new paths of a `diff --git a/X b/Y` header.
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

/// The path of a `--- a/X` or `+++ b/Y` line.
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

/// The content of a git C-style quoted path and the rest of the input.
/// The path spans the opening `"`, escaped content and closing `"`.
///
/// `None` if unterminated.
fn take_quoted(input: &str) -> Option<(&str, &str)> {
    let mut end = 1; // byte after the opening quote
    let mut rest = &input[1..];
    while let Some(ch) = rest.chars().next() {
        let len = ch.len_utf8();
        match ch {
            '\\' => {
                // Consume the escaped character too, it may be multi-byte.
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

/// Undo git's C-style path quoting, `\NNN` octal escapes, `\"` and `\\`.
///
/// Delegates to gitoxide's C-style quote implementation, `gix::quote::ansi_c::undo`.
/// It expects the surrounding double quotes, which are re-added around the interior.
fn unquote_path(path: &str) -> Result<String> {
    if !path.contains('\\') {
        return Ok(path.to_owned());
    }

    let quoted = format!("\"{path}\"");
    let (unquoted, _) = gix::quote::ansi_c::undo(gix::bstr::BStr::new(quoted.as_bytes()))
        .map_err(|e| anyhow::anyhow!("malformed quoted path: {e}"))?;
    String::from_utf8(unquoted.into_owned().to_vec()).context("invalid UTF-8 in quoted path")
}

/// Collects the hunks of one blob diff while tracking per-line numbers.
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

/// The commit HEAD points to, like `git log -1`.
///
/// `Ok(None)` for a repository without commits yet, an unborn HEAD.
pub fn head_commit(repo: &gix::Repository) -> Result<Option<FileCommit>> {
    let Some(head) = repo.head_id().ok() else {
        return Ok(None);
    };
    let commit = head.object()?.into_commit();
    Ok(Some(file_commit(&commit)?))
}

/// Full metadata of the commit `id`, short or full, in the repository at `workdir`.
/// Like [`head_commit`] for an arbitrary commit.
///
/// `Ok(None)` when the id cannot be resolved.
pub fn worktree_commit(workdir: &Path, id: &str) -> Result<Option<FileCommit>> {
    let repo = gix::open(workdir)?;
    match repo.rev_parse_single(id.as_bytes()) {
        Ok(commit_id) => {
            let commit = commit_id.object()?.into_commit();
            Ok(Some(file_commit(&commit)?))
        }
        Err(_) => Ok(None),
    }
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

/// Snapshot the worktree after a branch or tag switch.
///
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

/// Check out `tree` into the worktree of `repo`
fn force_checkout(repo: &gix::Repository, tree: &gix::hash::oid) -> Result<()> {
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

    // Check out the index into the worktree.
    gix_worktree_state::checkout(
        &mut index,
        workdir,
        objects,
        &files,
        &bytes,
        &gix::interrupt::IS_INTERRUPTED,
        options,
    )?;

    // Write the index to disk.
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

    // Update the reference, creating a reflog entry.
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

    // Move HEAD to the branch, creating a reflog entry.
    move_head(
        &repo,
        signature,
        gix::refs::Target::Symbolic(branch),
        &format!("checkout: moving to {name}"),
    )?;

    // Check out the branch's tree, replacing index + worktree.
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

    // Move HEAD to the tag, creating a reflog entry.
    move_head(
        &repo,
        signature,
        gix::refs::Target::Object(commit.detach()),
        &format!("checkout: moving to {name}"),
    )?;

    // Check out the tag's tree, replacing index + worktree.
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
    fn find_git_repos_discovers_repositories_recursively() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();

        // Repositories are found at any depth.
        // A linked worktree, with a `.git` file instead of a directory, counts too.
        let nested = root.join("a/b/project");
        std::fs::create_dir_all(nested.join(".git")).unwrap();
        let worktree = root.join("wt");
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::write(
            worktree.join(".git"),
            "gitdir: ../a/b/project/.git/worktrees/wt",
        )
        .unwrap();

        // Plain directories are not repositories.
        std::fs::create_dir_all(root.join("plain")).unwrap();

        // Hidden entries and dependency caches are skipped.
        std::fs::create_dir_all(root.join(".hidden/repo/.git")).unwrap();
        std::fs::create_dir_all(root.join("node_modules/pkg/.git")).unwrap();

        // A repository is not descended into.
        // Repositories inside it, like submodule worktrees, are not reported.
        let outer = root.join("outer");
        std::fs::create_dir_all(outer.join(".git")).unwrap();
        std::fs::create_dir_all(outer.join("sub/other/.git")).unwrap();

        let mut found = find_git_repos(root);
        found.sort();

        let mut expected = vec![
            nested.canonicalize().unwrap(),
            worktree.canonicalize().unwrap(),
            outer.canonicalize().unwrap(),
        ];
        expected.sort();
        assert_eq!(found, expected);
    }

    #[test]
    fn root_commit_reports_the_first_ancestor() {
        let (dir, repo) = fixture(&[("a.txt", b"one")]);
        commit_all(&repo, "initial");

        let dir = dir.path();
        let root = root_commit(dir).expect("root").expect("commit");
        assert_eq!(root.len(), 40);

        // The root commit does not change when history grows.
        std::fs::write(dir.join("b.txt"), b"two").expect("write");
        commit_all(&repo, "second");
        assert_eq!(
            root_commit(dir).expect("root").as_deref(),
            Some(root.as_str())
        );
    }

    #[test]
    fn root_commit_is_none_without_commits() {
        let (_dir, repo) = fixture(&[("a.txt", b"one")]);
        let workdir = repo.workdir().expect("workdir");
        assert_eq!(root_commit(workdir).expect("root"), None);
    }

    #[test]
    fn push_all_mirrors_branches_and_tags() {
        // A bare server repository reachable via a `file://` URL.
        // Mirrors a grasp server's `{base}/{owner}/{repo-id}.git` layout.
        let server = tempfile::tempdir().unwrap();
        let server_repo = server.path().join("npub1test").join("my-repo.git");
        std::fs::create_dir_all(server_repo.parent().unwrap()).unwrap();
        let init_status = Command::new("git")
            .args(["init", "--bare", "-q"])
            .arg(&server_repo)
            .status()
            .expect("spawn git init --bare");
        assert!(init_status.success());

        let (dir, repo) = fixture(&[("a.txt", b"one")]);
        commit_all(&repo, "initial");
        let dir = dir.path();

        // Two branches plus a tag are all mirrored.
        git_run(dir, &["checkout", "-b", "feature"]);
        std::fs::write(dir.join("b.txt"), b"two").expect("write");
        commit_all(&repo, "feature work");
        git_run(dir, &["checkout", "-"]);
        git_run(dir, &["tag", "v1.0"]);

        let base_url = format!("file://{}", server.path().display());
        push_all(dir, &base_url, "npub1test", "my-repo").expect("push");

        let refs = git_in(&server_repo, &["show-ref"]).expect("server refs");
        assert!(refs.contains("refs/heads/main"));
        assert!(refs.contains("refs/heads/feature"));
        assert!(refs.contains("refs/tags/v1.0"));
    }

    #[test]
    fn push_all_tolerates_a_missing_ref_kind() {
        // A repository with only tags and no branches still pushes.
        // Wildcard refspecs without a local match are ignored.
        let server = tempfile::tempdir().unwrap();
        let server_repo = server.path().join("npub1test").join("my-repo.git");
        std::fs::create_dir_all(server_repo.parent().unwrap()).unwrap();
        let init_status = Command::new("git")
            .args(["init", "--bare", "-q"])
            .arg(&server_repo)
            .status()
            .expect("spawn git init --bare");
        assert!(init_status.success());

        let (dir, repo) = fixture(&[("a.txt", b"one")]);
        commit_all(&repo, "initial");
        let dir = dir.path();
        git_run(dir, &["tag", "v1.0"]);
        git_run(dir, &["update-ref", "-d", "refs/heads/main"]);

        let base_url = format!("file://{}", server.path().display());
        push_all(dir, &base_url, "npub1test", "my-repo").expect("push");

        let refs = git_in(&server_repo, &["show-ref"]).expect("server refs");
        assert!(refs.contains("refs/tags/v1.0"));
        assert!(!refs.contains("refs/heads/"));
    }

    #[test]
    fn remote_has_refs_reports_whether_pushed_refs_landed() {
        let server = tempfile::tempdir().unwrap();
        let server_repo = server.path().join("npub1test").join("my-repo.git");
        std::fs::create_dir_all(server_repo.parent().unwrap()).unwrap();
        let init_status = Command::new("git")
            .args(["init", "--bare", "-q"])
            .arg(&server_repo)
            .status()
            .expect("spawn git init --bare");
        assert!(init_status.success());

        let (dir, repo) = fixture(&[("a.txt", b"one")]);
        commit_all(&repo, "initial");
        let dir = dir.path();
        let main = git_in(dir, &["rev-parse", "refs/heads/main"]).expect("main oid");
        let url = format!("file://{}/npub1test/my-repo.git", server.path().display());
        let expected = vec![("refs/heads/main".to_owned(), main.clone())];

        // Nothing pushed yet: the ref is absent.
        assert!(!remote_has_refs(dir, &url, &expected).expect("probe"));

        push_all(
            dir,
            &format!("file://{}", server.path().display()),
            "npub1test",
            "my-repo",
        )
        .expect("push");

        // The pushed ref is advertised at the expected commit.
        assert!(remote_has_refs(dir, &url, &expected).expect("probe"));

        // A stale expectation - the exact race a retry resolves - is false.
        let stale = vec![("refs/heads/main".to_owned(), "0".repeat(40))];
        assert!(!remote_has_refs(dir, &url, &stale).expect("probe"));

        // Extra remote refs (e.g. a tag pushed later) do not invalidate the
        // refs this push wanted to land.
        git_run(dir, &["tag", "v1.0"]);
        push_all(
            dir,
            &format!("file://{}", server.path().display()),
            "npub1test",
            "my-repo",
        )
        .expect("push");
        assert!(remote_has_refs(dir, &url, &expected).expect("probe"));
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

    /// Build a throwaway non-bare repository from `(rel, bytes)` file pairs.
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

    /// Stage everything and create a commit with the git CLI.
    /// Like [`apply_patch`], the crate already shells out to the CLI.
    fn commit_all(repo: &gix::Repository, message: &str) {
        git_run(repo.workdir().expect("workdir"), &["add", "-A"]);
        git_run(repo.workdir().expect("workdir"), &["commit", "-m", message]);
    }

    #[test]
    fn merge_base_finds_the_fork_point_and_reports_unrelated_history() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("repo");
        let initial = init_repository(&path, "My Repo", "desc").expect("init");

        // A feature branch and a mainline commit diverge from the initial commit.
        // The initial commit is their merge base.
        git_run(&path, &["checkout", "-b", "feature"]);
        std::fs::write(path.join("feature.txt"), "feature\n").expect("write");
        commit_all(&gix::open(&path).expect("open"), "feature commit");
        git_run(&path, &["checkout", "main"]);
        std::fs::write(path.join("main.txt"), "main\n").expect("write");
        commit_all(&gix::open(&path).expect("open"), "mainline commit");

        assert_eq!(
            merge_base(&path, "feature", "main")
                .expect("merge base")
                .as_deref(),
            Some(initial.as_str())
        );

        // An orphan branch shares no history with main, so `Ok(None)`.
        git_run(&path, &["checkout", "--orphan", "orphan"]);
        std::fs::write(path.join("orphan.txt"), "orphan\n").expect("write");
        commit_all(&gix::open(&path).expect("open"), "orphan commit");
        assert_eq!(merge_base(&path, "orphan", "main").expect("ok"), None);

        // An unresolvable revision is an error, not a missing ancestor.
        assert!(merge_base(&path, "orphan", "no-such-ref").is_err());
    }

    #[test]
    fn format_patch_between_produces_the_series_and_rejects_empty_ranges() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("repo");
        let initial = init_repository(&path, "My Repo", "desc").expect("init");

        git_run(&path, &["checkout", "-b", "feature"]);
        std::fs::write(path.join("feature.txt"), "feature\n").expect("write");
        commit_all(&gix::open(&path).expect("open"), "feature commit");

        let patch = format_patch_between(&path, &initial, "feature").expect("patch");
        assert!(patch.contains("Subject: [PATCH] feature commit"));
        assert!(patch.contains("feature.txt"));

        // An empty range has no commits to send.
        assert!(format_patch_between(&path, "feature", "feature").is_err());
    }

    #[test]
    fn push_commit_ref_pushes_to_the_event_namespace() {
        // A bare server repository reachable via a `file://` URL.
        // Mirrors a grasp server's `{base}/{owner}/{repo-id}.git` layout.
        let server = tempfile::tempdir().unwrap();
        let server_repo = server.path().join("npub1test").join("my-repo.git");
        std::fs::create_dir_all(server_repo.parent().unwrap()).unwrap();
        let init_status = Command::new("git")
            .args(["init", "--bare", "-q"])
            .arg(&server_repo)
            .status()
            .expect("spawn git init --bare");
        assert!(init_status.success());

        let (dir, repo) = fixture(&[("a.txt", b"one")]);
        commit_all(&repo, "initial");
        let dir = dir.path();
        let tip = git_in(dir, &["rev-parse", "HEAD"]).expect("tip");

        let url = format!("file://{}/npub1test/my-repo.git", server.path().display());
        push_commit_ref(dir, &url, &tip, "refs/nostr/abcd1234").expect("push");

        let refs = git_in(&server_repo, &["show-ref"]).expect("server refs");
        assert!(refs.contains("refs/nostr/abcd1234"));
    }

    #[test]
    fn split_patch_series_splits_real_multi_commit_mboxes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("repo");
        let initial = init_repository(&path, "My Repo", "desc").expect("init");

        git_run(&path, &["checkout", "-b", "feature"]);
        std::fs::write(path.join("one.txt"), "one\n").expect("write");
        commit_all(&gix::open(&path).expect("open"), "first commit");
        std::fs::write(path.join("two.txt"), "two\n").expect("write");
        commit_all(&gix::open(&path).expect("open"), "second commit");

        let series = format_patch_between(&path, &initial, "feature").expect("series");
        let parts = split_patch_series(&series);

        assert_eq!(parts.len(), 2);
        assert!(parts[0].contains("Subject: [PATCH 1/2] first commit"));
        assert!(parts[1].contains("Subject: [PATCH 2/2] second commit"));
        // Each part starts its own mbox message with its own commit id.
        let first = parts[0].lines().next().expect("first header");
        let second = parts[1].lines().next().expect("second header");
        assert!(first.starts_with("From ") && first.len() >= 45);
        assert_ne!(first, second);
    }

    #[test]
    fn split_patch_series_keeps_single_patches_whole() {
        let patch = "From abcdefabcdefabcdefabcdefabcdefabcdefab Mon Sep 17 00:00:00 2001\nFrom: A <a@b>\nSubject: [PATCH] fix\n\n---\n";
        let parts = split_patch_series(patch);
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0], patch);
    }

    #[test]
    fn head_commit_and_commits_since_track_applied_commits() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("repo");
        let initial = init_repository(&path, "My Repo", "desc").expect("init");

        assert_eq!(
            head_commit_id(&path).expect("head").as_deref(),
            Some(initial.as_str())
        );
        // No commits yet, `HEAD` alone.
        assert_eq!(
            commits_since(&path, None).expect("commits"),
            vec![initial.clone()]
        );

        std::fs::write(path.join("one.txt"), "one\n").expect("write");
        commit_all(&gix::open(&path).expect("open"), "first commit");
        let first = head_commit_id(&path).expect("head").expect("on a branch");

        std::fs::write(path.join("two.txt"), "two\n").expect("write");
        commit_all(&gix::open(&path).expect("open"), "second commit");
        let second = head_commit_id(&path).expect("head").expect("on a branch");

        // Oldest first, like the order `git am` creates them.
        assert_eq!(
            commits_since(&path, Some(&initial)).expect("commits"),
            vec![first.clone(), second.clone()]
        );
        assert_eq!(
            commits_since(&path, Some(&first)).expect("commits"),
            vec![second]
        );
    }

    #[test]
    fn head_commit_reports_unborn_repositories() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("repo");
        let status = Command::new("git")
            .args(["init", "-q"])
            .arg(&path)
            .status()
            .expect("spawn git init");
        assert!(status.success());

        assert_eq!(head_commit_id(&path).expect("head"), None);
        assert_eq!(
            commits_since(&path, None).expect("commits"),
            Vec::<String>::new()
        );
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
        // [`FileCommit`] carries the short id, the full id is 40 chars.
        assert_eq!(
            head_commit(&repo).expect("head").expect("commit").id,
            &commit[..7]
        );

        let state = repo_ref_state(&repo).expect("refs");
        assert_eq!(state.head.as_deref(), Some("main"));
        assert_eq!(state.refs, vec![("refs/heads/main".to_owned(), commit)]);

        // The index matches the committed tree, so the fresh repo is clean.
        assert!(!worktree_dirty(workdir));
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
        // The standard fetch mapping is configured with the remote.
        // Later `git fetch origin` updates `refs/remotes/origin/*`.
        assert_eq!(
            git_in(&path, &["config", "remote.origin.fetch"]).expect("refspec"),
            "+refs/heads/*:refs/remotes/origin/*"
        );

        // A second call must not override the existing remote.
        ensure_origin(&path, "https://other.example/repo.git").expect("keep");
        assert_eq!(
            git_in(&path, &["remote", "get-url", "origin"]).expect("url"),
            "https://gitnostr.com/npub1test/repo.git"
        );
    }

    #[test]
    fn origin_url_reads_the_remote_or_reports_none() {
        let (dir, _repo) = fixture(&[("a.txt", b"one")]);
        commit_all(&_repo, "initial");
        let dir = dir.path();

        // No remote configured yet.
        assert_eq!(origin_url(dir).expect("read"), None);

        ensure_origin(dir, "https://gitnostr.com/npub1test/repo.git").expect("add");
        assert_eq!(
            origin_url(dir).expect("read").as_deref(),
            Some("https://gitnostr.com/npub1test/repo.git")
        );
    }

    #[test]
    fn set_origin_creates_or_replaces_the_remote() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("my-repo");
        init_repository(&path, "My Repo", "").expect("init");

        // No origin yet, so one is added.
        set_origin(&path, "https://gitnostr.com/npub1test/repo.git").expect("add");
        assert_eq!(
            origin_url(&path).expect("url").as_deref(),
            Some("https://gitnostr.com/npub1test/repo.git")
        );

        // An existing origin is replaced, not duplicated.
        // A clone's origin points at the cloned-from path.
        // It is re-targeted at the grasp server.
        set_origin(&path, "https://grasp.example/npub1test/repo.git").expect("replace");
        assert_eq!(
            origin_url(&path).expect("url").as_deref(),
            Some("https://grasp.example/npub1test/repo.git")
        );
    }

    #[test]
    fn working_copy_cloned_from_the_mirror_matches_head_and_origin() {
        // The mirror is a freshly initialized repository.
        // Its `origin` points at the grasp server.
        // `Backend::create_repository` leaves it in the GitCache.
        let dir = tempfile::tempdir().expect("tempdir");
        let mirror = dir.path().join("mirror");
        let commit = init_repository(&mirror, "My Repo", "Does things.").expect("init");
        ensure_origin(&mirror, "https://gitnostr.com/npub1test/my-repo.git").expect("origin");

        // The working copy is cloned from the mirror.
        // It then shares the announced history exactly.
        // `origin` is re-pointed at the grasp server instead of the mirror path.
        let destination = dir.path().join("folder").join("My_Repo");
        std::fs::create_dir_all(destination.parent().unwrap()).expect("parent");
        clone_repo(&[format!("file://{}", mirror.display())], &destination).expect("clone");
        set_origin(&destination, "https://gitnostr.com/npub1test/my-repo.git").expect("set origin");

        assert_eq!(
            origin_url(&destination).expect("url").as_deref(),
            Some("https://gitnostr.com/npub1test/my-repo.git")
        );
        assert_eq!(
            head_commit_id(&destination).expect("head").as_deref(),
            Some(commit.as_str())
        );
        assert!(destination.join("README.md").is_file());
    }

    #[test]
    fn fast_forward_branches_moves_the_mirror_and_keeps_local_work() {
        // A bare server, like a grasp server's `{base}/{owner}/{repo}.git` layout.
        let dir = tempfile::tempdir().expect("tempdir");
        let base_server = dir.path().join("npub1test").join("repo.git");
        std::fs::create_dir_all(base_server.parent().unwrap()).unwrap();
        let init_status = Command::new("git")
            .args(["init", "--bare", "-q"])
            .arg(&base_server)
            .status()
            .expect("spawn git init --bare");
        assert!(init_status.success());

        // The owner's working repo pushes the initial commit.
        let (work_dir, work_repo) = fixture(&[("a.txt", b"one")]);
        commit_all(&work_repo, "initial");
        let work = work_dir.path();
        let base_url = format!("file://{}", dir.path().display());
        push_all(work, &base_url, "npub1test", "repo").expect("push");

        // A mirror clone, like the app's GitCache clones.
        let mirror = dir.path().join("mirror");
        git_run(
            dir.path(),
            &[
                "clone",
                "-q",
                &format!("{base_url}/npub1test/repo.git"),
                mirror.to_str().unwrap(),
            ],
        );
        let initial = git_in(&mirror, &["rev-parse", "HEAD"]).expect("initial");

        // The owner pushes a new commit.
        // The mirror fetches it, but its local `main` and worktree stay behind.
        std::fs::write(work.join("new.txt"), b"new\n").expect("write");
        commit_all(&gix::open(work).expect("open"), "new commit");
        push_all(work, &base_url, "npub1test", "repo").expect("push");
        git_run(&mirror, &["fetch", "origin"]);
        let remote = git_in(&mirror, &["rev-parse", "refs/remotes/origin/main"]).expect("remote");
        assert_eq!(
            git_in(&mirror, &["rev-parse", "HEAD"]).expect("local"),
            initial
        );
        assert_ne!(remote, initial);

        // Fast-forwarding catches the branch and its worktree up.
        // The second call has nothing left to move.
        assert!(fast_forward_branches(&mirror).expect("ff"));
        assert_eq!(
            git_in(&mirror, &["rev-parse", "HEAD"]).expect("local"),
            remote
        );
        assert!(mirror.join("new.txt").is_file());
        assert!(!fast_forward_branches(&mirror).expect("idle"));

        // A branch with local commits of its own is never touched.
        git_run(&mirror, &["checkout", "-b", "wip"]);
        std::fs::write(mirror.join("wip.txt"), b"wip\n").expect("write");
        commit_all(&gix::open(&mirror).expect("open"), "local wip");
        let wip = git_in(&mirror, &["rev-parse", "HEAD"]).expect("wip");
        assert!(!fast_forward_branches(&mirror).expect("wip skipped"));
        assert_eq!(
            git_in(&mirror, &["rev-parse", "HEAD"]).expect("wip kept"),
            wip
        );
    }

    #[test]
    fn fetch_repo_refs_imports_heads_under_a_prefix() {
        let dir = tempfile::tempdir().expect("tempdir");

        // A bare base server holding the initial commit.
        // Like a grasp server's `{base}/{owner}/{repo-id}.git` layout.
        let base_server = dir.path().join("npub1base").join("base.git");
        std::fs::create_dir_all(base_server.parent().unwrap()).unwrap();
        let init_status = Command::new("git")
            .args(["init", "--bare", "-q"])
            .arg(&base_server)
            .status()
            .expect("spawn git init --bare");
        assert!(init_status.success());

        let (upstream_dir, upstream_repo) = fixture(&[("a.txt", b"one")]);
        commit_all(&upstream_repo, "initial");
        let upstream_path = upstream_dir.path();
        let initial = git_in(upstream_path, &["rev-parse", "HEAD"]).expect("initial");
        push_all(
            upstream_path,
            &format!("file://{}", dir.path().display()),
            "npub1base",
            "base",
        )
        .expect("push");

        // The base mirror is a plain clone of the base server.
        let base_url = format!("file://{}", base_server.display());
        let mirror = dir.path().join("mirror");
        git_run(
            dir.path(),
            &["clone", "-q", &base_url, mirror.to_str().unwrap()],
        );

        // The fork server has the same initial commit.
        // It also carries a feature commit on its own `feature` branch.
        let fork_work = dir.path().join("fork-work");
        git_run(
            dir.path(),
            &["clone", "-q", &base_url, fork_work.to_str().unwrap()],
        );
        git_run(&fork_work, &["checkout", "-b", "feature"]);
        std::fs::write(fork_work.join("feature.txt"), "feature\n").expect("write");
        commit_all(&gix::open(&fork_work).expect("open"), "feature commit");
        let tip = git_in(&fork_work, &["rev-parse", "HEAD"]).expect("tip");

        let fork_server = dir.path().join("npub1fork").join("fork.git");
        std::fs::create_dir_all(fork_server.parent().unwrap()).unwrap();
        let init_status = Command::new("git")
            .args(["init", "--bare", "-q"])
            .arg(&fork_server)
            .status()
            .expect("spawn git init --bare");
        assert!(init_status.success());
        push_commit_ref(
            &fork_work,
            &format!("file://{}", fork_server.display()),
            &tip,
            "refs/heads/feature",
        )
        .expect("push");

        // Import the fork's heads into the mirror under a private prefix.
        // The first dead URL is skipped, the second works.
        let dead = format!("file://{}/missing.git", dir.path().display());
        fetch_repo_refs(
            &mirror,
            &[dead, format!("file://{}", fork_server.display())],
            "+refs/heads/*:refs/fork/npub1fork/fork/*",
        )
        .expect("fetch");

        // The imported refs are listed under the prefix only.
        assert_eq!(
            refs_with_prefix(&mirror, "refs/fork/npub1fork/fork").expect("refs"),
            vec!["refs/fork/npub1fork/fork/feature"]
        );
        // Nothing leaked into the normal ref namespaces.
        assert_eq!(
            refs_with_prefix(&mirror, "refs/heads/fork").expect("refs"),
            Vec::<String>::new()
        );

        // The mirror can now range across both histories.
        // The fork point is the shared initial commit, the proposal covers the fork commit.
        assert_eq!(
            merge_base(
                &mirror,
                "refs/remotes/origin/main",
                "refs/fork/npub1fork/fork/feature",
            )
            .expect("merge base")
            .as_deref(),
            Some(initial.as_str())
        );
        let patch = format_patch_between(&mirror, &initial, "refs/fork/npub1fork/fork/feature")
            .expect("patch");
        assert!(patch.contains("Subject: [PATCH] feature commit"));
        assert!(patch.contains("feature.txt"));

        // Pruning the prefix removes the import again.
        delete_refs_with_prefix(&mirror, "refs/fork/npub1fork/fork").expect("delete");
        assert_eq!(
            refs_with_prefix(&mirror, "refs/fork/npub1fork/fork").expect("refs"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn fetch_repo_refs_fails_when_every_url_fails() {
        let (_dir, repo) = fixture(&[("a.txt", b"one")]);
        commit_all(&repo, "initial");
        let dir = _dir.path();

        let dead = format!("file://{}/missing.git", dir.display());
        let err = fetch_repo_refs(dir, &[dead], "+refs/heads/*:refs/fork/x/*")
            .expect_err("all URLs fail");
        assert!(err.to_string().contains("failed to fetch"));

        // Without any URL there is nothing to try.
        let err = fetch_repo_refs(dir, &[], "+refs/heads/*:refs/fork/x/*").expect_err("no URLs");
        assert!(err.to_string().contains("no clone URLs"));
    }

    #[test]
    fn delete_refs_with_prefix_is_a_noop_without_matches() {
        let (_dir, repo) = fixture(&[("a.txt", b"one")]);
        commit_all(&repo, "initial");
        delete_refs_with_prefix(_dir.path(), "refs/fork/nothing").expect("noop");
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
        // `--no-ff` forces a merge commit, it is the latest commit changing a.txt.
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
                // Untracked paths are absent from the result.
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

        // Unborn HEAD means no commit yet.
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

        // The initial branch name depends on git configuration.
        // Only the branch we created is fixed.
        let branches = worktree_branches(dir).expect("branches");
        assert_eq!(branches.len(), 2);
        assert!(branches.contains(&"feature".to_string()));
        assert!(branches.windows(2).all(|pair| pair[0] <= pair[1]), "sorted");

        assert_eq!(
            repo_tags(&repo).expect("tags"),
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
        // One hunk with context around the single-line change.
        // The removed line is old 2, the added line is new 2.
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

        // The root commit diffs against the empty tree, everything is added.
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
        // A pure rename has no content change, the file is still listed.
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
        // A hand-written patch without author or date headers still lists a commit.
        // Time stays 0 and the author stays empty.
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
        // A diff-only body without an mbox envelope has no commits.
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
        // Build a commit touching a mix of file kinds.
        // Feed genuine `git format-patch` output through the parser.
        // It covers quoted and octal-escaped paths.
        // There are also a rename-free modification, an addition and a binary deletion.
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

        // Space in the name makes git quote the path in the header.
        let file = by_path("my file.txt");
        assert_eq!(file.status, DiffStatus::Modified);
        assert_eq!(file.insertions, 1);

        // UTF-8 names are emitted as octal escapes.
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

        // A binary deletion emits no `---` or `+++` lines.
        // Only the mode line and the `Binary files` marker remain.
        let file = by_path("img.png");
        assert_eq!(file.status, DiffStatus::Deleted);
        assert!(file.binary);
        assert!(file.hunks.is_empty());
    }

    #[test]
    fn worktree_dirty_tracks_changes_and_untracked_files() {
        let (dir, repo) = fixture(&[("tracked.txt", b"one")]);
        commit_all(&repo, "initial");
        let workdir = dir.path();

        assert!(!worktree_dirty(workdir));

        // A modified tracked file is dirty.
        std::fs::write(workdir.join("tracked.txt"), b"two").expect("write");
        assert!(worktree_dirty(workdir));

        // After restoring, an untracked file alone is dirty as well.
        git_run(workdir, &["checkout", "--", "tracked.txt"]);
        assert!(!worktree_dirty(workdir));
        std::fs::write(workdir.join("untracked.txt"), b"new").expect("write");
        assert!(worktree_dirty(workdir));

        // A staged change counts too.
        git_run(workdir, &["rm", "--cached", "tracked.txt"]);
        assert!(worktree_dirty(workdir));

        // A missing directory is clean, not an error.
        assert!(!worktree_dirty(&dir.path().join("missing")));
    }

    #[test]
    fn worktree_dirty_reports_unborn_worktrees_with_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("repo");
        let status = Command::new("git")
            .args(["init", "-q"])
            .arg(&path)
            .status()
            .expect("spawn git init");
        assert!(status.success());

        // No commits and no files: porcelain is empty.
        assert!(!worktree_dirty(&path));
        // An unborn repository holding files is dirty.
        std::fs::write(path.join("README.md"), "# hello\n").expect("write");
        assert!(worktree_dirty(&path));
    }

    #[test]
    fn worktree_commits_ahead_counts_branch_only_commits() {
        let (dir, repo) = fixture(&[("a.txt", b"one")]);
        commit_all(&repo, "initial");
        let path = dir.path();

        git_run(path, &["checkout", "-q", "-b", "feature"]);
        std::fs::write(path.join("f.txt"), b"f\n").expect("write");
        commit_all(&gix::open(path).expect("open"), "feature work");

        assert_eq!(worktree_commits_ahead(path, "main", "feature"), 1);
        assert_eq!(worktree_commits_ahead(path, "feature", "main"), 0);

        git_run(path, &["checkout", "-q", "main"]);
        assert_eq!(worktree_current_branch(path).as_deref(), Some("main"));
        assert!(worktree_ref_exists(path, "refs/heads/feature"));
        assert!(!worktree_ref_exists(path, "refs/heads/nope"));
        assert_eq!(worktree_commits_ahead(path, "main", "feature"), 1);
    }
}

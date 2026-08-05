//! Blocking local git operations against GRASP servers.
//!
//! All functions may block; call them inside `cx.background_spawn`.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
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
            .join(addr.owner.to_hex())
            .join(sanitize_path_component(&addr.id))
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

        let mut last_err: Option<anyhow::Error> = None;

        for url in clone_urls {
            match clone(url, &path) {
                Ok(repo) => return Ok(repo),
                Err(e) => last_err = Some(e),
            }
        }

        match last_err {
            Some(e) => Err(e).context("failed to clone from any mirror"),
            None => bail!("no clone URLs provided"),
        }
    }
}

/// Fetch all configured refspecs from `origin`.
pub fn fetch_all(repo: &gix::Repository) -> Result<()> {
    repo.find_remote("origin")?
        .connect(gix::remote::Direction::Fetch)?
        .prepare_fetch(Discard, Default::default())?
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
    let url = gix::url::parse(url).context("invalid clone URL")?;

    let mut prepare = gix::prepare_clone(url, path)?;
    let (mut checkout, _fetch) = prepare.fetch_then_checkout(Discard, &IS_INTERRUPTED)?;
    let (repo, _checkout) = checkout.main_worktree(Discard, &IS_INTERRUPTED)?;

    Ok(repo)
}

fn sanitize_path_component(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

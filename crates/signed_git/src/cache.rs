use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use signed_core::{Announcement, RepoAddr};

use crate::remote::{clone_repo, fetch_all};

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
    pub fn ensure_clone<U: AsRef<str>>(
        &self,
        addr: &RepoAddr,
        clone_urls: &[U],
    ) -> Result<gix::Repository> {
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

/// The refs namespace of a fork's import in the target mirror.
pub fn fork_namespace(announcement: &Announcement) -> String {
    format!(
        "{}/{}",
        announcement.owner.to_hex(),
        sanitize_path_component(&announcement.id)
    )
}

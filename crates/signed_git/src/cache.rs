use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use signed_core::{Announcement, RepoAddr};

use crate::repo::Repo;

#[derive(Debug, Clone)]
pub struct GitCache {
    root: PathBuf,
}

impl GitCache {
    /// Creates a cache rooted at `root`.
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Returns the cache root directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Returns the cache path for a repository address.
    pub fn repo_path(&self, addr: &RepoAddr) -> PathBuf {
        self.root
            .join(addr.public_key().to_hex())
            .join(Self::sanitize_path_component(addr.identifier()))
    }

    /// Opens the cached repository, or `None` when it is not cloned yet.
    pub fn open(&self, addr: &RepoAddr) -> Result<Option<Repo>> {
        let path = self.repo_path(addr);
        match gix::open(&path) {
            Ok(repo) => Ok(Some(Repo::new(repo))),
            Err(e) if e.is_not_found() => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Returns the cached repository, cloning from one of `clone_urls` on first use.
    pub fn ensure_clone<U: AsRef<str>>(&self, addr: &RepoAddr, clone_urls: &[U]) -> Result<Repo> {
        let path = self.repo_path(addr);

        if let Some(repo) = self.open(addr)? {
            if let Err(error) = repo.fetch() {
                log::warn!("failed to refresh the cached repository: {error:#}");
            }
            return Ok(repo);
        }

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }

        Repo::clone(clone_urls, &path)
    }

    /// Maps untrusted relay content onto a safe single path component.
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

    /// Returns the `owner/identifier` namespace used for fork checkouts.
    pub fn fork_namespace(announcement: &Announcement) -> String {
        format!(
            "{}/{}",
            announcement.owner.to_hex(),
            Self::sanitize_path_component(&announcement.id)
        )
    }
}

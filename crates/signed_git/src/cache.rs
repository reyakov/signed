use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use anyhow::Result;
use signed_core::{Announcement, RepoAddr};

use crate::repo::Repo;

#[derive(Debug, Clone)]
pub struct GitCache {
    root: PathBuf,
    clone_locks: Arc<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>>,
}

impl GitCache {
    /// Creates a cache rooted at `root`.
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            clone_locks: Arc::default(),
        }
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

    /// Returns the clone lock for `path`, creating it on first use.
    fn clone_lock(&self, path: &Path) -> Arc<Mutex<()>> {
        let mut locks = self
            .clone_locks
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        Arc::clone(locks.entry(path.to_path_buf()).or_default())
    }

    /// Returns the cached repository, cloning from one of `clone_urls` on first use.
    pub fn ensure_clone<U: AsRef<str>>(&self, addr: &RepoAddr, clone_urls: &[U]) -> Result<Repo> {
        let path = self.repo_path(addr);

        // Lock the clone lock for this path to prevent concurrent clones.
        let clone_lock = self.clone_lock(&path);
        let _clone_guard = clone_lock.lock().unwrap_or_else(PoisonError::into_inner);

        // Refreshes the cached repository if it is already cloned, or clones it from one of `clone_urls` otherwise.
        if let Ok(Some(repo)) = self.open(addr) {
            if let Err(error) = repo.fetch() {
                log::warn!("failed to refresh the cached repository: {error:#}");
            }
            return Ok(repo);
        }

        // Create the parent directory if it does not exist.
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Remove the cached clone if it exists but is unusable.
        if path.exists() {
            log::warn!("removing unusable cached clone at {}", path.display());
            std::fs::remove_dir_all(&path)?;
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

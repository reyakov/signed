use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use signed_core::{Announcement, RepoAddr};

use crate::repo::Repo;

#[derive(Debug, Clone)]
pub struct GitCache {
    root: PathBuf,
}

impl GitCache {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn repo_path(&self, addr: &RepoAddr) -> PathBuf {
        self.root
            .join(addr.public_key().to_hex())
            .join(Self::sanitize_path_component(addr.identifier()))
    }

    pub fn open(&self, addr: &RepoAddr) -> Result<Option<Repo>> {
        let path = self.repo_path(addr);
        match gix::open(&path) {
            Ok(repo) => Ok(Some(Repo { inner: repo })),
            Err(gix::open::Error::NotARepository { .. }) => Ok(None),
            Err(gix::open::Error::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn ensure_clone<U: AsRef<str>>(&self, addr: &RepoAddr, clone_urls: &[U]) -> Result<Repo> {
        let path = self.repo_path(addr);

        if let Some(repo) = self.open(addr)? {
            repo.fetch().ok();
            return Ok(repo);
        }

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }

        Repo::clone(clone_urls, &path)
    }

    // `id` is untrusted relay content: it must never escape the cache root as
    // a single path component.
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

    pub fn fork_namespace(announcement: &Announcement) -> String {
        format!(
            "{}/{}",
            announcement.owner.to_hex(),
            Self::sanitize_path_component(&announcement.id)
        )
    }
}

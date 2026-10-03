use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Error;
use gpui::{App, AppContext, Context, Entity, Global, SharedString, Task};
use signed_core::{Announcement, RepoAddr};
use signed_git::{LocalRepo, Nip34Binding, find_git_repos};

struct GlobalLocalReposStore(Entity<LocalReposStore>);

impl Global for GlobalLocalReposStore {}

pub struct LocalReposStore {
    pub roots: Arc<Vec<PathBuf>>,
    pub repos: Arc<Vec<LocalRepo>>,
    pub scanning: bool,
    scan_dirty: bool,
}

impl LocalReposStore {
    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalLocalReposStore>().0.clone()
    }

    pub(crate) fn set_global(entity: Entity<Self>, cx: &mut App) {
        cx.set_global(GlobalLocalReposStore(entity));
    }

    pub fn new(roots: Vec<PathBuf>, cx: &mut Context<Self>) -> Self {
        let weak = cx.entity().downgrade();
        cx.defer(move |cx| {
            if let Err(error) = weak.update(cx, |this, cx| this.rescan(cx)) {
                log::warn!("local repos store dropped before initial scan could run: {error}");
            }
        });

        Self {
            roots: Arc::new(roots),
            repos: Arc::new(Vec::new()),
            scanning: false,
            scan_dirty: false,
        }
    }

    pub fn remove(&mut self, path: &Path, cx: &mut Context<Self>) {
        self.repos = Arc::new(
            self.repos
                .iter()
                .filter(|repo| repo.path.as_path() != path)
                .cloned()
                .collect(),
        );
        cx.notify();
    }

    pub fn rescan(&mut self, cx: &mut Context<Self>) {
        if self.scanning {
            self.scan_dirty = true;
            return;
        }

        if self.roots.is_empty() {
            return;
        }

        self.scanning = true;
        cx.notify();

        let roots = self.roots.clone();

        let work = cx.background_spawn(async move {
            let mut repos = Vec::new();
            for root in roots.iter() {
                repos.extend(find_git_repos(root));
            }
            repos.sort_by(|a, b| a.path.cmp(&b.path));
            repos.dedup_by(|a, b| a.path == b.path);
            repos
        });

        let task: Task<Result<(), Error>> = cx.spawn(async move |this, cx| {
            let repos = work.await;
            let again = this.update(cx, |this, cx| {
                this.repos = Arc::new(repos);
                this.scanning = false;
                cx.notify();

                let dirty = this.scan_dirty;
                this.scan_dirty = false;
                dirty
            })?;

            if again {
                this.update(cx, |this, cx| this.rescan(cx))?;
            }

            Ok(())
        });

        task.detach();
    }
}

pub fn local_repo_addr(repo: &LocalRepo) -> Option<RepoAddr> {
    let binding = repo.nip34.as_ref()?;
    let owner = binding.owner?;
    let identifier = binding.identifier.as_deref()?;

    Some(RepoAddr::new(owner, identifier))
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedLocalRepo {
    pub path: PathBuf,
    pub nip34: Option<Nip34Binding>,
    pub announcement: Option<Announcement>,
}

impl ResolvedLocalRepo {
    pub fn name(&self) -> SharedString {
        self.path
            .file_name()
            .map(|name| SharedString::from(name.to_string_lossy().into_owned()))
            .unwrap_or_else(|| SharedString::from("Untitled"))
    }
}

impl LocalReposStore {
    pub fn resolve(
        repos: &[LocalRepo],
        known: &[Announcement],
        own: &[Announcement],
    ) -> Vec<ResolvedLocalRepo> {
        let shown: HashSet<RepoAddr> = own.iter().map(Announcement::addr).collect();

        repos
            .iter()
            .filter_map(|repo| {
                let addr = local_repo_addr(repo);

                if let Some(addr) = &addr
                    && shown.contains(addr)
                {
                    return None;
                }

                let announcement = addr
                    .as_ref()
                    .and_then(|addr| {
                        known
                            .iter()
                            .find(|announcement| announcement.addr() == *addr)
                    })
                    .cloned();

                Some(ResolvedLocalRepo {
                    path: repo.path.clone(),
                    nip34: repo.nip34.clone(),
                    announcement,
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use nostr::prelude::*;
    use signed_git::{GraspSignals, Nip34Kind};

    use super::*;

    const KEY: &str = "0000000000000000000000000000000000000000000000000000000000000001";
    const OTHER_KEY: &str = "0000000000000000000000000000000000000000000000000000000000000002";

    fn announcement(secret: &str, id: &str) -> Announcement {
        let keys = Keys::new(SecretKey::from_hex(secret).expect("secret"));
        let tag = Tag::parse(vec!["d", id]).expect("tag");
        let event = EventBuilder::new(Kind::GitRepoAnnouncement, "")
            .tags(vec![tag])
            .finalize(&keys)
            .expect("signed");

        Announcement::from_event(&event).expect("parsed")
    }

    fn owner(secret: &str) -> PublicKey {
        Keys::new(SecretKey::from_hex(secret).expect("secret")).public_key()
    }

    fn bound(secret: &str, id: &str) -> LocalRepo {
        let binding = Nip34Binding {
            kind: Nip34Kind::Initialized,
            signals: GraspSignals {
                nip34_json: true,
                ..Default::default()
            },
            owner: Some(owner(secret)),
            identifier: Some(id.to_owned()),
            grasp_urls: Vec::new(),
        };

        LocalRepo {
            path: PathBuf::from(id),
            nip34: Some(binding),
        }
    }

    #[test]
    fn the_users_own_announcement_is_dropped() {
        let own = announcement(KEY, "mine");
        let repo = bound(KEY, "mine");

        let own = std::slice::from_ref(&own);
        assert!(LocalReposStore::resolve(&[repo], own, own).is_empty());
    }

    #[test]
    fn another_owners_announcement_is_linked_and_kept() {
        let known = announcement(OTHER_KEY, "theirs");
        let repo = bound(OTHER_KEY, "theirs");

        let resolved = LocalReposStore::resolve(&[repo], std::slice::from_ref(&known), &[]);

        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].announcement.as_ref(), Some(&known));
    }
}

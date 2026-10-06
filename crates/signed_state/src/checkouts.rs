use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Error;
use gpui::{App, AppContext, Context, Entity, Global, Subscription};
use nostr::prelude::*;
use settings::{CheckoutRecord, CheckoutsSettings, SettingsStore};
use signed_core::{Announcement, RepoAddr};
use signed_git::Repo;
use utils::same_repo_url;

use crate::backend::{Backend, BackendEvent};
use crate::git_store::Mirrors;
use crate::local_repos::LocalReposStore;
use crate::refresh::{RefreshGate, RefreshRequest};
use crate::repos::RepoListStore;

const REFRESH_DEBOUNCE: Duration = Duration::from_millis(300);
const MAX_STATUS_CHECKOUTS: usize = 8;
const LOCAL_POLL: Duration = Duration::from_secs(2);
const STATUS_POLL: Duration = Duration::from_secs(15);
const PUSH_POLL: Duration = Duration::from_secs(60);

struct GlobalCheckoutsStore(Entity<CheckoutsStore>);

impl Global for GlobalCheckoutsStore {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckoutStatus {
    pub path: PathBuf,
    /// A detached checkout is idle and yields no status.
    pub branch: String,
    /// For tip-based PR dedupe.
    pub head: String,
    /// `refs/remotes/origin/<branch>`, else `origin/HEAD` for new branches.
    pub base: String,
    /// Zero-ahead checkouts are dropped, so always above zero.
    pub ahead: u32,
}

struct Remembered {
    path: PathBuf,
    addr: RepoAddr,
    last_used: u64,
}

pub struct CheckoutsStore {
    by_repo: HashMap<RepoAddr, Vec<PathBuf>>,
    statuses: HashMap<RepoAddr, Vec<CheckoutStatus>>,
    status_requested: HashSet<RepoAddr>,
    push_requested: HashSet<RepoAddr>,
    push_statuses: HashMap<RepoAddr, Vec<CheckoutStatus>>,
    requested_head: HashMap<RepoAddr, Option<String>>,
    refresh: RefreshGate,
    debounce_pending: bool,
    local_pending: bool,
    last_full_sync: Option<Instant>,
    checkouts_settings: CheckoutsSettings,
    _subscriptions: Vec<Subscription>,
}

impl CheckoutsStore {
    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalCheckoutsStore>().0.clone()
    }

    pub(crate) fn set_global(entity: Entity<Self>, cx: &mut App) {
        cx.set_global(GlobalCheckoutsStore(entity));
    }

    pub fn new(cx: &mut Context<Self>) -> Self {
        let mut subscriptions = Vec::new();
        let mut checkouts_settings = CheckoutsSettings::default();

        if !cfg!(target_arch = "wasm32") {
            let settings = SettingsStore::global(cx);
            let local = LocalReposStore::global(cx);
            let repos = RepoListStore::global(cx);
            let backend = Backend::global(cx);

            checkouts_settings = settings.read(cx).settings().checkouts.clone();

            subscriptions.push(cx.observe(&settings, |this, settings, cx| {
                let checkouts = settings.read(cx).settings().checkouts.clone();
                if this.checkouts_settings == checkouts {
                    return;
                }
                // Only edits to the checkouts section affect the derived state
                this.checkouts_settings = checkouts;
                this.refresh(cx);
            }));

            subscriptions.push(cx.observe(&local, |this, _local, cx| {
                this.refresh(cx);
            }));

            subscriptions.push(cx.observe(&repos, |this, _repos, cx| {
                this.refresh(cx);
            }));

            // Another identity's repositories must not keep the old statuses alive.
            subscriptions.push(cx.subscribe(&backend, |this, _backend, event, cx| {
                if matches!(event, BackendEvent::SignerChanged) {
                    this.status_requested.clear();
                    this.push_requested.clear();
                    this.requested_head.clear();
                    this.statuses.clear();
                    this.push_statuses.clear();
                    cx.notify();

                    this.refresh(cx);
                }
            }));
        }

        if !cfg!(target_arch = "wasm32") {
            let weak = cx.entity().downgrade();
            cx.defer(move |cx| {
                if let Err(error) = weak.update(cx, |this, cx| this.refresh(cx)) {
                    log::warn!("checkouts store dropped before initial refresh could run: {error}");
                }
            });
        }

        Self {
            by_repo: HashMap::new(),
            statuses: HashMap::new(),
            status_requested: HashSet::new(),
            push_requested: HashSet::new(),
            push_statuses: HashMap::new(),
            requested_head: HashMap::new(),
            refresh: RefreshGate::default(),
            debounce_pending: false,
            local_pending: false,
            last_full_sync: None,
            checkouts_settings,
            _subscriptions: subscriptions,
        }
    }

    pub fn record(&mut self, path: PathBuf, addr: RepoAddr, cx: &mut Context<Self>) {
        if cfg!(target_arch = "wasm32") {
            return;
        }
        let last_used = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let addr_str = addr.to_string();

        let settings = SettingsStore::global(cx);
        settings.update(cx, |settings, cx| {
            settings.edit(
                |s| {
                    s.checkouts
                        .records
                        .retain(|r| !(r.path == path && r.addr == addr_str));
                    s.checkouts.records.push(CheckoutRecord {
                        path,
                        addr: addr_str,
                        last_used,
                    });
                },
                cx,
            );
        });
    }

    pub fn associations_of(&self, addr: &RepoAddr) -> Vec<PathBuf> {
        self.by_repo.get(addr).cloned().unwrap_or_default()
    }

    pub fn request_statuses(
        &mut self,
        addr: &RepoAddr,
        announced_head: Option<String>,
        cx: &mut Context<Self>,
    ) {
        self.status_requested.insert(addr.clone());
        if announced_head != self.requested_head.get(addr).cloned().flatten() {
            self.requested_head.insert(addr.clone(), announced_head);
        }
        self.refresh(cx);
    }

    pub fn ready_statuses_of(&self, addr: &RepoAddr) -> Vec<CheckoutStatus> {
        self.statuses.get(addr).cloned().unwrap_or_default()
    }

    pub fn request_push_statuses(&mut self, addr: &RepoAddr, cx: &mut Context<Self>) {
        self.push_requested.insert(addr.clone());
        self.refresh(cx);
    }

    pub fn checkout_pushed(&mut self, addr: &RepoAddr, path: &Path, cx: &mut Context<Self>) {
        let mut removed = false;

        if let Some(statuses) = self.push_statuses.get_mut(addr) {
            let before = statuses.len();
            statuses.retain(|status| status.path.as_path() != path);
            removed = statuses.len() != before;

            if removed && statuses.is_empty() {
                self.push_statuses.remove(addr);
            }
        }

        if removed {
            cx.notify();
        }

        self.request_push_statuses(addr, cx);
    }

    pub fn push_statuses_of(&self, addr: &RepoAddr) -> Vec<CheckoutStatus> {
        self.push_statuses.get(addr).cloned().unwrap_or_default()
    }

    pub fn unpushed(&self, addr: &RepoAddr) -> usize {
        self.push_statuses
            .get(addr)
            .map(|list| list.iter().map(|status| status.ahead as usize).sum())
            .unwrap_or(0)
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.debounce_pending || self.refresh.request() != RefreshRequest::Schedule {
            return;
        }

        self.debounce_pending = true;

        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(REFRESH_DEBOUNCE).await;
            this.update(cx, |this, cx| {
                this.run_refresh(cx);
            })
        })
        .detach();
    }

    fn run_refresh(&mut self, cx: &mut Context<Self>) {
        self.debounce_pending = false;
        self.refresh.begin();

        let records = {
            let settings = SettingsStore::global(cx);
            settings.read(cx).settings().checkouts.records.clone()
        };

        let remembered: Vec<Remembered> = records
            .into_iter()
            .filter_map(|record| {
                let addr = record.addr.parse::<RepoAddr>().ok()?;
                Some(Remembered {
                    path: record.path,
                    addr,
                    last_used: record.last_used,
                })
            })
            .collect();

        let announcements = RepoListStore::global(cx).read(cx).announcements.clone();
        let scanned = LocalReposStore::global(cx).read(cx).repos.clone();
        let cache_root = Mirrors::root().canonicalize().ok();

        let requested: Vec<(RepoAddr, Option<String>)> = self
            .status_requested
            .iter()
            .map(|addr| {
                (
                    addr.clone(),
                    self.requested_head.get(addr).cloned().flatten(),
                )
            })
            .collect();

        let push_requested: Vec<RepoAddr> = self.push_requested.iter().cloned().collect();
        let poll = !self.status_requested.is_empty() || !self.push_requested.is_empty();

        let work = cx.background_spawn(async move {
            let mut facts: Vec<(PathBuf, Option<String>, Option<String>)> = Vec::new();
            for scanned in scanned.iter() {
                let path = &scanned.path;

                // The browser's mirror clones are not user checkouts.
                if cache_root
                    .as_ref()
                    .is_some_and(|root| path.starts_with(root))
                {
                    continue;
                }

                let origin = Repo::open(path)
                    .and_then(|repo| repo.origin_url())
                    .ok()
                    .flatten();

                let root = Repo::open(path)
                    .and_then(|repo| repo.root_commit())
                    .ok()
                    .flatten();

                facts.push((path.clone(), origin, root));
            }

            let associations =
                CheckoutsStore::resolve_associations(&remembered, &facts, announcements.iter());

            let associations: HashMap<RepoAddr, Vec<PathBuf>> = associations
                .into_iter()
                .map(|(addr, paths)| (addr, paths.into_iter().filter(|p| p.is_dir()).collect()))
                .collect();

            let (statuses, push_statuses) =
                CheckoutsStore::compute_statuses(&associations, &requested, &push_requested, true);

            Ok::<_, Error>((associations, statuses, push_statuses))
        });

        cx.spawn(async move |this, cx| {
            let (associations, statuses, push_statuses) = match work.await {
                Ok(results) => results,
                Err(_) => {
                    return this.update(cx, |this, cx| {
                        this.refresh.abort();
                        if poll {
                            this.schedule_local_pass(cx);
                        }
                    });
                }
            };

            let again = this.update(cx, |this, cx| {
                let associations_changed = this.by_repo != associations;
                let statuses_changed = this.statuses != statuses;
                let push_statuses_changed = this.push_statuses != push_statuses;

                this.by_repo = associations;
                this.statuses = statuses;
                this.push_statuses = push_statuses;

                // Notify only when something actually changed.
                if associations_changed || statuses_changed || push_statuses_changed {
                    cx.notify();
                }

                this.last_full_sync = Some(Instant::now());
                this.refresh.finish()
            })?;

            if again {
                this.update(cx, |this, cx| {
                    this.refresh(cx);
                })?;
            }

            this.update(cx, |this, cx| {
                if poll {
                    this.schedule_local_pass(cx);
                }
            })?;

            Ok(())
        })
        .detach();
    }

    fn schedule_local_pass(&mut self, cx: &mut Context<Self>) {
        if self.local_pending {
            return;
        }
        self.local_pending = true;

        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(LOCAL_POLL).await;
            this.update(cx, |this, cx| {
                this.local_pending = false;
                this.local_tick(cx);
            })
        })
        .detach();
    }

    fn local_tick(&mut self, cx: &mut Context<Self>) {
        if self.status_requested.is_empty() && self.push_requested.is_empty() {
            return;
        }

        if self.refresh.running() || self.debounce_pending {
            self.schedule_local_pass(cx);
            return;
        }

        let cadence = if self.status_requested.is_empty() {
            PUSH_POLL
        } else {
            STATUS_POLL
        };

        let full_due = self
            .last_full_sync
            .is_none_or(|sync| sync.elapsed() >= cadence);

        if full_due {
            self.last_full_sync = Some(Instant::now());
            self.refresh(cx);
        } else {
            self.run_local_statuses(cx);
        }

        self.schedule_local_pass(cx);
    }

    fn run_local_statuses(&mut self, cx: &mut Context<Self>) {
        let associations = self.by_repo.clone();

        let requested: Vec<(RepoAddr, Option<String>)> = self
            .status_requested
            .iter()
            .map(|addr| {
                (
                    addr.clone(),
                    self.requested_head.get(addr).cloned().flatten(),
                )
            })
            .collect();

        let push_requested: Vec<RepoAddr> = self.push_requested.iter().cloned().collect();

        let work = cx.background_spawn(async move {
            let (statuses, push_statuses) =
                CheckoutsStore::compute_statuses(&associations, &requested, &push_requested, false);
            Ok::<_, Error>((statuses, push_statuses))
        });

        let task: gpui::Task<Result<(), Error>> = cx.spawn(async move |this, cx| {
            let Ok((statuses, push_statuses)) = work.await else {
                return Ok(());
            };

            this.update(cx, |this, cx| {
                if this.refresh.running() || this.debounce_pending {
                    return;
                }

                let statuses_changed = this.statuses != statuses;
                let push_statuses_changed = this.push_statuses != push_statuses;

                this.statuses = statuses;
                this.push_statuses = push_statuses;

                if statuses_changed || push_statuses_changed {
                    cx.notify();
                }
            })?;

            Ok(())
        });
        task.detach();
    }
}

impl CheckoutsStore {
    fn resolve_associations<'a>(
        remembered: &[Remembered],
        scanned: &[(PathBuf, Option<String>, Option<String>)],
        announcements: impl IntoIterator<Item = &'a Announcement>,
    ) -> HashMap<RepoAddr, Vec<PathBuf>> {
        let announcements: Vec<&Announcement> = announcements.into_iter().collect();
        let mut out: HashMap<RepoAddr, Vec<PathBuf>> = HashMap::new();

        let mut sorted: Vec<&Remembered> = remembered.iter().collect();
        sorted.sort_by_key(|record| std::cmp::Reverse(record.last_used));
        for record in sorted {
            let paths = out.entry(record.addr.clone()).or_default();
            if !paths.contains(&record.path) {
                paths.push(record.path.clone());
            }
        }

        for (path, origin, root) in scanned {
            for announcement in &announcements {
                let url_match = origin.as_deref().is_some_and(|origin| {
                    announcement
                        .clone
                        .iter()
                        .any(|url| same_repo_url(origin, url.as_str()))
                });

                let euc_match = root
                    .as_deref()
                    .is_some_and(|root| announcement.euc.as_deref() == Some(root));

                if url_match || euc_match {
                    let paths = out.entry(announcement.addr()).or_default();
                    if !paths.contains(path) {
                        paths.push(path.clone());
                    }
                }
            }
        }

        out
    }

    fn checkout_status(path: &Path, announced_head: Option<&str>) -> Option<CheckoutStatus> {
        let repo = Repo::try_open(path)?;
        let branches = repo.branches().ok()?;

        if branches.is_empty() || repo.is_dirty() {
            return None;
        }

        let branch = repo.current_branch()?;
        let head = repo.head()?;
        let base = announced_head
            .filter(|name| branches.iter().any(|b| b == name))
            .map(str::to_owned)
            .or_else(|| branches.iter().find(|b| *b == "main").cloned())
            .or_else(|| branches.first().cloned())?;

        if base == branch {
            return None;
        }

        let ahead = repo.commits_ahead(&base, &branch);
        (ahead > 0).then_some(CheckoutStatus {
            path: path.to_path_buf(),
            branch,
            head,
            base,
            ahead,
        })
    }

    // Never fetches the checked-out refs; reads the tracking refs as-is.
    fn checkout_push_status(path: &Path, fetch: bool) -> Option<CheckoutStatus> {
        let repo = Repo::try_open(path)?;
        if repo.is_dirty() {
            return None;
        }

        let branch = repo.current_branch()?;
        let head = repo.head()?;
        let origin = repo.origin_url().ok().flatten()?;

        if fetch {
            repo.fetch_refs(&[origin], "+refs/heads/*:refs/remotes/origin/*")
                .ok();
        }

        let remote = format!("refs/remotes/origin/{branch}");

        // A branch never fetched or pushed yet compares against the remote
        // HEAD, the fork point in practice.
        let base = if repo.ref_exists(&remote) {
            remote
        } else if repo.ref_exists("refs/remotes/origin/HEAD") {
            "refs/remotes/origin/HEAD".to_owned()
        } else {
            return None;
        };

        let ahead = repo.commits_ahead(&base, &branch);

        (ahead > 0).then_some(CheckoutStatus {
            path: path.to_path_buf(),
            branch,
            head,
            base,
            ahead,
        })
    }

    fn compute_statuses(
        associations: &HashMap<RepoAddr, Vec<PathBuf>>,
        requested: &[(RepoAddr, Option<String>)],
        push_requested: &[RepoAddr],
        fetch: bool,
    ) -> (
        HashMap<RepoAddr, Vec<CheckoutStatus>>,
        HashMap<RepoAddr, Vec<CheckoutStatus>>,
    ) {
        let mut statuses: HashMap<RepoAddr, Vec<CheckoutStatus>> = HashMap::new();
        for (addr, announced_head) in requested {
            let Some(paths) = associations.get(addr) else {
                continue;
            };

            let list: Vec<CheckoutStatus> = paths
                .iter()
                .take(MAX_STATUS_CHECKOUTS)
                .filter_map(|path| Self::checkout_status(path, announced_head.as_deref()))
                .collect();

            if !list.is_empty() {
                statuses.insert(addr.clone(), list);
            }
        }

        let mut push_statuses: HashMap<RepoAddr, Vec<CheckoutStatus>> = HashMap::new();
        for addr in push_requested {
            let Some(paths) = associations.get(addr) else {
                continue;
            };

            let list: Vec<CheckoutStatus> = paths
                .iter()
                .take(MAX_STATUS_CHECKOUTS)
                .filter_map(|path| Self::checkout_push_status(path, fetch))
                .collect();

            if !list.is_empty() {
                push_statuses.insert(addr.clone(), list);
            }
        }

        (statuses, push_statuses)
    }

    pub fn pr_proposes_checkout(
        pr: &Event,
        open: bool,
        user: PublicKey,
        checkout: &CheckoutStatus,
    ) -> bool {
        if pr.kind != Kind::GitPullRequest || !open || pr.pubkey != user {
            return false;
        }

        let branch_matches = pr
            .tags
            .iter()
            .find(|t| t.kind() == "branch-name")
            .and_then(|t| t.content())
            .is_some_and(|name| name == checkout.branch);

        // A renamed branch falls back to the proposed tip commit.
        let tip_matches = pr
            .tags
            .iter()
            .find(|t| t.kind() == "c")
            .and_then(|t| t.content())
            .is_some_and(|tip| tip == checkout.head);

        branch_matches || tip_matches
    }
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::*;

    #[test]
    fn same_repo_url_ignores_the_transport_scheme() {
        assert!(same_repo_url(
            "grasp://relay.ngit.dev/npub1test/repo",
            "https://relay.ngit.dev/npub1test/repo.git"
        ));
        assert!(same_repo_url(
            "ws://localhost:8080/npub1test/repo",
            "http://localhost:8080/npub1test/repo"
        ));
        assert!(!same_repo_url(
            "wss://localhost:8081/npub1test/repo",
            "wss://localhost:8080/npub1test/repo"
        ));
        assert!(!same_repo_url(
            "wss://host/npub1test/repo",
            "wss://host/npub1other/repo"
        ));
        assert!(same_repo_url("/local/path", "/local/path"));
        assert!(!same_repo_url("/local/path", "/local/other"));
    }

    #[test]
    fn checkout_status_reports_ahead_branches_only() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("repo");
        let _initial = Repo::init(&path, "My Repo", "").expect("init");
        let run = |args: &[&str]| {
            let status = Command::new("git")
                .current_dir(&path)
                .env("GIT_AUTHOR_NAME", "Test Author")
                .env("GIT_AUTHOR_EMAIL", "test@example.com")
                .env("GIT_COMMITTER_NAME", "Test Author")
                .env("GIT_COMMITTER_EMAIL", "test@example.com")
                .env("GIT_EDITOR", "true")
                .args(args)
                .status()
                .expect("git");
            assert!(status.success(), "git {args:?} failed");
        };
        let commit = |message: &str| {
            run(&["add", "-A"]);
            run(&["commit", "-m", message]);
        };

        run(&["checkout", "-b", "feature"]);
        std::fs::write(path.join("feature.txt"), "x\n").expect("write");
        commit("feature work");
        let status = CheckoutsStore::checkout_status(&path, Some("main")).expect("status");
        assert_eq!(status.branch, "feature");
        assert_eq!(status.base, "main");
        assert_eq!(status.ahead, 1);
        assert_eq!(status.head.len(), 40);

        std::fs::write(path.join("uncommitted.txt"), "y\n").expect("write");
        assert!(CheckoutsStore::checkout_status(&path, Some("main")).is_none());
        run(&["checkout", "--", "."]);

        run(&["checkout", "main"]);
        assert_eq!(CheckoutsStore::checkout_status(&path, Some("main")), None);
    }

    #[test]
    fn checkout_push_status_counts_unpushed_commits_only() {
        let dir = tempfile::tempdir().expect("tempdir");
        let remote = dir.path().join("remote");
        Repo::init(&remote, "My Repo", "").expect("init");
        let config = Command::new("git")
            .args(["config", "receive.denyCurrentBranch", "ignore"])
            .current_dir(&remote)
            .status()
            .expect("git config");
        assert!(config.success(), "git config failed");

        let checkout = dir.path().join("checkout");
        let status = Command::new("git")
            .args([
                "clone",
                "-q",
                remote.to_str().unwrap(),
                checkout.to_str().unwrap(),
            ])
            .status()
            .expect("git clone");
        assert!(status.success(), "git clone failed");

        let run = |args: &[&str]| {
            let status = Command::new("git")
                .current_dir(&checkout)
                .env("GIT_AUTHOR_NAME", "Test Author")
                .env("GIT_AUTHOR_EMAIL", "test@example.com")
                .env("GIT_COMMITTER_NAME", "Test Author")
                .env("GIT_COMMITTER_EMAIL", "test@example.com")
                .env("GIT_EDITOR", "true")
                .args(args)
                .status()
                .expect("git");
            assert!(status.success(), "git {args:?} failed");
        };

        // A fresh clone has nothing to push.
        assert_eq!(CheckoutsStore::checkout_push_status(&checkout, true), None);

        std::fs::write(checkout.join("work.txt"), "x\n").expect("write");
        run(&["add", "-A"]);
        run(&["commit", "-m", "local work"]);
        let status = CheckoutsStore::checkout_push_status(&checkout, true).expect("status");
        assert_eq!(status.branch, "main");
        assert_eq!(status.base, "refs/remotes/origin/main");
        assert_eq!(status.ahead, 1);
        assert_eq!(status.head.len(), 40);

        let local = CheckoutsStore::checkout_push_status(&checkout, false).expect("local status");
        assert_eq!(local.ahead, 1);

        run(&["push", "origin", "main"]);
        assert_eq!(CheckoutsStore::checkout_push_status(&checkout, true), None);

        let remote_run = |args: &[&str]| {
            let status = Command::new("git")
                .current_dir(&remote)
                .env("GIT_AUTHOR_NAME", "Other Author")
                .env("GIT_AUTHOR_EMAIL", "other@example.com")
                .env("GIT_COMMITTER_NAME", "Other Author")
                .env("GIT_COMMITTER_EMAIL", "other@example.com")
                .args(args)
                .status()
                .expect("git");
            assert!(status.success(), "git {args:?} failed");
        };
        std::fs::write(remote.join("other.txt"), "y\n").expect("write");
        remote_run(&["add", "-A"]);
        remote_run(&["commit", "-m", "remote work"]);
        assert_eq!(CheckoutsStore::checkout_push_status(&checkout, true), None);
    }
}

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Error;
use gpui::{App, AppContext, Context, Entity, Global, Subscription};
use nostr::prelude::*;
use settings::{CheckoutRecord, SettingsStore};
use signed_core::{Announcement, RepoAddr};

use crate::backend::{Backend, BackendEvent};
use crate::git_store::repo_mirror_root;
use crate::local_repos::LocalReposStore;
use crate::refresh::{RefreshGate, RefreshRequest};
use crate::repos::RepoListStore;

const REFRESH_DEBOUNCE: Duration = Duration::from_millis(300);

/// How often the statuses are recomputed against the local refs.
///
/// A commit lands in a checkout long before the remote reconciliation cadence,
/// so this fast pass surfaces ready-to-push and ready-to-contribute checkouts
/// within a second or two. It reads the tracking refs only, no network.
const LOCAL_POLL: Duration = Duration::from_secs(2);

/// How often a full pass refreshes the remotes while any repository panel is open.
const STATUS_POLL: Duration = Duration::from_secs(15);

/// Remote refresh interval for the `ready to push` badges of the user's own repositories.
const PUSH_POLL: Duration = Duration::from_secs(60);

const MAX_STATUS_CHECKOUTS: usize = 8;

struct GlobalCheckoutsStore(Entity<CheckoutsStore>);

impl Global for GlobalCheckoutsStore {}

/// One associated local checkout of a repository.
///
/// Carries the git facts needed to suggest a pull request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckoutStatus {
    pub path: PathBuf,
    /// The branch checked out. A detached checkout is idle and yields no status.
    pub branch: String,
    /// Commit the branch points at, for tip-based PR dedupe.
    pub head: String,
    /// What the branch is compared against.
    /// For ready-to-contribute statuses, the announced HEAD branch.
    /// The fallbacks are `main`, then the first local branch.
    /// For ready-to-push statuses, the remote-tracking ref.
    /// Unpushed commits are counted against it.
    ///
    /// It is `refs/remotes/origin/<branch>`, else `origin/HEAD` for new branches.
    pub base: String,
    /// Commits in `base..branch`.
    ///
    /// Zero-ahead checkouts are dropped, so this is always above zero.
    pub ahead: u32,
}

/// A remembered record, with the address already parsed.
struct Remembered {
    path: PathBuf,
    addr: RepoAddr,
    last_used: u64,
}

/// Global store of local-checkout associations and per-checkout statuses.
///
/// Readers (the sidebar rows, the repository panels) observe this store and
/// derive what they display from their own snapshots, so publishing needs no
/// fine-grained entities: the store notifies when a slice changed and each
/// reader re-derives only what it shows.
pub struct CheckoutsStore {
    /// Checkout paths per announced repository.
    by_repo: HashMap<RepoAddr, Vec<PathBuf>>,
    /// Ready-to-contribute statuses of the requested repositories.
    ///
    /// Those are the repository detail panels currently open.
    statuses: HashMap<RepoAddr, Vec<CheckoutStatus>>,
    /// Repositories whose statuses are recomputed on every input change.
    ///
    /// Those are the repository detail panels currently open.
    status_requested: HashSet<RepoAddr>,
    /// Repositories whose `ready to push` statuses are recomputed on the same cycle.
    ///
    /// The sidebar rows of the user's own repositories and their detail panels.
    push_requested: HashSet<RepoAddr>,
    /// The ready-to-push statuses of the requested own repositories.
    push_statuses: HashMap<RepoAddr, Vec<CheckoutStatus>>,
    /// Last announced head branch per requested repository.
    ///
    /// A recompute defaults the base the same way.
    requested_head: HashMap<RepoAddr, Option<String>>,
    refresh: RefreshGate,
    /// True while the timer between a scheduled refresh and its run is pending.
    debounce_pending: bool,
    local_pending: bool,
    /// When the last full pass (with a remote refresh) completed.
    ///
    /// The local pass runs a full pass again once this is older than the
    /// reconciliation cadence, so remote moves still land.
    last_full_sync: Option<Instant>,
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

        if !cfg!(target_arch = "wasm32") {
            let settings = SettingsStore::global(cx);
            let local = LocalReposStore::global(cx);
            let repos = RepoListStore::global(cx);
            let backend = Backend::global(cx);

            subscriptions.push(cx.observe(&settings, |this, _settings, cx| {
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

    /// The associated checkouts of `addr`, freshest first.
    ///
    /// Empty when none are known or the resolution has not run yet.
    pub fn associations_of(&self, addr: &RepoAddr) -> Vec<PathBuf> {
        self.by_repo.get(addr).cloned().unwrap_or_default()
    }

    /// Ask for the `ready to contribute` statuses of `addr` to stay current.
    /// Called while the repository's detail panel is open.
    ///
    /// `announced_head` is the announced HEAD branch, used to default the base.
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

    /// The ready-to-contribute statuses of `addr`.
    ///
    /// Empty while none are known or nothing is ahead.
    pub fn ready_statuses_of(&self, addr: &RepoAddr) -> Vec<CheckoutStatus> {
        self.statuses.get(addr).cloned().unwrap_or_default()
    }

    /// Ask for the `ready to push` statuses of `addr` to stay current.
    pub fn request_push_statuses(&mut self, addr: &RepoAddr, cx: &mut Context<Self>) {
        self.push_requested.insert(addr.clone());
        self.refresh(cx);
    }

    /// The checkout at `path` was just pushed to the remote.
    ///
    /// Its ready-to-push status is obsolete. Drop it from the cached statuses
    /// and notify observers right away, so the sidebar badge and the push
    /// banner update immediately instead of waiting for the next background
    /// pass, which re-scans and re-fetches the remote. The debounced refresh
    /// reconciles the remaining checkouts of the repository afterwards.
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

        // The other checkouts of this repository still need re-deriving
        // against the remote, now that the pushed refs landed there.
        self.request_push_statuses(addr, cx);
    }

    /// The ready-to-push statuses of `addr`.
    ///
    /// Empty while none are known or nothing is unpushed.
    pub fn push_statuses_of(&self, addr: &RepoAddr) -> Vec<CheckoutStatus> {
        self.push_statuses.get(addr).cloned().unwrap_or_default()
    }

    pub fn unpushed(&self, addr: &RepoAddr) -> usize {
        self.push_statuses
            .get(addr)
            .map(|list| list.iter().map(|status| status.ahead as usize).sum())
            .unwrap_or(0)
    }

    /// Re-resolve the associations and the requested statuses.
    ///
    /// Requests arriving while a pass runs fold into a follow-up, requests
    /// arriving while the debounce timer is pending are dropped.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.debounce_pending || self.refresh.request() != RefreshRequest::Schedule {
            return;
        }

        self.debounce_pending = true;

        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(REFRESH_DEBOUNCE).await;
            this.update(cx, |this, cx| this.run_refresh(cx))
        })
        .detach();
    }

    /// One full resolve and apply cycle, the debounced entry point.
    ///
    /// Re-resolves the associations from the settings, the scan and the
    /// announcements, then recomputes the requested statuses against freshly
    /// fetched remotes. Full passes run on every input change and on the
    /// remote reconciliation cadence ([`Self::local_tick`]); they also restart
    /// the fast local pass.
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
        let cache_root = repo_mirror_root().canonicalize().ok();

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
            // Read the git facts of every scanned repository off the main thread.
            //
            // The facts are the origin URL and the root commit, both CLI reads.
            let mut facts: Vec<(PathBuf, Option<String>, Option<String>)> = Vec::new();
            for scanned in scanned.iter() {
                let path = &scanned.path;

                // The browser's mirror clones share the announce URLs and EUCs. They are not user checkouts.
                if cache_root
                    .as_ref()
                    .is_some_and(|root| path.starts_with(root))
                {
                    continue;
                }
                let origin = signed_git::origin_url(path).ok().flatten();
                let root = signed_git::root_commit(path).ok().flatten();
                facts.push((path.clone(), origin, root));
            }

            let associations = resolve_associations(&remembered, &facts, announcements.iter());

            // Missing directories are stale records, drop them.
            let associations: HashMap<RepoAddr, Vec<PathBuf>> = associations
                .into_iter()
                .map(|(addr, paths)| (addr, paths.into_iter().filter(|p| p.is_dir()).collect()))
                .collect();

            let (statuses, push_statuses) =
                compute_statuses(&associations, &requested, &push_requested, true);

            Ok::<_, Error>((associations, statuses, push_statuses))
        });

        cx.spawn(async move |this, cx| {
            let (associations, statuses, push_statuses) = match work.await {
                Ok(results) => results,
                Err(_) => {
                    // Git reads are best-effort, keep the last results.
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

                // Notify only when something actually changed, so observers
                // skip the no-op heartbeats.
                if associations_changed || statuses_changed || push_statuses_changed {
                    cx.notify();
                }

                this.last_full_sync = Some(Instant::now());
                this.refresh.finish()
            })?;

            if again {
                this.update(cx, |this, cx| this.refresh(cx))?;
            }

            // Restart the fast local pass so the freshly resolved
            // associations drive it. The pass itself decides when the next
            // full pass runs.
            this.update(cx, |this, cx| {
                if poll {
                    this.schedule_local_pass(cx);
                }
            })?;

            Ok(())
        })
        .detach();
    }

    /// Schedule the fast local status pass, unless one is already pending.
    ///
    /// Every [`LOCAL_POLL`] the pass recomputes the requested statuses against
    /// the local refs, with no network, so a new commit in a checkout surfaces in
    /// a second or two instead of at the next remote reconciliation.
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

    /// The fast local status pass.
    ///
    /// Recomputes the statuses against the local refs; when the remote
    /// reconciliation cadence elapsed, it runs a full pass instead so pushes
    /// made elsewhere do not linger as `to push`.
    fn local_tick(&mut self, cx: &mut Context<Self>) {
        // Nothing watched: the chain idles out until a new request restarts it.
        if self.status_requested.is_empty() && self.push_requested.is_empty() {
            return;
        }

        // A full pass or a fresh request covers this tick, skip it.
        if self.refresh.running() || self.debounce_pending {
            self.schedule_local_pass(cx);
            return;
        }

        // Open panels get the faster remote cadence.
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

    /// Recompute the requested statuses against the tracking refs only.
    ///
    /// The refs were last refreshed by a full pass. Comparing against them is
    /// enough to pick up new local commits, and skipping the network keeps
    /// this pass cheap enough to run every [`LOCAL_POLL`].
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
                compute_statuses(&associations, &requested, &push_requested, false);
            Ok::<_, Error>((statuses, push_statuses))
        });

        let task: gpui::Task<Result<(), Error>> = cx.spawn(async move |this, cx| {
            let Ok((statuses, push_statuses)) = work.await else {
                // Git reads are best-effort, keep the last results.
                return Ok(());
            };

            this.update(cx, |this, cx| {
                // A full pass or a fresh request will apply fresher data
                // (the tracking refs move only when a full pass fetches).
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

fn url_identity(url: &str) -> Option<(String, Option<u16>, String)> {
    let parsed = Url::parse(url).ok()?;
    let host = parsed.host_str()?.to_ascii_lowercase();
    let mut path = parsed.path().trim_matches('/').to_owned();
    if let Some(stripped) = path.strip_suffix(".git") {
        path = stripped.to_owned();
    }
    Some((host, parsed.port(), path))
}

fn same_repo_url(a: &str, b: &str) -> bool {
    match (url_identity(a), url_identity(b)) {
        (Some(a), Some(b)) => a == b,
        _ => a == b,
    }
}

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
    let branches = signed_git::worktree_branches(path).ok()?;

    if branches.is_empty() || signed_git::worktree_dirty(path) {
        return None;
    }

    let branch = signed_git::worktree_current_branch(path)?;
    let head = signed_git::head_commit_id(path).ok().flatten()?;
    let base = announced_head
        .filter(|name| branches.iter().any(|b| b == name))
        .map(str::to_owned)
        .or_else(|| branches.iter().find(|b| *b == "main").cloned())
        .or_else(|| branches.first().cloned())?;

    if base == branch {
        return None;
    }

    let ahead = signed_git::worktree_commits_ahead(path, &base, &branch);
    (ahead > 0).then_some(CheckoutStatus {
        path: path.to_path_buf(),
        branch,
        head,
        base,
        ahead,
    })
}

/// The `ready to push` status of one checkout of the user's own repository.
///
/// `fetch` refreshes the remote heads first, so a full pass sees pushes made
/// elsewhere; the fast local pass skips it and compares against the tracking
/// refs left by the last full pass, which is enough to detect local commits.
fn checkout_push_status(path: &Path, fetch: bool) -> Option<CheckoutStatus> {
    if signed_git::worktree_dirty(path) {
        return None;
    }

    let branch = signed_git::worktree_current_branch(path)?;
    let head = signed_git::head_commit_id(path).ok().flatten()?;
    let origin = signed_git::origin_url(path).ok().flatten()?;

    if fetch {
        // Refresh the remote heads first.
        // Commits made elsewhere or pushed from another machine must not linger as `to push`.
        signed_git::fetch_repo_refs(path, &[origin], "+refs/heads/*:refs/remotes/origin/*").ok();
    }

    let remote = format!("refs/remotes/origin/{branch}");

    // A branch never fetched or pushed yet compares against the remote HEAD.
    // The remote HEAD is the fork point in practice.
    let base = if signed_git::worktree_ref_exists(path, &remote) {
        remote
    } else if signed_git::worktree_ref_exists(path, "refs/remotes/origin/HEAD") {
        "refs/remotes/origin/HEAD".to_owned()
    } else {
        return None;
    };

    let ahead = signed_git::worktree_commits_ahead(path, &base, &branch);

    (ahead > 0).then_some(CheckoutStatus {
        path: path.to_path_buf(),
        branch,
        head,
        base,
        ahead,
    })
}

/// Compute the requested statuses against the checkout paths of `associations`.
///
/// Shared by the full and the local pass. `fetch` refreshes the checkouts'
/// remote heads first, so the full pass sees remote moves; the fast local
/// pass reads the tracking refs only, which is enough to detect local commits.
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
            .filter_map(|path| checkout_status(path, announced_head.as_deref()))
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
            .filter_map(|path| checkout_push_status(path, fetch))
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

#[cfg(test)]
mod tests {
    use std::process::Command;

    use signed_core::{RepoAddr, repo_addr};

    use super::*;

    #[test]
    fn same_repo_url_ignores_the_transport_scheme() {
        // grasp announce vs https origin, with and without `.git`.
        assert!(same_repo_url(
            "grasp://relay.ngit.dev/npub1test/repo",
            "https://relay.ngit.dev/npub1test/repo.git"
        ));
        assert!(same_repo_url(
            "ws://localhost:8080/npub1test/repo",
            "http://localhost:8080/npub1test/repo"
        ));
        // The port and the path matter.
        assert!(!same_repo_url(
            "wss://localhost:8081/npub1test/repo",
            "wss://localhost:8080/npub1test/repo"
        ));
        assert!(!same_repo_url(
            "wss://host/npub1test/repo",
            "wss://host/npub1other/repo"
        ));
        // Unparseable URLs compare literally.
        assert!(same_repo_url("/local/path", "/local/path"));
        assert!(!same_repo_url("/local/path", "/local/other"));
    }

    fn remembered(path: &str, id: &str, last_used: u64) -> Remembered {
        Remembered {
            path: PathBuf::from(path),
            addr: addr(id),
            last_used,
        }
    }

    fn scanned(
        path: &str,
        origin: Option<&str>,
        root: Option<&str>,
    ) -> (PathBuf, Option<String>, Option<String>) {
        (
            PathBuf::from(path),
            origin.map(str::to_owned),
            root.map(str::to_owned),
        )
    }

    const KEY: &str = "0000000000000000000000000000000000000000000000000000000000000001";

    fn owner() -> PublicKey {
        Keys::new(SecretKey::from_hex(KEY).expect("secret")).public_key()
    }

    fn addr(id: &str) -> RepoAddr {
        repo_addr(owner(), id)
    }

    /// Build one announcement by the fixed test owner.
    /// Takes `clone` URLs and an EUC.
    fn announcement(id: &str, clones: &[&str], euc: Option<&str>) -> Announcement {
        let keys = Keys::new(SecretKey::from_hex(KEY).expect("secret"));
        let mut tags = vec![Tag::parse(vec!["d", id]).expect("tag")];
        for url in clones {
            tags.push(Tag::parse(vec!["clone", *url]).expect("tag"));
        }
        if let Some(euc) = euc {
            tags.push(Tag::parse(vec!["r", euc, "euc"]).expect("tag"));
        }
        let event = EventBuilder::new(Kind::GitRepoAnnouncement, "")
            .tags(tags)
            .finalize(&keys)
            .expect("signed");
        Announcement::from_event(&event).expect("parsed")
    }

    #[test]
    fn resolve_orders_remembered_freshest_first() {
        let announcements = vec![announcement("repo", &[], None)];
        let base = addr("repo");

        let resolved = resolve_associations(
            &[
                remembered("/old", "repo", 100),
                remembered("/fresh", "repo", 200),
                remembered("/other", "unrelated", 300),
            ],
            &[],
            &announcements,
        );

        let paths = resolved.get(&base).expect("associations");
        assert_eq!(paths, &vec![PathBuf::from("/fresh"), PathBuf::from("/old")]);
        // Records for repositories without announcements stay inert.
        assert_eq!(resolved.len(), 2);
    }

    #[test]
    fn resolve_deduplicates_paths_remembering_first() {
        let euc = "aa231c4c6a5777dc89b42207b499891a344add5c";
        let announcements = vec![announcement(
            "repo",
            &["https://host/npub1x/repo.git"],
            Some(euc),
        )];
        let base = addr("repo");

        // The same path is both remembered and scanned, its origin matches.
        // The remembered occurrence wins and the path is listed once.
        let resolved = resolve_associations(
            &[remembered("/shared", "repo", 100)],
            &[
                scanned("/shared", Some("https://host/npub1x/repo"), None),
                scanned("/scanned-only", Some("https://host/npub1x/repo.git"), None),
            ],
            &announcements,
        );

        let paths = resolved.get(&base).expect("associations");
        assert_eq!(
            paths,
            &vec![PathBuf::from("/shared"), PathBuf::from("/scanned-only")]
        );
    }

    #[test]
    fn checkout_status_reports_ahead_branches_only() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("repo");
        let _initial = signed_git::init_repository(&path, "My Repo", "").expect("init");
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

        // A feature branch ahead of main, ready to contribute.
        run(&["checkout", "-b", "feature"]);
        std::fs::write(path.join("feature.txt"), "x\n").expect("write");
        commit("feature work");
        let status = checkout_status(&path, Some("main")).expect("status");
        assert_eq!(status.branch, "feature");
        assert_eq!(status.base, "main");
        assert_eq!(status.ahead, 1);
        assert_eq!(status.head.len(), 40);

        // Dirty worktrees are never suggested.
        std::fs::write(path.join("uncommitted.txt"), "y\n").expect("write");
        assert!(checkout_status(&path, Some("main")).is_none());
        run(&["checkout", "--", "."]);

        // Even on main, nothing to propose.
        run(&["checkout", "main"]);
        assert_eq!(checkout_status(&path, Some("main")), None);
    }

    #[test]
    fn checkout_push_status_counts_unpushed_commits_only() {
        // The `grasp remote` is a plain repository the checkout clones from.
        // Its origin URL is a local path, so the whole cycle runs offline.
        // Git refuses pushes to a checked-out branch by default.
        // Act like a grasp server and allow them.
        let dir = tempfile::tempdir().expect("tempdir");
        let remote = dir.path().join("remote");
        signed_git::init_repository(&remote, "My Repo", "").expect("init");
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
        assert_eq!(checkout_push_status(&checkout, true), None);

        // One local commit, ready to push, counted against the remote.
        std::fs::write(checkout.join("work.txt"), "x\n").expect("write");
        run(&["add", "-A"]);
        run(&["commit", "-m", "local work"]);
        let status = checkout_push_status(&checkout, true).expect("status");
        assert_eq!(status.branch, "main");
        assert_eq!(status.base, "refs/remotes/origin/main");
        assert_eq!(status.ahead, 1);
        assert_eq!(status.head.len(), 40);

        // The local-only pass reads the tracking refs, no fetch needed:
        // a commit lands locally long before the remote is reconciled.
        let local = checkout_push_status(&checkout, false).expect("local status");
        assert_eq!(local.ahead, 1);

        // After the push the same commit is on the remote, idle again.
        run(&["push", "origin", "main"]);
        assert_eq!(checkout_push_status(&checkout, true), None);

        // A commit made by someone else on the remote must not count as local work.
        // It is behind, not ahead.
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
        assert_eq!(checkout_push_status(&checkout, true), None);
    }
}

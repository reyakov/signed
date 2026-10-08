# Nostr sync detection plan

Detect when a local repository has diverged from its NIP-34 state on Nostr
(kind 30618) and suggest syncing when the app opens.

Scope: repositories already initialized on Nostr (NIP-34 binding present).
Purely local repositories without a binding do not get this feature.

Scenario: the repository is initialized on Nostr and also pushed to Gitea.
Work continues locally and is pushed to Gitea only, so the Nostr state falls
behind. Signed should notice the difference on startup and offer to sync.

## Prior art

- nak (`git.go`): `nak git sync` fetches the latest kind 30617/30618 events
  from relays and writes them to local refs (`refs/nip34/state/*`); push and
  pull then compare against those refs with ordinary git machinery
  (e.g. `merge-base --is-ancestor` for fast-forward checks).
- ngit-cli: the same idea — the Nostr state is materialized as git refs in the
  repository, then ahead/behind comparison applies.

## Current state of the codebase

| Piece | Location | Status |
|---|---|---|
| NIP-34 binding detection | `Repo::nip34_binding()` in `signed_git/src/repo.rs` | done |
| Full Nostr state (refs + HEAD) | `RepoState::parse` in `signed_core/src/state.rs` | parsed, but `RepoStore` keeps only `head` and drops the refs |
| Push-to-Nostr flow | `Backend::push_repo_from` / `RepoStore::push_checkout` / `push_repository` | done; stages the state event on grasp relays, pushes objects, broadcasts 30618 |
| "Ahead of origin" detection | `CheckoutsStore::checkout_push_status` — local branch vs `refs/remotes/origin/<branch>` | exists, but that is local vs Gitea, not local vs Nostr |
| UI suggestion surfaces | Sidebar unpushed badge (`SidebarPanel::refresh_unpushed`), push banner (`RepoDetailView::push_suggestion` / `render_push_banner`) | pattern exists for origin; nothing for Nostr |

Gap: nothing compares the local repository's refs against the Nostr
`RepoState` refs, and no "sync to Nostr" suggestion is based on that
comparison.

## Plan

### Phase 1 — Divergence computation in `signed_git`

Add a pure function in a new `sync.rs` module that takes local `ref_state()`
and the Nostr `RepoState.refs` and classifies each `refs/heads/*` ref:

```rust
pub enum RefSync {
    InSync,
    LocalAhead { ahead: usize },            // fast-forwardable push
    RemoteAhead { behind: usize },          // Nostr has newer commits
    Diverged { ahead: usize, behind: usize },
    LocalOnly,   // branch never published
    RemoteOnly,  // branch exists only on Nostr
}

pub struct RepoSyncStatus {
    pub refs: Vec<(String, RefSync)>,  // branch name -> classification
    pub ahead_total: usize,            // summary for badges
    pub behind_total: usize,
}
```

Implemented with the existing `Repo::merge_base` and `commits_since` /
`commits_ahead` — no new dependencies.

### Phase 2 — Keep Nostr state refs and add a sync-status store in `signed_state`

1. `RepoStore`: store `state_refs: Vec<(String, String)>` (and the state event
   timestamp) alongside `head` in `run_refresh` instead of discarding them.
2. New `SyncStatusStore` (following the `CheckoutsStore` pattern: global
   entity, background compute, notify-on-change) that:
   - watches own announcements (`RepoListStore` + `Backend::current_user`;
     for repositories the user does not own, sync means PRs, which is out of
     scope);
   - reads the latest kind 30618 event per own repository from the Nostr LMDB
     (`RepoListStore::sync_own_repo_states` already ensures those events are
     synced);
   - for each associated checkout (`CheckoutsStore::associations_of`)
     computes `RepoSyncStatus` against the local refs on the background
     executor;
   - recomputes on app open, `BackendEvent::Synced` / `RepoUpdates` (state
     kind), after a successful push, and on a slow poll (~60s, like
     `PUSH_POLL`).

### Phase 3 — UI: suggest sync

- Sidebar: out-of-sync badge on own repository rows, reusing the `unpushed`
  badge pattern, visually distinct (e.g. "N commits not on Nostr" tooltip).
- RepoDetailView: banner modeled on `render_push_banner`: "main is 3 commits
  ahead of Nostr — Sync now", calling the existing `store.push_checkout(path)`.

## Open decisions

1. Badge placement: sidebar only, or sidebar plus repo-detail banner in the
   first pass. Recommendation: both — the banner hosts the action button, the
   badge drives the click.

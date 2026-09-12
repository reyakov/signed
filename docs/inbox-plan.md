# Inbox (home screen) implementation plan

Ported from GitWorkshop's home screen, the `Dashboard` rendered at route `/` for a logged-in user.

> **Correction to the first draft.** The first draft assumed the inbox was the `/notifications`
> page. It is not. GitWorkshop's `Index` route (`src/pages/Index.tsx`) renders `<Dashboard />` when
> an account is active, and that home screen is the inbox.

> **Status.** Phases 0-4 are implemented and green on `feat/inbox`, then the screen was redesigned to
> group **threads by repository** and to merge notifications with own activity into one row per thread
> (see the repository-grouping and thread-merge notes in §7).
> `cargo test -p signed_core` (69), `cargo test -p signed_state` (24),
> `cargo test -p workspace` (7), `cargo test -p dock` (1), `cargo clippy -p workspace --all-targets`
> clean, `cargo check --workspace --all-targets` succeeds.
> Phase 5 is not started. This document reflects the implementation as it stands: the Phase 1
> refactors, the §4.3 split of the inbox into a thin global `Inbox` and a panel-owned derivation, the
> Phase 4 click-through, and the repository-grouped thread list. The Phase 3 bottom-dock sub-views were
> removed before the redesign; their implementation notes in §7 are historical.

## 1. What the GitWorkshop home screen is

`Index.tsx`:

```tsx
if (account) return <Dashboard />;
return <LandingPage />;
```

`Dashboard.tsx` layout:

- Desktop: two columns.
  - **Left column**: `GreetingHeader`, `NotificationsPanel`, `RecentActivitySection`.
  - **Right column**: `MyRepositoriesPanel`, `AccessiblePrivateRepositoriesPanel`,
    `FollowedReposPanel`.
- Mobile: a single column in a different order.

The panel that gives the screen its inbox identity is `NotificationsPanel`:

- heading **Notifications** with a bell icon and an unread count badge,
- a **Mark all read** action and a **View all** link to `/notifications`,
- a compact list of the first 5 **non-archived** notification items,
- the empty state reads **"Your inbox is empty"** (with an `Inbox` icon).

So in GitWorkshop's vocabulary, "inbox" is the non-archived activity directed at you, surfaced
inline on the home screen. The home screen also shows your own recent activity and your repositories.

Data hooks:

| Section | Hook | What it loads |
|---|---|---|
| Notifications (inbox) | `useNotifications()` | Notification model: grouped thread activity directed at you, read/archived state |
| Continue where you left off | `useUserActivity(pubkey)` | Git activity authored by you: kinds 1621/1617/1618/1111 (git `K`)/1624/1630-1633, newest first, limit 50 |
| My repositories | `useUserRepositories(pubkey)` | Kind 30617 announcements authored by you |
| Followed repositories | `useUserFollowedRepos(pubkey)` | Repos you follow |
| Accessible private repositories | `useAccessiblePrivateRepositories()` | Private repos from CI/services |

## 2. Scope for Signed

| Priority | Section | Notes |
|---|---|---|
| **P0** | Inbox panel | Activity directed at you and your own activity, **grouped by repository**; unread badge; mark all read; all groups shown |
| **P1** | Click-through | Open the issue/PR detail panel at the relevant thread root |
| **P2 (defer)** | Standalone notifications page, NIP-65 relay discovery, pagination | Web-app concerns |
| **Out of scope** | Greeting header, my repositories, followed repositories, private repositories, pinned repositories, Unread/Archived sub-views | Not needed in Signed |

Notes:

- There is **no greeting header**. The screen starts with the inbox panel.
- There is **no My repositories column**. The sidebar already lists the signed-in user's repositories, so the inbox is a single column.
- The Unread/Archived sub-view panels were removed: the panel is a single repository-grouped list instead.

## 3. The Signed screen

`InboxView` is a center panel, opened by the sidebar's existing **Inbox** nav item. It is one bordered
card holding a single virtual list. Every row is either a **repository header** or one of that
repository's **threads**, newest first. A thread merges the notifications directed at the user with
the user's own events in the same root, and shows the root's title plus up to five of its most recent
events:
The sections are **all of the user's own repositories**, seeded from `RepoListStore`, plus any other
repository that has threads. Owned repositories with nothing to show render an
empty state ("No activity yet.") under their header, and sort after the ones with activity (newest
announcement first). Threads with no repository address fall into a single "Other repository" section.

```
+-------------------------------------------------------------------------+
| Inbox   (3 unread)                          [Mark all read]             |
|-------------------------------------------------------------------------|
| [repo] you/repo-a                                        (2)            |
|   [icon] Add retry logic                              (unread dot)     |
|     [avatar] You    opened an issue                    3d              |
|     [avatar] alice  commented                          2d              |
|   [icon] Fix flaky test                                                 |
|     [avatar] You    opened a PR                        1h              |
|-------------------------------------------------------------------------|
| [repo] you/repo-b                                                       |
|   No activity yet.                                                      |
|-------------------------------------------------------------------------|
| [repo] you/repo-c                                                       |
|   No activity yet.                                                      |
+-------------------------------------------------------------------------+
```

The sections are the repositories that actually have threads, ordered by their
newest row. A repository the user owns but that has no items is not shown. Threads with no repository
address fall into a single "Other repository" section.

## 4. Data layer

### 4.1 `signed_core`: pure logic

**`filters.rs`** (extend, next to `activity`/`comments_for`):

```rust
/// Kinds that notify a user when they tag them directly.
pub const NOTIFICATION_KINDS: [Kind; 9] = [
    Kind::GitIssue,
    Kind::GitPullRequest,
    Kind::GitPatch,
    Kind::GitPullRequestUpdate,
    COVER_NOTE_KIND,
    Kind::GitStatusOpen,
    Kind::GitStatusApplied,
    Kind::GitStatusClosed,
    Kind::GitStatusDraft,
];

/// Comments on our issues/PRs/patches.
pub fn notification_comments(me: PublicKey) -> Filter {
    Filter::new()
        .kind(Kind::Comment)
        .custom_tags(SingleLetterTag::UPPERCASE_P, [me.to_hex()])
        .custom_tags(SingleLetterTag::UPPERCASE_K, ["1621", "1617", "1618"])
}

/// Activity directed at us: comments on our roots, and git events tagging us.
pub fn notifications(me: PublicKey) -> Vec<Filter> {
    vec![
        notification_comments(me),
        Filter::new().kinds(NOTIFICATION_KINDS).pubkey(me),
    ]
}

/// Git activity authored by `me`, for "Continue where you left off".
pub fn authored_activity(me: PublicKey) -> Filter {
    Filter::new()
        .kinds([ACTIVITY_KINDS.as_slice(), &[COVER_NOTE_KIND]].concat())
        .author(me)
}
```

`ACTIVITY_KINDS` already exists in this file. All builders use existing SDK APIs
(`Filter::kind/kinds/pubkey/custom_tags`, `SingleLetterTag::{UPPERCASE_P, UPPERCASE_K}`).

Comments authored by `me` are not all git comments, so the activity query needs a post-filter:
keep kind 1111 only when its uppercase `K` tag is a git root kind (1621/1617/1618/30617), matching
gitworkshop's `isGitComment`.

**`inbox.rs`** (new file):

```rust
pub struct InboxItem {
    pub root: EventId,
    /// The root event itself, when known locally; drives the row title.
    pub root_event: Option<Event>,
    pub root_kind: Option<Kind>,
    pub address: Option<RepoAddr>,
    /// Notification events directed at the user, newest first.
    pub events: Vec<Event>,
    /// The user's own events in the same thread, newest first.
    pub own_events: Vec<Event>,
    /// Unread event ids, oldest first.
    pub unread_ids: Vec<EventId>,
    pub archived: bool,
}

impl InboxItem {
    /// Title of the thread root; falls back to the newest event it has.
    pub fn title(&self) -> String;
    /// Kind of the thread root; falls back to the newest event it has.
    pub fn kind(&self) -> Option<Kind>;
    pub fn latest_activity(&self) -> Timestamp;
    /// Up to `limit` most recent events of the thread, oldest first.
    pub fn timeline(&self, limit: usize) -> Vec<Event>;
    pub fn is_unread(&self) -> bool;
    pub fn apply_state(&mut self, state: &InboxReadState);
}

/// The thread root of a notification event, or `None` if it isn't git-related.
pub fn notification_root(
    event: &Event,
    lookup: &impl Fn(EventId) -> Option<Event>,
) -> Option<EventId>;

/// Group the notifications directed at the user together with the user's own
/// events into one item per thread, newest activity first.
pub fn group(
    events: impl IntoIterator<Item = Event>,
    own: impl IntoIterator<Item = Event>,
    me: PublicKey,
    state: &InboxReadState,
    lookup: &impl Fn(EventId) -> Option<Event>,
) -> Vec<InboxItem>;
```

Root resolution, ported from `getNotificationRootId`:

- issue (1621) / PR (1618): itself
- patch (1617): its `e` parent patch, else itself
- NIP-22 comment (1111): uppercase `E` root pointer (SDK `nip22::extract_root`)
- PR update (1619): uppercase `E`
- statuses (1630-1633) / cover note (1624): NIP-10 root `e`
- notification events authored by `me` are dropped; the user's own events are kept in
  `own_events` instead, never in `events`
- `unread_ids` and `archived` are derived from `events` only, so the user's own activity is never
  unread and a thread with only own events is never archived

Read/archive state, the compact high-water-mark model:

```rust
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct InboxReadState {
    #[serde(default)] pub read_before: Timestamp,
    #[serde(default)] pub read_ids: HashSet<EventId>,
    #[serde(default)] pub archived_before: Timestamp,
    #[serde(default)] pub archived_ids: HashSet<EventId>,
}

impl InboxReadState {
    pub fn is_read(&self, event: &Event) -> bool;
    pub fn is_archived(&self, event: &Event) -> bool;
    pub fn mark_read(&mut self, event: &Event);
    pub fn mark_all_read(&mut self, all: &[Event], me: PublicKey);
    /// Move the cutoff to `min(oldest unread - 1, now - 3 days)` and prune ids.
    pub fn advance_read(&mut self, all: &[Event], me: PublicKey);
    pub fn advance_archived(&mut self, all: &[Event], me: PublicKey);
}
```

`activity_subject` in `model.rs` already gives an issue/PR title from the `subject` tag or first
line; reuse it for the home screen rows.

### 4.2 Persistence: NIP-78 in the local database, never published

Read state is a normal NIP-78 (kind `30078`, `Kind::ApplicationSpecificData`) addressable event
**written to LMDB only**. It is never broadcast to a relay, so the read state stays on this device.

It is signed with a **random keypair**, never the user's signer. The event is local application
storage, so its author carries no identity; this avoids a signing round-trip and does not depend on
the signer type. The `d` tag identifies the owning user, so state does not leak across identities
when the signed-in key changes.

```rust
/// d tag identifying the inbox read/archive state event of `me`.
fn inbox_state_d_tag(me: PublicKey) -> String {
    format!("signed-inbox-state:{}", me.to_hex())
}

/// Newest stored read state for `me`.
async fn load_state(client: &Client, me: PublicKey) -> Result<Option<InboxReadState>, Error> {
    // No author filter: the signing key is random per save.
    let filter = Filter::new()
        .kind(Kind::ApplicationSpecificData)
        .identifier(inbox_state_d_tag(me));

    let events = client.database().query(filter).await?;

    let Some(event) = events.into_iter().max_by_key(|event| event.created_at) else {
        return Ok(None);
    };

    match serde_json::from_str(&event.content) {
        Ok(state) => Ok(Some(state)),
        Err(error) => {
            log::warn!("ignoring unreadable inbox state {}: {error}", event.id);
            Ok(None)
        }
    }
}

/// Sign with a fresh random key and store locally.
async fn save_state(client: &Client, me: PublicKey, state: &InboxReadState) -> Result<(), Error> {
    let event = EventBuilder::new(Kind::ApplicationSpecificData, serde_json::to_string(state)?)
        .tags([Tag::identifier(inbox_state_d_tag(me))])
        .finalize(&Keys::generate())?; // synchronous: random key, no user signer
    // Local-only: no `send_event`, no broadcast. The event lives in LMDB.
    client.database().save_event(&event).await?;
    Ok(())
}
```

A fresh random key is generated on every save, so each save writes a new event rather than
replacing the previous one. LMDB only auto-replaces an addressable event when the incoming event
has the **same pubkey**, so old copies accumulate. Nothing prunes them; `load_state` reads the
newest by `created_at`, so the behavior is correct. This is a deliberate trade for not caching a
key in the store (see §4.3). An earlier implementation deleted the previous event by tracking its
id across saves; that was removed as more derived state than it was worth.
`NostrDatabase::{save_event, query}` and `Client::database()` are existing SDK APIs.

### 4.3 Data layer: a thin global `Inbox`, a panel-owned derivation

The inbox is split in two, because the expensive derivation is only needed while the home screen is
open.

**`Inbox`** is a child `Entity<Inbox>` owned by `Backend` (`inbox: Entity<Inbox>`) and is
deliberately thin: it owns only the read/archive state that must outlive the panel and the NIP-78
load/save.

```rust
// backend.rs
pub struct Backend {
    ...
    inbox: Entity<Inbox>,
}

// inbox.rs
#[derive(Default)]
pub struct Inbox {
    state: InboxReadState,
    loaded: bool,
}

impl Inbox {
    pub fn state(&self) -> &InboxReadState;
    pub fn is_loaded(&self) -> bool;
    pub fn mark_read(&mut self, group: &[Event], all: &[Event], me: PublicKey, cx);
    pub fn mark_archived(&mut self, group: &[Event], all: &[Event], me: PublicKey, cx);
    pub fn mark_all_read(&mut self, all: &[Event], me: PublicKey, cx);
    pub(crate) fn activate(&mut self, me: PublicKey, client: Client, cx);
    pub(crate) fn reset(&mut self, cx);
}

// inbox.rs (signed_state)
/// One item per thread, notifications and own activity merged.
pub async fn query_inbox(
    client: &Client,
    me: PublicKey,
    state: &InboxReadState,
) -> Result<(Vec<InboxItem>, usize), Error>;
```

**The panel owns the derivation.** `InboxView` itself holds the derived lists, the copy of the read
state they were computed with, and the refresh coalescing. There is no separate store entity: the
panel is the only consumer, so an `Entity<InboxStore>` would add an `update` indirection and a
forwarding subscription without buying any sharing. The panel's own `unread_count` feeds its header
badge only; there is no global count and no sidebar badge.

```rust
pub struct InboxView {
    focus_handle: FocusHandle,
    dock_area: WeakEntity<DockArea>,
    threads: Arc<Vec<InboxItem>>,       // one row per thread, merged
    sections: Arc<Vec<InboxSection>>,   // grouped by repository
    rows: Arc<Vec<InboxRow>>,           // flattened list
    unread_count: usize,
    state: InboxReadState,
    state_loaded: bool,
    refresh: RefreshGate,
    list: ListState,
    _subscriptions: Vec<Subscription>,
}

impl InboxView {
    pub fn new(dock_area: WeakEntity<DockArea>, cx: &mut Context<Self>);  // cx.defer(… sync_state)
    pub fn sync_state(&mut self, cx);                     // observes the global Inbox
    pub fn mark_all_read(&mut self, cx);
    fn handle_backend_event(&mut self, event: &BackendEvent, cx);
    fn refresh(&mut self, cx);
    fn run_refresh(&mut self, cx);
    fn regroup(&mut self, cx);                            // re-apply read state
    fn rebuild(&mut self, cx);                            // group by repository, seed owned, flatten
    fn clear(&mut self);
}
```

The panel owns three subscriptions that carry logic: it observes the global `Inbox`
(`InboxView::sync_state`), subscribes to `Backend` (`InboxView::handle_backend_event`), and observes
`RepoListStore` to rebuild when the user's own repositories load. Re-rendering itself needs no
subscription: GPUI invalidates a window for every entity it read during render, so the panel tracks
`RepoListStore` and `ProfileStore` just by reading them in `render`. The panel does not write back
to the global.

**`signed_state::query_inbox`.** The database work stays in `signed_state`, so the UI crate never
queries LMDB directly. `query_inbox` returns the grouped notifications, the user's own git
activity and the unread count; the panel applies the results on the main thread. `RefreshGate` is
re-exported for the panel's debounce.

```rust
pub async fn query_inbox(
    client: &Client,
    me: PublicKey,
    state: &InboxReadState,
) -> Result<(Vec<InboxItem>, Vec<Event>, usize), Error>;
```

**Lifespan.** `Inbox` is created with the backend but idles until the user has a signer. The
derived lists live only as long as the panel. Nothing is wired from the `desktop` crate and
`signed_state::init` gains no parameters.

**No sidebar badge.** The sidebar's inbox nav item has no unread suffix (an earlier global count
derivation was removed with it). The unread count lives entirely in the panel, which shows it in
its header and per repository section. The trade-off is that the count is only current while the
panel is open, which is acceptable now that nothing outside it displays one.

The dependency chain is `Backend` → `Inbox` and `InboxView` → `query_inbox`.

`Backend` owns the inbox lifecycle (`sync_inbox`); the panel subscribes to `Backend` directly for
its lists. `BackendEvent::SignerChanged` and `SignerRequired` are still emitted and must stay:
`CheckoutsStore` and `SidebarPanel` consume them. They no longer drive the inbox's activation
directly.

`InboxView::handle_backend_event` refreshes on:

- `NostrUpdate(updates)`: when any update kind is in `NOTIFICATION_KINDS`, is `Kind::Comment`, or is
  a deletion (`EventDeletion` / `RequestToVanish`).
- `Synced` / `Published`.
- everything else: ignored.

**Signer lifecycle: `Backend::sync_inbox`.** `Backend` owns the wiring and calls `sync_inbox` from the
three real signer transitions: `create_identity`, `set_signer` (nsec, bunker and passphrase restore)
and `logout`. It starts the subscriptions and repo-relay connects, then calls `Inbox::activate` or
`Inbox::reset`. The client is passed into `activate`, so the global inbox never reads `Backend`
while `sync_inbox` is mid-update:

```rust
fn sync_inbox(&mut self, cx: &mut Context<Self>) {
    let me = self.current_user;

    if let Some(me) = me {
        self.subscribe_bootstrap(filters::notifications(me), cx);
        self.subscribe_bootstrap(vec![filters::authored_activity(me)], cx);

        let relays: HashSet<RelayUrl> = RepoListStore::global(cx)
            .read(cx)
            .announcements_of(&me)
            .into_iter()
            .flat_map(|announcement| announcement.relays)
            .collect();

        if !relays.is_empty() {
            let relays: Vec<RelayUrl> = relays.into_iter().collect();
            self.connect_repo_relays(relays.clone(), filters::notifications(me), cx);
            self.connect_repo_relays(relays, vec![filters::authored_activity(me)], cx);
        }
    }

    let client = self.client.clone();
    self.inbox.update(cx, |inbox, cx| match me {
        Some(me) => inbox.activate(me, client, cx),
        None => inbox.reset(cx),
    });
}
```

The repo relays are read from `RepoListStore::global(cx).read(cx).announcements_of(&me)` at call
time and never cached. (NIP-65 outbox relay discovery is deferred; Signed does not fetch kind
10002 yet.) `Inbox::activate` and `Inbox::reset` are `pub(crate)`; `Inbox` has no `subscribe_remote`
/ `connect_own_repo_relays`.

**Activation** clears the state and loads the NIP-78 state from LMDB. The panel clears its own
lists and in-flight refresh when it sees the unloaded state, then refreshes once it is loaded:

```rust
pub(crate) fn activate(&mut self, me: PublicKey, client: Client, cx: &mut Context<Self>) {
    // state = default; state_loaded = false; cx.notify();
    // spawn load_state(client, me), then set state and state_loaded = true
}
```

`reset` performs the same clearing without a state load, and is used on logout.

Reading the state event needs no signer at all (the `d` tag carries the identity); activation is
still gated on the signer because the fetch filters need the user's pubkey.

**Fetch** reuses `Backend::subscribe_bootstrap` and `Backend::connect_repo_relays` through
`Backend::sync_inbox` (see above). The query the panel runs is intentionally the offline-first cache
read, not a wait on the network; see the note below.

**Refresh** (`InboxView::run_refresh`, mirrors `RepoListStore::run_refresh`):

- `cx.background_spawn`: query the notification filters and the activity filter from
  `client.database()`.
- Query `filters::deletions()`, build `Deletions`, skip deleted events.
- Build `HashMap<EventId, Event>` for root walking; group the notification events with
  `inbox::group`.
- Filter the activity events: keep issues/PRs/patches/statuses/cover notes, and comments only when
  their `K` tag is a git kind; sort newest first.
- Cross back to the main thread: guard on `Backend::global(cx).read(cx).current_user() ==
  Some(me)`; if the signer changed while the query ran, `refresh.abort()` instead of applying, so a
  previous user's results never land. Then set `notifications`, `activity`, `unread_count`, rebuild the
  repository sections (`rebuild`), `cx.notify()`, `refresh.finish()`.

`InboxView::sync_state` reacts to the global `Inbox`: while the state is not loaded it clears the
lists, on the first load it runs the initial refresh, and on a state change (a mark action) it
re-derives the flags (`InboxItem::apply_state`).

**Fetch vs. the immediate query.** `subscribe_bootstrap` / `connect_repo_relays` return immediately,
so the query that follows them reads the local cache rather than waiting for the relays. That is
deliberate offline-first behavior: cached content appears at once on a warm start and with no
network, instead of blocking the home screen on the network. The gap is closed by the SDK, not by
timing: received events are written to LMDB and surfaced as `ClientNotification::Event`, so
`Backend`'s pump batches them into `BackendEvent::NostrUpdate` and the store refreshes. This was
reviewed and left as-is.

**Actions**: `mark_all_read()` lives on the panel, which passes every known notification event to the
global `Inbox`. The global marks them, advances the cutoffs against *all* notification events to bound
the id sets, saves the state to LMDB (signed with a fresh random key, see 4.2), and notifies. The
panel then re-derives and publishes the unread count.

**Repository names need no new store**: `RepoListStore` already holds every announcement and
`repo_name` resolves an address to a display name.

### 4.4 `Cargo.toml`

- `signed_core`: add `serde.workspace` for the `InboxReadState` derives.
- `signed_state`: add `serde_json.workspace` for the NIP-78 content.

## 5. UI

### 5.1 `InboxView` center panel

`crates/workspace/src/views/inbox.rs`, a `BasePanel` + `Panel` + `Render`, like `RepoListView`.
It owns the derived lists directly, so `cx.notify()` from an update re-renders it. The panel is one
bordered card (`flex_1`, `min_h_0`) with a header bar and a scrolling body. The body is a single
`gpui::list` virtual list (`ListState` + `ListAlignment::Top`, 400px overdraw) with a
`vertical_scrollbar`; the panel itself does not scroll, so the list gets a definite viewport height.
The list count is reset from `render` whenever the rendered row count changes.

- **Header**: the unread count badge and **Mark all read**.
- **Body**: the flattened repository-grouped rows. A repository header is a muted bar with a git icon,
  the repository name (or "Other repository" when the address is unknown) and its unread badge. Rows
  under it show the actor avatar, a kind icon, the subject, the kind label, a relative time, and an
  unread dot (the subject is semibold while unread). A repository with nothing to show renders
  "No activity yet."; the panel-level "You're all caught up." empty state appears only when there are
  no sections at all (no owned repositories and no items).

No greeting header, and no **My repositories** column - the sidebar already lists the user's
repositories.

### 5.2 Grouping by repository

The grouping is panel-owned derivation, done once per data change in `InboxView::rebuild` (called
from `run_refresh` and `regroup`), never per frame:

```rust
struct InboxSection {
    address: Option<RepoAddr>,   // repository, None for items without one
    unread: usize,               // unread notification groups
    entries: Vec<InboxEntry>,    // newest first
    latest: Timestamp,           // orders the sections
}

enum InboxEntry {                // indices into the panel's own lists
    Notification(usize),
    Activity(usize),
}

enum InboxRow {                  // the flattened list
    Repo(usize),
    Entry(usize, usize),
    Empty,                       // "No activity yet." under an empty section
}
```

The section list and the flattened rows are stored as `Arc`s and cloned into the `gpui::list`
closure, which indexes the panel's `notifications` / `activity` lists - no per-frame deep copies.
Notification groups carry their repository in `InboxItem::address`; activity events carry it in a
`GitRepoAnnouncement` `a` tag (`repo_address`). Archived notification groups are left out.

`rebuild` also seeds a section for every repository in `announcements_of(me)`. The panel observes
`RepoListStore` so a repository that loads after the last refresh still appears (its own empty
section, or with items if any arrived); this is the one logic subscription beyond the `Inbox` and
`Backend` ones. Repository names are resolved per render through `repo_name` -> `RepoListStore`, so a
late announcement still labels its section without re-deriving the grouping.

### 5.3 Sidebar

In `views/sidebar/mod.rs`:

- Add `inbox: Option<WeakEntity<InboxView>>` (mirrors `explore`).
- Add `fn open_inbox(&mut self, window, cx)` that returns when the panel is already open, else adds
  a center panel (same shape as `open_explore`; there is no dock API to focus an existing tab).
  `InboxView::new` takes the sidebar's `WeakEntity<DockArea>` so the panel can open a repo for a row.
- Point the existing nav item at it:

  ```rust
  NavItem::new("inbox", "Inbox", Icon::new(IconName::Inbox).small())
      .on_click(cx.listener(|this, _ev, window, cx| this.open_inbox(window, cx))),
  ```

- No unread badge. The nav item carries no suffix, and the sidebar does not observe the global
  `Inbox`. The unread count lives in the panel only.

### 5.4 Click-through (P1)

The detail panels need a `Window`, and GPUI's `Entity::update_in` only exists on a `VisualContext`,
which a synchronous `App` + `Window` pair is not - so the entry point is a free function rather than
a `RepoDetailView::open_item` method. In `repo_detail/mod.rs`:

```rust
pub(crate) enum RepoItem {
    Issue(EventId),
    PullRequest(EventId),
    Patch,
}

pub(crate) fn open_repo_item(
    dock_area: &WeakEntity<DockArea>,
    announcement: &Announcement,
    item: RepoItem,
    window: &mut Window,
    cx: &mut App,
) { /* build the RepoStore here, then a new IssueDetailView / PullRequestDetailView, added to the center */ }
```

- `open_repo_item` builds its own `RepoStore` from `announcement` (a private `repo_store` helper calls
  `RepoStore::new(addr, relays, cx)`), so the item panel is the **only** panel docked. An earlier
  version opened `RepoDetailView` first and reused its store via `RepoDetailView::store()`; that
  docked the repository panel too, which surfaced the repository load state (a `not found` error for
  an announced repo with no local worktree) and left two center tabs. `RepoDetailView::store()` was
  removed with it.
- `views/mod.rs` re-exports `RepoItem` and `open_repo_item`.
- `InboxView::open_item` resolves `item.address` to an `Announcement` from `RepoListStore`, and calls
  `open_repo_item` with the root id and kind. The detail panel renders a "not found" placeholder
  until the store's fetch lands, then re-renders.
- The item panel is added to the center group and activated.

Patches have no detail view in Signed (they are only consumed inside `PullRequestDetailView`), so a
patch-root click opens nothing. `RepoItem::Patch` carries no id for that reason. A group whose root is
not an issue/PR/patch, or whose repository is not in `RepoListStore`, opens nothing.

## 6. File-by-file change list

| File | Change |
|---|---|
| `crates/signed_core/Cargo.toml` | add `serde` |
| `crates/signed_core/src/filters.rs` | `NOTIFICATION_KINDS`, `notification_comments`, `notifications`, `authored_activity`, `is_git_activity`, `deletions` |
| `crates/signed_core/src/inbox.rs` | **new**: `InboxItem` (root event, notifications, own events), `notification_root`, `group`, `InboxReadState`, tests |
| `crates/signed_core/src/lib.rs` | `mod inbox;` and re-exports |
| `crates/signed_state/Cargo.toml` | add `serde_json` |
| `crates/signed_state/src/inbox.rs` | thin global `Inbox` (NIP-78 read state, mark actions) and `query_inbox` (query, merge notifications + activity into threads) |
| `crates/signed_state/src/backend.rs` | `inbox: Entity<Inbox>` field, construction, `inbox()` accessor, `sync_inbox`, `RepoListStore` import |
| `crates/signed_state/src/refresh.rs` | doc comment lists `Inbox` among the `RefreshGate` users |
| `crates/signed_state/src/lib.rs` | `mod inbox;`, re-export `Inbox` and `query_inbox`; re-export `RefreshGate` (no global install) |
| `crates/dock/src/lib.rs` | `add_bottom_panel` helper (currently unused; left over from the removed sub-views) |
| `crates/workspace/src/views/inbox.rs` | `InboxView` home panel owning the threads, the repository grouping, and the thread click-through |
| `crates/workspace/src/views/mod.rs` | `mod inbox; pub use inbox::InboxView;`; re-export `RepoItem`, `open_repo_item`, `open_repo_panel` |
| `crates/workspace/src/views/sidebar/mod.rs` | `inbox` field, `open_inbox`, nav wiring |
| `crates/workspace/src/views/repo_detail/mod.rs` | `RepoItem`, `open_repo_item` (builds its own `RepoStore` via the private `repo_store` helper) |

No changes to `desktop` or `signed_nostr`. `signed_state::init` gains no parameters; `Backend::sync_inbox`
activates the `Inbox` child entity at each signer transition.

## 7. Phasing

1. **Phase 0 - pure logic**: `signed_core` filters and `inbox.rs` plus tests. **DONE.**
   Implemented as `filters::{NOTIFICATION_KINDS, notification_comments, notifications, authored_activity, is_git_activity}`
   and `inbox::{InboxItem, notification_root, group, InboxReadState}`. Two deviations from the sketch:
   the cutoff methods take an explicit `now: Timestamp` so the pure logic stays deterministic and testable,
   and `authored_activity` results must pass through `is_git_activity` before display (comments on
   non-git roots are matched by the filter). `cargo test -p signed_core` passes (66 tests at the
   end of Phase 0; 68 after the two Phase 1 additions).
2. **Phase 1 - store**: `Inbox` child entity, activated by `Backend::sync_inbox` once a signer
   exists; both queries, unread count, and NIP-78 load/save to LMDB. **DONE.** See the
   implementation notes below.
3. **Phase 2 - screen**: `InboxView` (inbox + activity) and the sidebar nav item.
   **DONE.** See the implementation notes below.
4. **Phase 3 - sub-views**: `add_bottom_panel` and `InboxFilterView` for Unread / Archived.
   **Done, then reverted.** The sub-views were removed before the repository-grouping redesign; the
   notes below are historical.
5. **Phase 4 - click-through**: `open_item` and announcement lookup. **DONE.** See the implementation
   notes below.
6. **Phase 5 (optional)**: standalone notifications page, NIP-65 relays, pagination, patch detail
   view.

Each phase compiles and is usable on its own.

### Phase 1 implementation notes

Files: `crates/signed_state/{Cargo.toml, src/inbox.rs, src/lib.rs, src/backend.rs, src/refresh.rs}`
and two additions to `crates/signed_core/src/inbox.rs`.

- `Inbox` is a child entity of `Backend` (`inbox: Entity<Inbox>`), created in `Backend::new` and
  reached via `Backend::inbox()`. Nothing in `desktop` is wired and `signed_state::init` gains no
  parameters. The dependency is strictly one-way: `Inbox` holds no `Backend` handle.
- `Backend::emit` is the single funnel for every `BackendEvent`. It updates the inbox through
  `cx.defer` and then emits to the other subscribers. The defer is required: every emit site runs
  inside `Backend::update`, and the inbox handlers read `Backend`, so a synchronous call panics on
  a re-entrant entity access.
- The signer lifecycle lives in `Backend::sync_inbox`, called from `create_identity`, `set_signer`
  and `logout`. It starts the subscriptions and repo-relay connects, then defers `inbox.activate`
  / `inbox.reset`. `SignerChanged` / `SignerRequired` are still emitted for `CheckoutsStore` and
  `SidebarPanel`, but no longer drive the inbox.
- `Inbox` mirrors `RepoListStore`: `RefreshGate` coalescing, `cx.background_spawn` for the
  database work, plain data applied on the main thread, refresh-on-`NostrUpdate`/`Synced`/`Published`.
- Added `state_loaded: bool`, not in the sketch. Groups are derived from the read state, so a refresh
  before the stored state is read would briefly mark everything unread. The first refresh is chained
  after `load_state`, and later `refresh` calls are ignored until `state_loaded` is set.
- Account switches are guarded. `activate` and `reset` both replace `self.refresh` with a fresh
  `RefreshGate`, dropping any in-flight or pending run of the previous user, and the apply step of
  `run_refresh` aborts instead of applying when `Backend::current_user()` no longer matches the
  user the query was started for.
- Two additions to `signed_core::inbox` that Phase 1 needs: `InboxReadState::mark_archived` (mirrors
  `mark_read`) and `InboxItem::apply_state` (recomputes `unread_ids`/`archived`; `group` now uses it).
  Both are covered by tests.
- The thread-root lookup is built by walking every `e`/`E` ancestor transitively (`fetch_notifications`)
  rather than a single hop, because a patch series chains through parent patches. Only the notification
  events are grouped; ancestors are used solely as the lookup, so a root authored by someone else is
  not mistaken for a notification.
- The read/archive state event is written to LMDB only (`database().save_event`), signed with a fresh
  `Keys::generate()` on each save and never published. Filtering is by `d` tag only, no author, so the
  random key is irrelevant across sessions. `d` tag uses `me.to_hex()` rather than `Display`.
- Actions: `mark_read(root)`, `mark_archived(root)`, `mark_all_read()`. Each marks the group, advances
  the relevant cutoffs against **all** notification events (matching GitWorkshop's use of `allEvents`),
  re-derives the groups locally so the UI updates immediately, then persists in the background.
- The global `Inbox` keeps no derived state. The signing key is generated per save, the current user is read
  from `Backend::current_user()` where needed, and the relays of the user's own repositories are
  queried from `RepoListStore` in `Backend::sync_inbox` rather than cached. There is no prune logic
  either: the newest state event is selected by `created_at`.
- `Inbox::activate` / `Inbox::reset` are `pub(crate)`; the former `subscribe_remote` and
  `connect_own_repo_relays` methods were deleted once their work moved into `Backend::sync_inbox`.
- `cargo test -p signed_core` passes (68 tests), `cargo test -p signed_state` passes (24 tests);
  `cargo clippy -p signed_state --all-targets` is clean; `cargo check --workspace` succeeds.

### Phase 2 implementation notes

Files: `crates/workspace/src/views/{inbox.rs, mod.rs, sidebar/mod.rs}`. No store changes.

- `InboxView` is a plain center panel like `RepoListView`; the sidebar holds a
  `WeakEntity<InboxView>` so there is no cycle. Re-rendering relies on GPUI's render-time entity
  tracking rather than explicit observations. (Phase 2 introduced an `Entity<InboxStore>` here; it
  was later folded into the panel - see the store-merge note below.)
- The layout is a column of two flexible bordered cards (`gap_4`, `p_4`, each `flex_1`/`min_h_0`),
  inbox over activity. Each card is a rounded `v_flex` with a header bar (`section`) and a
  `gpui::list` body. There is no **My repositories** column: the sidebar already lists the user's
  repositories, so the panel is a single column.
- Notification rows read the newest event of each group for the actor, subject and time, and the
  root's kind for the icon. The repo name is resolved from `item.address` through a linear scan of
  `RepoListStore::announcements` (`repo_name`); the list is small and this keeps the store unchanged.
- The **Unread** / **Archived** header buttons are intentionally absent: they need
  `add_bottom_panel` / `InboxFilterView`, which are Phase 3. The header is only **Mark all read**,
  so the panel is fully usable on its own.
- `kind_icon` / `kind_label` map a `Kind` to a `CustomIconName`/`IconName` and a short noun. The
  cover note is compared with `==` rather than matched, since `Kind` cannot appear in a pattern arm.
- Sidebar: `open_inbox` mirrors `open_explore` (return if open, else add a center panel); the inbox
  nav item is repointed. The screen is still opened by the nav item, not on app startup, matching the
  "idle until signer" rule; auto-opening it as the post-login home is a possible follow-up.
- The **My repositories** column (search `InputState`, **New** button, `open_repo_panel` rows) was
  removed after Phase 2 as redundant with the sidebar, along with the panel's `dock_area`,
  `open_repo` / `open_create_repo` helpers and the `create_repo_dialog` / `open_repo_panel` imports.
  `InboxView::new` now takes only `cx`. `create_repo_dialog` is private again.
- `cargo clippy -p workspace --all-targets` is clean and `cargo check --workspace --all-targets`
  succeeds. `cargo test -p signed_core` (68) and `cargo test -p signed_state` (24) still pass.

### Architecture refactor (after Phase 2)

Phases 0-2 kept all derivation in the global `Inbox`, so every notification and activity query ran
whether or not the home screen was open, and `Backend::emit` carried a deferred side effect just to
feed it.

- The global `Inbox` is now thin: `state: InboxReadState`, `state_loaded`, plus the NIP-78 load/save
  and the mark actions.
- `Backend::emit` is gone. All `BackendEvent`s are emitted with `cx.emit` again, and `sync_inbox`
  updates the inbox synchronously, passing the client in so nothing reads `Backend` mid-update.
- The panel became the client-side owner of the derivation, initially through a panel-scoped
  `Entity<InboxStore>`.
- `signed_core` is unchanged.

### Store merged into the panel (after Phase 2)

The `InboxStore` entity was then folded into `InboxView`, since the panel was its only consumer.

- `InboxView` holds `notifications`, `activity`, `unread_count`, `state`, `state_loaded` and
  `RefreshGate` as fields, and the store's methods (`sync_state`, `handle_backend_event`,
  `refresh`/`run_refresh`, `regroup`, `publish_unread_count`, `clear`, the mark actions) became panel
  methods. The two subscriptions call them directly, with no `update` indirection.
- The database work stayed in `signed_state` as `pub async fn query_inbox(...)`; `RefreshGate` and
  `RefreshRequest` are re-exported. The UI crate never queries LMDB directly.
- `mark_read`, `mark_archived` and their `group_events` helper carry a scoped `#[allow(dead_code)]`
  until the Phase 3 sub-views wire them up.
- `cargo test -p signed_core` (68), `cargo test -p signed_state` (24) and `cargo test -p workspace`
  (7) pass; clippy and `cargo check --workspace --all-targets` are clean.

Trade-off: the unread count is derived by the panel, so it is only current while the panel is open.
(`publish_unread_count` fed a sidebar badge at the time; both were removed later - see "Sidebar
badge removed" below.)

### Phase 3 implementation notes

> Historical: the Unread/Archived sub-views below were later removed; the panel is now a single
> repository-grouped list. Kept for the `add_bottom_panel` / sub-view rationale.

Files: `crates/dock/src/lib.rs` and `crates/workspace/src/views/{inbox.rs, sidebar/mod.rs}`. No
store changes.

- `add_bottom_panel` sits next to `add_center_panel` and wraps
  `DockArea::add_panel_view(panel, DockPlacement::Bottom, None, ...)`. A new bottom dock starts open,
  and the workspace's existing `DockEvent::LayoutChanged` subscription removes an emptied bottom dock,
  so a closed sub-view leaves no strip behind.
- `InboxFilterView` is private to `views/inbox.rs`. It holds an `Entity<InboxView>` (strong; the
  panel keeps only the weak `filter_view` back, so there is no cycle), the mode, and its own
  `ListState`. There is no subscription: it reads the inbox entity during render, which is enough for
  GPUI to invalidate the window when the inbox notifies.
- `InboxFilter` is a private two-variant enum with `label()` and `matches(&InboxItem)`. The tab title
  comes from `Panel::title`, so switching modes through `set_mode` retitles the same tab instead of
  opening a second one.
- `InboxView` regained a `dock_area: WeakEntity<DockArea>` (removed with the My-repositories column)
  and takes it in `new`. `open_filter` reuses the existing panel, focuses it, and reopens the bottom
  dock when it is collapsed; otherwise it creates and adds the panel. `InboxView::new` is now called
  as `InboxView::new(self.dock_area.clone(), cx)` from `SidebarPanel::open_inbox`.
- The three `#[allow(dead_code)]` markers on `mark_read`, `mark_archived` and `group_events` are gone:
  Unread rows call `mark_read` on click and `mark_archived` from a trailing ghost icon button
  (`Button` + `IconName::FolderClosed`, tooltip "Archive"). The button calls `cx.stop_propagation()`
  so it does not also trigger the row's mark-read click. Archived rows are display-only; the read
  state has no un-archive operation.
- `notification_row` takes an id `prefix` and returns `Stateful<Div>` rather than `AnyElement`, so
  callers can attach a click handler and a trailing action. The inbox list passes `"inbox-row"` and
  the sub-view `"inbox-filter-row"`, because the two lists render in the same window and would
  otherwise collide on `(str, ix)` ids.
- `cargo clippy -p workspace -p dock --all-targets` is clean, `cargo check --workspace --all-targets`
  succeeds, and `cargo test -p signed_core -p signed_state -p workspace` passes (68 / 24 / 7).

### Phase 4 implementation notes

Files: `crates/workspace/src/views/{inbox.rs, mod.rs, repo_detail/mod.rs}`. No store changes.

- `RepoItem { Issue(EventId), PullRequest(EventId), Patch }` and `pub(crate) fn open_repo_item` live
  in `repo_detail/mod.rs`, next to `open_repo_panel`. `open_repo_item` builds its own `RepoStore` from
  the announcement (private `repo_store` helper), so only the item panel is docked.
- It is a free function, not `RepoDetailView::open_item`: the detail constructors take a `Window`, and
  a synchronous `&mut App` + `&mut Window` pair is not a `VisualContext`, so `Entity::update_in` is
  not available. `InboxView` already has the window in the list's `on_click`, so it drives the free
  function directly. The plan's original `detail.update_in(window, cx, ...)` sketch could not compile.
- `InboxView::open_item` is also a free function (it needs nothing but `dock_area`, which it captures
  from the panel) because the `gpui::list` item closure only receives `&mut App`. It resolves
  `item.address` through `RepoListStore`, returns silently when the repository is unknown, maps the
  root kind to a `RepoItem`, and calls `open_repo_item`.
- Fixed: the first version opened `RepoDetailView` to borrow its store (`RepoDetailView::store()`),
  which docked the repository panel alongside the item panel and showed its `not found` load error.
  `open_repo_item` now builds the `RepoStore` itself and `RepoDetailView::store()` is gone.
- Only the notification rows are clickable. Activity rows are display-only. The Phase 3 mark-read /
  archive row behaviour is gone with the sub-views.
- `RepoItem::Patch` is a unit variant because the id would be unused: patches have no detail panel, so
  `open_repo_item` returns before doing anything and nothing is docked.
- `cargo clippy -p workspace --all-targets` is clean, `cargo check --workspace --all-targets` succeeds,
  and `cargo test -p signed_core -p signed_state -p workspace -p dock` passes (68 / 24 / 7 / 1).

### Repository grouping redesign (after Phase 4)

Files: `crates/workspace/src/views/inbox.rs`. No store, no `signed_core` changes.

The two-card layout (notifications over activity) was replaced by a single repository-grouped list.

- The panel now derives `sections: Vec<InboxSection>` and a flattened `rows: Vec<InboxRow>` in
  `rebuild`, called from `run_refresh` and `regroup`. Both are stored as `Arc`s and cloned into the
  `gpui::list` closure, which indexes `notifications` / `activity` - no deep copies per frame and no
  data duplicated between the section list and the source lists.
- `InboxSection` groups a repository's non-archived notification groups and the user's own activity,
  newest first; sections are ordered by their newest entry. `InboxEntry` holds indices into the
  panel's lists; `InboxRow::Repo` / `InboxRow::Entry` / `InboxRow::Empty` is the flattened shape the
  list renders.
- All of the user's own repositories are seeded as sections from `RepoListStore::announcements_of`,
  so an owned repository with nothing to show gets an empty section ("No activity yet.") and sorts
  after the sections with activity. The panel observes `RepoListStore` to rebuild when the user's
  repositories load or change.
- Activity is matched to a repository through a `GitRepoAnnouncement` `a` tag (`repo_address`).
  Items without an address share the "Other repository" section.
- `notification_row` / `activity_row` no longer render the repository name - the section header does.
  That also drops one `RepoListStore` scan per row.
- The single card has one `ListState`; the old `notifications_list` / `activity_list` and the
  `render_inbox_panel` / `render_activity_panel` / `section` helpers are gone. `notification_row` still
  takes an id prefix so rows stay unique within the list.
- `cargo clippy -p workspace --all-targets` is clean and `cargo test -p signed_core -p signed_state
  -p workspace -p dock` passes (68 / 24 / 7 / 1).

### Sidebar badge removed (after the repository grouping redesign)

Files: `crates/signed_core/src/filters.rs`, `crates/signed_state/src/{inbox.rs,backend.rs}`,
`crates/workspace/src/views/{inbox.rs,sidebar/mod.rs}`.

An intermediate change made the sidebar badge live by moving the unread count into the global
`Inbox` (a `refresh_unread_count` driven by `Backend`). That was then reverted along with the badge
itself, so the global is thin again.

- The sidebar nav item no longer renders a `CountBadge`; `SidebarPanel` lost its `unread` field and
  its observe of the global `Inbox`.
- The global `Inbox` no longer stores an `unread_count` and has no `set_unread_count` /
  `refresh_unread_count`. `Backend` has no `refresh_inbox_unread` and no per-batch or per-sync count
  refresh. `filters::affects_inbox` and the `query_inbox` helper split were reverted with it.
- `InboxView` keeps its local `unread_count` for its header badge and the per-section `unread` for
  the repository headers; `publish_unread_count` stays deleted.
- Consequence: the unread count is only current while the panel is open, and there is no unread
  indication anywhere else in the app.
- `cargo clippy -p signed_core -p signed_state -p workspace --all-targets` is clean,
  `cargo check -p signed_core -p signed_state -p workspace --all-targets` succeeds, and
  `cargo test -p signed_core -p signed_state -p workspace -p dock` passes (68 / 24 / 7 / 1).

### Threads merged: notifications + activity (after the sidebar badge removal)

Files: `crates/signed_core/src/inbox.rs`, `crates/signed_state/src/inbox.rs`,
`crates/workspace/src/views/inbox.rs`.

Notifications and own activity were two separate row kinds that could describe the same thread. They
are now one item per thread: the notifications directed at the user and the user's own events in that
thread live in the same `InboxItem`. A row shows the thread root's title and up to five of the
thread's most recent events:

```
[icon] Add retry logic                                      (unread dot)
  [avatar] You    opened an issue · 3d
  [avatar] alice  commented · 2d
```

- `InboxItem` gained `root_event: Option<Event>` and `own_events: Vec<Event>`. `events` keeps only the
  notifications (others' events); `own_events` holds the user's own. `unread_ids`/`archived` are
  derived from `events` alone, so own activity is never unread and a thread with only own events is
  never archived (`apply_state` guards the empty case).
- New methods on `InboxItem`: `title()` (root event's subject, falling back to the newest event),
  `kind()` (root kind, same fallback), and `timeline(limit)` (thread events deduplicated by id,
  oldest first, always keeping the root event and filling the remaining slots with the most recent
  others).
- `group` now takes both `events` (notifications) and `own` (the user's activity) and merges them on
  the resolved root. Own events resolve through the same `notification_root`; an unresolved own event
  becomes its own root. `query_inbox` returns `(Vec<InboxItem>, usize)` - the separate activity list
  is gone, and `by_id` is extended with the own events so a comment of ours resolves to its thread.
- The panel holds `threads: Arc<Vec<InboxItem>>` instead of `notifications` + `activity`. The
  `InboxEntry` enum, `entry_time`, `repo_address`, `related_activity`, `notification_row`,
  `activity_row` and `kind_label` are gone. `thread_row` replaces both row kinds and is clickable like
  the old notification row; `group_sections` now just buckets threads by `item.address`.
- `sub_activity_line` is unchanged and still renders `[avatar] [name] [phrase] · [ago]`, with `You`
  for the signed-in user and `activity_phrase(kind)` for the verb. Rows are variable height
  (`py_2`), which `gpui::list` auto-measures.
- Thread rows in a section are drawn as one stack: `render_entry` passes `first`/`last` within the
  section (`entry_ix == 0` / `entry_ix + 1 == section.entries.len()`), and `thread_row` rounds the
  outer edges (`rounded_t` on the first, `rounded_b` on the last, theme radius) and draws a
  `border_b_1` divider on every row but the last.
- Trade-off: the row title is the thread root's, not the newest event's, so a comment thread no longer
  previews the comment text. That is the point of the merge - the row identifies the thread.
- `cargo clippy -p signed_core -p signed_state -p workspace --all-targets` is clean,
  `cargo check -p signed_core -p signed_state -p workspace --all-targets` succeeds, and
  `cargo test -p signed_core -p signed_state -p workspace` passes (69 / 24 / 7).

## 8. Validation

- `cargo test -p signed_core` (69 tests): root resolution, grouping, merging, read-state cutoff, serde
  round-trip.
- `cargo test -p signed_state` (24 tests): the `Inbox` / `query_inbox` paths that do not need GPUI
  (state round-trip, grouping helpers).
- `cargo test -p workspace` (7 tests): repository-detail helpers.
- `cargo clippy -p signed_state --all-targets`, `cargo clippy -p workspace --all-targets` and
  `cargo check --workspace --all-targets` after each phase.
- Manual: log in with a repo-owning identity; open the inbox from the sidebar and confirm the panel
  populates from another identity's issue/comment, the activity list shows your own items, and that no
  kind-30078 event is broadcast (watch the relays / `Published` events). Restart to confirm the read
  state is read back from LMDB. Confirm the sidebar has no unread badge.

## 9. SDK APIs used (verified in the pinned `5c669a4` checkout)

- `Kind::{Comment, GitIssue, GitPullRequest, GitPatch, GitPullRequestUpdate,`
  `GitStatusOpen/Applied/Closed/Draft, ApplicationSpecificData, EventDeletion, RequestToVanish}`
- `Filter::{kind, kinds, pubkey, pubkeys, custom_tags, limit, since, events, coordinate, identifier}`
  - Non-obvious: `Filter::pubkey`/`pubkeys` set the lowercase **`p` tag**, not `authors`. Use
    `Filter::author`/`authors` for authorship. The `notifications` filter relies on this.
- `SingleLetterTag::{LOWERCASE_P, LOWERCASE_E, UPPERCASE_P, UPPERCASE_K, UPPERCASE_E}`
- `nostr::nips::nip22::{extract_root, extract_parent, CommentTarget}`: NIP-22 root/parent pointers
- `Tags::{event_ids, public_keys, coordinates, identifier, hashtags}` iterators
- `Client::{database, subscribe, sync, notifications, send_event, add_relay}`;
  `NostrDatabase::{save_event, query}`; `NostrLmdb`, `NostrGossipMemory`
- `EventBuilder::{new, tags, finalize}`, `Tag::identifier`, `Keys::generate`
- `Timestamp`, `EventId` (hex serde), `PublicKey`, `Coordinate`
- Fetch paths converge on the same notification: `client.subscribe(...)` and negentropy
  `client.sync(...)` both persist received events to LMDB and surface them as
  `ClientNotification::Event`, which `Backend`'s pump batches into `BackendEvent::NostrUpdate`.
  This is why the query right after a fetch is a cache read, not a race.

# Backend audit: relays, sync and NIP-65

Status: audit of the current code, done before implementing
`docs/event-fetching-strategy.md`. Where this document and the first draft of
that proposal disagree, this document is authoritative.

## Scope and method

Crates read for relay and network behaviour:

- `signed_state`: `backend`, `repo`, `repos`, `inbox`, `profile`, `checkouts`,
  `local_repos`, `refresh`, `git_store`.
- `signed_nostr`: `backend`, `signer`, `update`.
- `signed_core`: `filters`, `deletions`, `model`, `state`, `status`, `inbox`.
- Relay touchpoints of `signed_git`, `settings`, `paths`, `utils`.

Cross-checked against:

- rust-nostr at the revision from `Cargo.lock`,
  `b230cecf9dbb38e0228e6fff4544ed9d261326fc`. **All** `nostr*` crates
  (`nostr`, `nostr-database`, `nostr-gossip`, `nostr-gossip-memory`,
  `nostr-lmdb`, `nostr-sdk`) resolve to that single revision. The local
  checkout is `~/.cargo/git/checkouts/nostr-619b808bb247a9ed/b230cec`.
- GitWorkshop, cloned with `ngit` from
  `nostr://npub15qydau2hjma6ngxkl2cyar74wzyjshvl65za5k5rl69264ar2exs5cyejr/gitworkshop`
  at revision `420c0c3`.

Paths below are relative to those checkouts unless prefixed with
`crates/`, which means this repository.

## 1. Verified SDK behaviour

### Target selection

- `nostr-sdk/src/client/api/req_target.rs`: `ReqTarget::auto` wraps a bare
  filter list; `ReqTarget::manual` wraps a relay map. `From<HashMap<T, Vec<Filter>>>`
  (L91) makes an explicit map a **manual** request.
- `nostr-sdk/src/client/api/util.rs::build_targets`:
  - Auto + gossip configured -> `gossip_break_down_filters`.
  - Manual -> the map is used as-is, gossip is skipped.
- `nostr-sdk/src/client/api/subscribe.rs` and `api/sync.rs` both go through
  these rules. `client.sync(filter)` with no `.with(...)` uses gossip;
  `client.sync(filter).with(urls)` does not.
- `pool.sync` errors with `relay not found` when a targeted relay has not been
  added to the pool. Targets are not implicitly added.

### Gossip breakdown

- `nostr-sdk/src/client/gossip/updater.rs::gossip_break_down_filter` (L412)
  extracts pubkeys via `Filter::extract_public_keys`
  (`nostr/src/filter/mod.rs` L591): **only `authors` and the lowercase `#p`
  tag**. It first calls `ensure_gossip_public_keys_fresh`, then breaks the
  filter down, then adds every resolved relay to the pool with
  `RelayCapabilities::GOSSIP`.
- Freshness (`updater.rs` L381-L410, L164-L185) negentropy-syncs kind `10002`
  for the extracted pubkeys over the pool's `DISCOVERY | READ` relays. This is
  the only automatic way the gossip store learns kind `10002`.
- `nostr-sdk/src/client/gossip/resolver.rs::break_down_filter` (L93):
  - `authors` only -> each author's **write** relays, plus hint and
    most-received relays.
  - `#p` only -> each pubkey's **read** relays, plus hint and most-received.
  - both -> the union of read and write relays, keeping the filter unchanged.
  - neither (`Other`), or no relay found (`Orphan`) -> the pool's **read**
    relays (`read_relay_urls`).
- `relay/capabilities.rs`: `GOSSIP` is its own bit. `pool.read_relay_urls` /
  `write_relay_urls` / `relay_urls_with_any_cap` use a raw bit test
  (`filter_relays_with_any_cap`), so GOSSIP-only relays are excluded from
  broadcast and from the `Other`/`Orphan` fallback. Per-relay send checks use
  `can_read`/`can_write`, which do include GOSSIP, so gossip-resolved targets
  work when named explicitly.
- `GossipConfig::default()` (`builder.rs` L30-L120): limits read 3, write 3,
  hints 1, most-received 1, NIP-17 3 per user; `background_refresh` on by
  default, disabled by `.no_background_refresh()`.
- `NostrGossipMemory` reads only the first 7 entries of a kind `10002`
  (`MAX_NIP65_SIZE`) and stops tracking new hint/most-received relays once a
  pubkey is near `MAX_RELAYS_PER_PK` (7). `get_best_relays(pk, selection, allowed)`
  (public trait, `gossip/nostr-gossip/src/lib.rs`) reads whatever is in the
  store, sorted by received-event count then recency. It does **not** fetch
  kind `10002` itself. The store is primed by the freshness step of an Auto
  request (or by any kind `10002` that arrives for another reason), and marks
  a key outdated 24 h after its last fetch attempt.

### Publishing

- `client.send_event(event)` with **no** policy and gossip configured targets
  NIP-65: it triggers the same freshness, then sends to the author's outbox
  and every `p`-tagged pubkey's inbox (`send_event.rs::gossip_prepare_urls`).
- `client.send_event(event).broadcast()` overrides this to the pool's write
  relays only (`send_event.rs` L428-L430). `signed` uses `broadcast()`
  everywhere.

### Notifications

- Every message from a relay passes `relay/inner.rs::handle_event_msg`, which
  saves new events and emits `ClientNotification::Event`. This includes events
  received by a negentropy **sync** (the sync's down subscription is
  registered as an auto-closing subscription, and the events travel the normal
  message path). Events already in the database do not re-notify.
- The comment in `crates/signed_state/src/profile.rs` claiming synced events
  produce no `NostrUpdate` is inaccurate for this revision. The re-read after
  a sync is harmless, but the comment should not be relied on.

## 2. What `signed` actually does today

Every fetch path is manual. There is no Auto request anywhere in the
workspace, so `gossip_break_down_filter` and `ensure_gossip_public_keys_fresh`
never run. The gossip store is configured and passively fed
(`handle_event_msg` calls `gossip.process` for every received event), but it
is never consulted for targeting.

| Call site | Request | Target relays | Target kind |
| --- | --- | --- | --- |
| `Backend::bootstrap` | `add_relay` + connect | `BOOTSTRAP_RELAYS` (READ\|WRITE), `INDEXER_RELAYS` (`DISCOVERY`) | n/a |
| `Backend::subscribe_bootstrap` -> `subscribe_bootstrap_only` | `client.subscribe(HashMap<&str, Vec<Filter>>)`, `ExitOnEOSE`, 10 s timeout | `BOOTSTRAP_RELAYS` | manual |
| `Backend::sync_bootstraps` -> `sync_bootstrap_only` | `client.sync(filter).with(BOOTSTRAP_RELAYS)` | `BOOTSTRAP_RELAYS` | manual |
| `Backend::connect_repo_relays` | `add_relay().and_connect()` then `client.sync(filter).with(relays)` | announcement `relays` tag | manual |
| `RepoListStore::subscribe_remote` | `sync_bootstraps(all_announcements, all_states, deletions)` | `BOOTSTRAP_RELAYS` | manual |
| `RepoListStore::sync_own_repo_states` | `connect_repo_relays(state(addr))` | own repos' announced relays | manual |
| `RepoStore::subscribe_remote` | `subscribe_bootstrap(repo_filters)` | `BOOTSTRAP_RELAYS` | manual |
| `RepoStore::connect_announced_relays` | `connect_repo_relays(repo_filters)` | announced relays | manual |
| `RepoStore::run_refresh` root follow-ups | `subscribe_bootstrap(root_filters)` + `connect_repo_relays(root_filters)` | bootstrap + announced relays | manual |
| `Backend::sync_inbox` | `subscribe_bootstrap(notifications, authored_activity)` + `connect_repo_relays` over own announcements | bootstrap + own repos' relays | manual |
| `Backend::bootstrap_user` | `sync_bootstrap_only(grasp_list)` | `BOOTSTRAP_RELAYS` | manual |
| `ProfileStore::handle_requests` | `sync_bootstrap_only(metadata)` | `BOOTSTRAP_RELAYS` | manual |
| All publishes | `client.send_event(event).broadcast()` | pool write relays | no gossip |

Consequences:

- `INDEXER_RELAYS` are connected but receive no request: being `DISCOVERY`-only
  they are excluded from every manual target, and no Auto request exists to
  use them. They become useful only once an Auto request runs, or if a manual
  request names them.
- The NIP-34 `p` tags on events do not influence fetching. `extract_public_keys`
  reads the **filter**, not the events, and the repo filters carry no pubkeys
  outside the announcement/state and author-deletion filters.
- The pool has no `max_relays` (`builder.rs` default `None`) and relays are
  never removed. Any future Auto traffic accumulates GOSSIP relays for the
  lifetime of the session.

## 3. The NIP-34 `p` tag, per event kind

The claim "activity events already carry a `p` tag pointing at the repository
owner" is true for the root kinds and false for kind-1111 comments:

| Kind | Lowercase `p` | Source |
| --- | --- | --- |
| Issue 1621 | repository owner | `nostr/src/nips/nip34.rs::GitIssue` (L476-L484) |
| Patch 1617 | repository owner | `nip34.rs::GitPatch` (L575-L583); `RepoStore::publish_patch_series` builds patches by hand and adds `Tag::public_key(owner)` explicitly |
| PR 1618 | repository owner | `nip34.rs::GitPullRequest` (L654-L664) |
| PR update 1619 | repository owner | `nip34.rs::GitPullRequestUpdate` (L721-L730) |
| Status 1630-1633 | owner and root author | `RepoStore::set_status` / `publish_applied_status` push both explicitly |
| Comment 1111 | **parent author, not necessarily the owner** | `CommentBuilder` emits the root as uppercase `E`/`K`/`P` and the parent as lowercase `e`/`k`/`p` (`nostr/src/nips/nip22.rs::as_vec`). `RepoStore::comment_builder` passes the root as the parent for top-level comments, so `p` is the issue/PR author there. |

So a single `Filter` with both `.coordinate(addr)` (`#a`) and
`.pubkey(maintainers)` (`#p`) would drop comments on roots authored by
non-maintainers. Keeping the existing `#a`-only filter and adding a second
`#p`-scoped filter is safe. The `#p`-scoped filter buys read-relay ("inbox")
targeting through gossip, not root coverage.

## 4. Findings

1. **Startup ordering hazard (medium).** `Backend::new` defers `bootstrap`,
   which adds relays inside a background task; `RepoListStore::new` defers
   `subscribe_remote`, which immediately background-spawns
   `sync_bootstraps`. If the sync reaches `pool.sync` before the relays are in
   the pool, every filter fails with `relay not found`. The global sync runs
   only once per session and is not retried, so a lost race means no fresh
   announcements, states or deletions are fetched until the next launch (the
   persistent LMDB still supplies what previous sessions stored). Fix
   options: add the bootstrap relays before spawning, or make the sync helpers
   ensure their relays.
2. **Profile coverage vs. the indexers (medium).** Moving
   `wss://profiles.nostr1.com` from `BOOTSTRAP_RELAYS` to `INDEXER_RELAYS`
   (working tree) means `ProfileStore`, which syncs metadata over
   `BOOTSTRAP_RELAYS`, no longer reaches it. `INDEXER_RELAYS` themselves are
   currently inert (see above). Decide whether profile metadata should target
   a profile indexer explicitly, and whether `profiles.nostr1.com` indexes
   kind `10002` at all.
3. **State is owner-only (low, verify against NIP-34).** `filters::state`
   and `repo_filters` fetch kind `30618` with `.author(addr.public_key)`, the
   announcement author. GitWorkshop reads state from every confirmed
   maintainer (`useResolvedRepository.ts`, `stateCandidates`). If co-maintainers
   publish state events, `signed` ignores them.
4. **`profile.rs` comment wrong (cosmetic).** Synced events do emit
   `NostrUpdate` on this SDK revision; the post-sync re-read in
   `handle_requests` is defensive, not load-bearing.
5. **Progress fields are global (cosmetic).** Overlapping `sync_bootstraps`
   calls share `sync_progress`; the last writer wins and completion of either
   clears it. Progress counters from sequential filters accumulate correctly
   because all filters share one watch channel.
6. **Background fetch failures are log-only (policy).** `connect_repo_relays`
   and the per-filter sync errors are logged, not surfaced. Given the new
   strategy adds more fetch paths, decide whether relay reachability should
   feed the existing `last_error` / `last_warning` surfaces.

## 5. Consequences for the fetching strategy

- **Auto and Manual can coexist**, as asked. They are independent requests on
  the same pool; events deduplicate in the database and only new events
  notify.
- **The gossip store must be primed by an Auto request before it can resolve
  anything.** `get_best_relays` is a pure store read, and
  `ensure_gossip_public_keys_fresh` is private to the client. Sequence
  Uncensored as: Auto request (freshness runs as a side effect) -> resolve
  maintainers via `get_best_relays(pk, All { .. })` -> manual sync through
  `connect_repo_relays`.
- **The manual leg already covers GitWorkshop's "outbox and inbox".**
  `BestRelaySelection::All { read, write, hints, most_received }` returns both
  directions, so the activity filter does not need `.pubkey(..)` for reach,
  and the comment caveat in section 3 becomes optional rather than blocking.
  Adding `.pubkey(maintainers)` still helps the Auto leg reach maintainer
  inboxes and is worth doing as a second filter.
- **Curated can stay exactly what the code does today** (announced relays,
  manual). The only decision is whether the per-repository REQ against
  `BOOTSTRAP_RELAYS` remains in Curated or not.
- Publishing is unaffected; it already broadcasts to every pooled write relay
  and does not consult the strategy.

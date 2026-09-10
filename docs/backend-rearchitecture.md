# Backend re-architecture: findings and outcome

This is a follow-up to an initial architecture review. It re-checks every claim
against the **actual `nostr`/`nostr-sdk` source pinned by `Cargo.lock`**
(`rev 0c6fad2ac8ce934747096953f6dba355e3532614`, checked out locally at
`~/.cargo/git/checkouts/nostr-9dff06fa64f758da/0c6fad2/{nostr,nostr-sdk}/src`)
and the actual **GPUI source pinned by `Cargo.lock`**
(`git+https://github.com/zed-industries/zed#1870e269ad88802147f2baec3086abb67d17260a`,
checked out at `~/.cargo/git/checkouts/zed-a70e2ad075855582/1870e26/crates/{gpui,scheduler}/src`),
not from general knowledge of either. Every API claim below cites the file it
was verified against.

Scope: `crates/signed_nostr`, `crates/signed_state`, `crates/signed_core`,
`crates/signed_git`, and `crates/workspace` (the actual call sites of the
backend, audited for business-logic flaws and redundant conversions).

**Status: complete.** All 14 items of the plan are implemented and verified;
the compact record is the **Outcome** section at the end. Sections 1–17 are
kept as the analysis each change was based on — they describe the code as it
was *before* the change, so read them as rationale, not as current
documentation.

## Summary of the ask

1. Never call `fetch_events`. Bootstrap only via `subscribe`/`sync` (negentropy), read from `client.database()`.
2. Collapse the multiple "send an event" functions into direct `nostr-sdk` calls, no house wrappers.
3. Verify every API claim against the locally checked-out SDK/GPUI source.
4. Remove unnecessary logic (relay add/connect round trips, the fetch/sync dedup cache, unbounded task lists).
5. Re-evaluate `signed_git`'s dependence on `gix` — how much of it duplicates functionality `gix` (or another crate) already provides.
6. Check for unnecessary `cx.notify()` / over-broad re-renders vs. partial re-render.
7. Audit `crates/workspace` (the real UI call sites) for business-logic flaws of the same shape as `create_repository`, and for unnecessary string/type conversions and clones.

Each is addressed below with concrete file:line references and a verified replacement.

---

## 1. `fetch_events` — one call site, and it should go too

```
grep -rn "fetch_events" crates/
crates/signed_state/src/backend.rs:1065
```

The **only** use in the whole workspace is `Backend::bootstrap_user`
(`crates/signed_state/src/backend.rs:1059-1086`):

```rust
fn bootstrap_user(&mut self, public_key: PublicKey, cx: &mut Context<Self>) {
    let client = self.client.clone();
    self.push_task(cx.spawn(async move |this, cx| {
        let result = async {
            let events: Vec<Event> = client
                .fetch_events(filters::grasp_list(public_key))
                .await?
                .into_iter()
                .collect();
            for url in latest_grasp_list_servers(events) {
                client.add_relay(url.as_str()).await.ok();
            }
            client.connect().await;
            Ok::<_, Error>(())
        }.await;
        ...
    }));
}
```

Verified against `nostr-sdk/src/client/mod.rs:963-1018` (doc comment on
`Client::fetch_events`): it's explicitly the "buffer events, return a `Vec`"
sibling of `stream_events`, both explicitly documented as **short-lived**
subscriptions for one-off reads — the SDK's own guidance ("for long-lived
subscriptions use `Client::subscribe`") doesn't forbid `fetch_events`
outright, but the project rule you want is stricter: never bypass the
database. That's achievable here too, because `client.sync` degrades
gracefully to a plain fetch-and-store when the local DB has nothing yet.

**Replacement** — sync against the bootstrap relays (same relays already
used for every other bootstrap query, see `BOOTSTRAP_RELAYS`,
`backend.rs:28-33`) and then read the result out of the database, exactly
like every other store in this codebase already does:

```rust
// Also drops push_task/tasks in favor of .detach() — see §6.
fn bootstrap_user(&mut self, public_key: PublicKey, cx: &mut Context<Self>) {
    let client = self.client.clone();
    cx.spawn(async move |this, cx| {
        let result = async {
            client
                .sync(filters::grasp_list(public_key))
                .with(BOOTSTRAP_RELAYS)
                .await?;

            let events = client.database().query(filters::grasp_list(public_key)).await?;
            for url in latest_grasp_list_servers(events) {
                client.add_relay(url).and_connect().await.ok(); // see §8
            }
            Ok::<_, Error>(())
        }.await;
        ...
    })
    .detach();
}
```

Verified against `nostr-sdk/src/client/api/sync.rs:1-32` and
`nostr-sdk/src/client/mod.rs:1020-1030` (`Client::sync` doc: "Performs a
negentropy-based reconciliation between the local database and one or more
relays" — this is exactly a bootstrap-and-store operation, no separate
"first fetch" step needed). No other code changes: `filters::grasp_list` and
`latest_grasp_list_servers` are unaffected.

This also removes the last inconsistency in the codebase between "how we get
data:" everywhere else is sync-then-query; now it's sync-then-query
everywhere, no exceptions.

---

## 2. The "send an event" functions — there are 8, there should be roughly 2

Grep for anything that ends up calling `client.send_event`:

| Function | File:line | What it adds over `client.send_event` |
|---|---|---|
| `Backend::send` | `backend.rs:1308-1322` | signs with the current signer, then calls `broadcast_event` |
| `Backend::publish_event` | `backend.rs:1325-1332` | calls `broadcast_event` on an already-signed event |
| `Backend::publish_task` | `backend.rs:1335-1360` | wraps a future, emits `BackendEvent::Published`/`Error` |
| `Backend::send_fire_and_forget` | `backend.rs:1363-1375` | calls `send`, drops the result except logging |
| `Backend::retract_events` | `backend.rs:1378-1398` | hand-builds NIP-09 tags, calls `send` |
| `broadcast_event` (free fn) | `backend.rs:1404-1418` | calls `client.send_event`, turns "0 relays accepted" into an `Err` |
| `stage_event_on_relay` | `backend.rs:1768-1795` | calls `client.send_event(..).to([relay])`, same 0-accept-is-Err logic, different error type (`String`) |
| `RepoStore::send` | `repo.rs:1334-1344` | calls `Backend::send`, tracks `last_error` — **but several `RepoStore` methods bypass it** and call `Backend::send`/`Backend::publish_event` directly (`repo.rs:803`, `repo.rs:935`), so error surfacing is inconsistent across `RepoStore` methods |

That's 8 layers for what the SDK already does in one call. Verified against
`nostr-sdk/src/client/api/send_event.rs:119-350`:

- `client.send_event(&event)` **already** verifies the signature, saves the
  event to the local database (`save_into_database`, default `true`), and
  broadcasts — all before you touch anything (`send_event.rs:337-345`).
- Zero-relay-accepted is *not* an error from the SDK's point of view — it
  returns `Ok` with `output.success` empty and `output.failed` populated.
  Turning that into an app-level error is legitimate domain logic (the repo
  already gets this right), it just doesn't need 3 separate functions
  (`broadcast_event`, `stage_event_on_relay`, and the implicit success check
  buried in `RepoStore::send`) doing the same "empty success ⇒ error" check.

### Recommended shape: one helper, and direct SDK calls everywhere else

Keep exactly **one** small helper because the "empty success ⇒ Err" rule is
real, repeated, app-specific policy (the SDK intentionally leaves that
decision to the caller):

```rust
/// The event was accepted by at least one relay, or a descriptive error otherwise.
async fn require_relay_accepted(output: SendEventOutput) -> Result<Event, Error> {
    if output.success.is_empty() && !output.failed.is_empty() {
        let reasons = output.failed.values().cloned().collect::<Vec<_>>().join(", ");
        bail!("event not accepted by any relay: {reasons}");
    }
    Ok(event)
}
```

Then delete `Backend::send`, `Backend::publish_event`,
`Backend::send_fire_and_forget`, `broadcast_event`, and `RepoStore::send`.
Call `client.send_event(...)` **directly** at each call site, exactly like
`stage_event_on_relay` already does for the GRASP staging path — that
function is the one place in the codebase that already follows this
pattern (`.to([relay.clone()])`, explicit target, no extra wrapper beyond
the accept-check). Generalize *that* pattern instead of routing everything
through `Backend`.

```rust
// A GPUI call site, e.g. RepoStore::open_issue, today:
self.send(builder, cx);

// direct SDK call instead. No task list to push into and prune either —
// see §6, `.detach()` is the right default here.
let signer = Backend::global(cx).read(cx).signer();
let client = Backend::global(cx).read(cx).client();
cx.spawn(async move |this, cx| {
    let event = builder.finalize_async(&signer).await?;
    let output = client.send_event(&event).await?;
    let event = require_relay_accepted(output, event).await?;
    this.update(cx, |this, cx| { /* apply + cx.notify() */ })
})
.detach();
```

`Backend` still owns the `Client`/`UniversalSigner` (a real, load-bearing
type — see §4 for why it must stay), but it should expose them
(`Backend::client()`/`Backend::signer()`, both already exist,
`backend.rs:1089-1096`) rather than mediate every publish through 4 layers
of wrapper. Emitting `BackendEvent::Published` for cross-store invalidation
(e.g. so `RepoListStore` refreshes when a new announcement lands) is the one
piece of `publish_task` worth keeping — but it can be a single `fn` taking
`&Event` that any call site invokes after its own `send_event`, not the
thing that *does* the sending.

### `Backend::retract_events` — use the SDK's own NIP-09 builder, one deletion event per target

`nostr` already ships `EventDeletionRequest` (verified in
`nostr/src/nips/nip09.rs:15-92`), which implements `IntoEventBuilder` exactly
like `GitRepositoryAnnouncement`/`GitIssue`/etc. already used elsewhere in
this codebase. Today's code hand-builds the tags for **one** deletion event
covering every target, plus a `k` tag per target:

```rust
// today, backend.rs:1378-1398
let mut tags: Vec<Tag> = Vec::with_capacity(events.len() * 2);
for event in events {
    tags.push(Tag::event(event.id));
    tags.push(Tag::parse(["k", &event.kind.to_string()]).expect("valid kind tag"));
}
let task = self.send(EventBuilder::new(Kind::EventDeletion, "").tags(tags), cx);
```

Per direction from the team: no `k` tag, and each event gets its own
deletion event rather than one deletion event listing multiple `e` tags.
`EventDeletionRequest` (`nip09.rs:15-92`) supports exactly that shape
already — call `.id(event.id)` once per event and send each independently:

```rust
async fn retract_event(client: &Client, signer: &UniversalSigner, event: &Event) -> Result<(), Error> {
    let builder = EventDeletionRequest::new().id(event.id).into_event_builder();
    let deletion = builder.finalize_async(signer).await?;
    client.send_event(&deletion).await?;
    Ok(())
}

fn retract_events(&mut self, events: &[Event], cx: &mut Context<Self>) {
    let client = self.client.clone();
    let signer = self.signer.clone();

    for event in events.to_vec() {
        let client = client.clone();
        let signer = signer.clone();

        cx.spawn(async move |_this, _cx| {
            if let Err(e) = retract_event(&client, &signer, &event).await {
                log::warn!("failed to retract event {}: {e}", event.id);
            }
        })
        .detach();
    }
}
```

No hand-rolled tag construction, no batching multiple targets into one
event, no `k` tag, and no task list to maintain (§6). Each deletion is
independent: a relay rejecting or dropping one doesn't affect the others.

---

## 3. Remove the fetch/sync dedup cache — it duplicates state that already exists elsewhere

`Backend` carries:

```rust
recent_fetches: HashMap<u64, Instant>,          // backend.rs:86
const FETCH_DEDUP_WINDOW: Duration = ...;       // backend.rs:39
fn fetch_recently_started(&mut self, fingerprint: u64) -> bool { ... } // backend.rs:1178-1186
fn fetch_fingerprint(relays: &[&str], filters: &[Filter]) -> u64 { ... } // backend.rs:1423-1433
```

used at 3 call sites (`connect_repo_relays`, `sync_bootstrap`, and
indirectly wherever those are called), e.g.:

```rust
pub fn sync_bootstrap(&mut self, filter: Filter, cx: &mut Context<Self>) {
    let fingerprint = fetch_fingerprint(&BOOTSTRAP_RELAYS, std::slice::from_ref(&filter));
    if self.fetch_recently_started(fingerprint) {
        log::debug!("skipping duplicate bootstrap sync");
        return;
    }
    ...
}
```

This is a generic "have I already asked for this filter recently"
cache, sorting + hashing relay lists and filters, pruning on a 5-minute
window, and un-inserting on error so a failed sync can retry immediately.
It exists purely to avoid redundant `sync`/`subscribe` calls — but every
call site that calls into `Backend::sync_bootstrap`/`connect_repo_relays`
**already has its own, more precise state for exactly this purpose**:

- `RepoStore` tracks `repo_relays: HashSet<RelayUrl>` (`repo.rs:77`) — "have
  I already connected+fetched this repo's relays" — and `root_fetches:
  HashSet<EventId>` (`repo.rs:81`) for per-root fetches.
- `RepoListStore` and `CheckoutsStore` each already run every refresh
  through `RefreshGate` (`refresh.rs`), which itself exists to coalesce
  bursts of refresh requests — that's the same "don't do this again right
  now" idea, at the right granularity (per-store, per-purpose), not a
  generic cross-cutting cache keyed by a hash of relays+filters.

The `Backend`-level cache is solving the same problem a second time, at a
coarser and more error-prone granularity (a hash collision or an
order-sensitivity bug silently drops a legitimate sync; the 5-minute window
is a magic number with no connection to how often any of the 3 call sites
actually fire). Delete `recent_fetches`, `fetch_recently_started`,
`fetch_fingerprint`, `FETCH_DEDUP_WINDOW`, and `DefaultHasher`/`Hash`/`Hasher`
imports they pull in. Let each caller guard itself the way `RepoStore`
already does for `repo_relays`:

```rust
// RepoStore, once per repo — this pattern already exists (repo.rs:189ish),
// just needs to also gate the *bootstrap* sync calls the same way instead
// of relying on a Backend-side cache.
if self.repo_relays.insert(relay.clone()) {
    backend.update(cx, |backend, cx| backend.connect_repo_relays(vec![relay], filters, cx));
}
```

`sync_bootstrap` for repo-independent filters (announcements, deletions) is
called from exactly one place today (`RepoListStore::subscribe_remote`,
`repo_list.rs:144-153`), on store construction — i.e., once per app
session. It does not need a dedup cache at all; if you're worried about a
second `RepoListStore` instance ever existing, that's a `Global`-uniqueness
invariant, not something to paper over with a fingerprint cache.

---

## 4. Gossip is enabled, and stays enabled — but today's git-domain sends should bypass it explicitly

Per team direction: gossip is a deliberate, load-bearing choice for this
client (it's not fully wired up to a feature yet, but it's not incidental
configuration either). `.gossip(...)` stays in `signed_nostr::backend::with_database`
(`crates/signed_nostr/src/backend.rs:31-51`). This section is scoped down
to what falls out of that: how the currently-implemented send paths
interact with gossip being on, verified against the SDK source.

```rust
let client = ClientBuilder::default()
    .database(database)
    .authenticator(authenticator)
    .gossip(NostrGossipMemory::unbounded())
    .gossip_config(GossipConfig::default().no_background_refresh())
    ...
    .build();
```

Verified against `nostr-sdk/src/client/api/send_event.rs:337-388` and the
doc comment on `Client::send_event` (`client/mod.rs:1097-1130`): **when no
explicit target is set** (no `.to()`/`.broadcast()`/`.to_nip17()`/`.to_nip65()`),
and gossip is configured, `send_event` resolves the destination via the
gossip engine (NIP-65 relay discovery for the event's author + tagged
pubkeys), not simply "every relay you `add_relay`'d". Every one of the 8
send-paths in §2 calls `client.send_event(&event)` with **no explicit
target** — meaning every one of them is going through gossip-based relay
resolution today, on top of the relays this app added on purpose
(`BOOTSTRAP_RELAYS`, the repo's own `relays` tag, GRASP servers).

That happens not to lose anything today, because `gossip_prepare_urls`
(`send_event.rs:229-320`) *also* unions in `client.pool().write_relay_urls()`
at the end — so events still reach every WRITE relay in the pool, gossip
only adds more relays on top. But it's not free: every plain `send_event`
call (opening an issue, commenting, reacting to a PR) does gossip
relay-list resolution — potentially a network round trip to fetch a NIP-65
list — for events whose target set is already fully determined by the
repo's own `relays` tag or the bootstrap relay list, and where the extra
NIP-65 relays gossip adds are not places NIP-34 consumers are expected to
look.

**Recommendation:** keep `.gossip(...)` configured (it's wanted for
whatever's next — NIP-17 DMs, NIP-65 profile/relay-list features, etc.),
but make the git-domain sends that already have a well-defined target
explicit about it, the same way `stage_event_on_relay` already is
(`.to([relay.clone()])`, `backend.rs:1768-1795`):

- Repository-scoped events (announcements, state, issues, PRs, patches,
  comments, statuses, deletions) know their target relays already (the
  repo's `relays` tag, or `BOOTSTRAP_RELAYS` for repo-independent
  discovery events) — send them with `.broadcast()` or `.to(relays)` so
  they don't pay for gossip resolution and don't silently depend on the
  sender's NIP-65 list being fresh.
- Anything that *should* use gossip once it exists (e.g. a future NIP-17
  DM, or explicit NIP-65 profile publishing) keeps the default routing, or
  calls `.to_nip17()`/`.to_nip65()` explicitly.

This is a small, additive change (one `.broadcast()`/`.to(...)` call per
send site as part of the §2 consolidation), not a removal — do it while
touching each call site for the send-path cleanup below, so gossip stays
fully available for the features that are meant to use it, while today's
repo/issue/PR/patch traffic stays deterministic about where it goes.

---

## 5. `signed_git` vs `gix` — split verdict, not "throw it all out"

`crates/signed_git/src/lib.rs` is 4118 lines. Checked the actual `gix`
version pinned (`gix = "0.87.1"`, feature set in the root `Cargo.toml`) and
its `gix-diff 0.67.1` dependency against what `signed_git` hand-rolls.

### Already correct, idiomatic `gix` usage — keep as-is

`tree_diff` (`signed_git/src/lib.rs:1468-1583`) generates commit-to-commit
diffs by calling `repo.diff_tree_to_tree(...)`, then
`gix::diff::blob::diff_with_slider_heuristics(...)`, then feeding the result
through `gix::diff::blob::UnifiedDiff::new(&diff, &input, collector, ..)`
where `collector` implements gix's own `ConsumeHunk` trait
(`signed_git/src/lib.rs:1963-2027`, matching `gix-diff-0.67.1/src/blob/unified_diff/mod.rs:70-84`
exactly). This *is* the documented, intended way to consume `gix`'s diff
engine — there is no simpler API to fall back to, and no unnecessary
reimplementation here. Same for the porcelain wrappers around `gix::Repository`
for refs, branches, tags, worktree checkout, etc. — that's inherent surface
area for a git-porcelain layer, not bloat.

### Real duplication — the `git format-patch` text parser

The other ~700 lines (`parse_diff_section`, `parse_hunk`, `hunk_header`,
`header_paths`, `diff_line_path`, `take_quoted`, `unquote_path`,
`strip_patch_prefix`, `name_from_address`, `signed_git/src/lib.rs:1660-2027`)
are a hand-rolled parser for **already-rendered** `git format-patch`/unified
diff text — this is necessary because a NIP-34 patch event's content *is*
the raw text output of `git format-patch`, arriving over Nostr with no
backing git objects to hand to `gix`'s diff engine. `gix-diff` only
*generates* unified diffs from git objects; it has no facility to *parse*
unified-diff text back into structured hunks, so this isn't a case of
"gix already does this and we reimplemented it."

However, a maintained crate already exists for exactly this parsing job:
[`diffy`](https://docs.rs/diffy)'s `PatchSet` module
(`diffy::patch_set::PatchSet::parse(text, ParseOptions::gitdiff())`)
explicitly parses "the output of `git diff` or `git format-patch`",
supporting `diff --git` headers, extended headers (`new file mode`,
`deleted file mode`, etc.), rename/copy detection via `rename from`/`rename
to`/`copy from`/`copy to`, and binary-file detection — i.e., the exact
feature list `signed_git`'s hand-rolled parser reimplements
(`FileDiff::status` has `Renamed`/`Copied`/`Added`/`Deleted`/`Modified`
variants, `signed_git/src/lib.rs:1373-1379`; binary detection at
`signed_git/src/lib.rs:1395`).

**Recommendation:** spike replacing `patch_diffs`/`parse_diff_section`/
`parse_hunk`/`unquote_path`/etc. with `diffy::patch_set::PatchSet`, mapping
its `FileOperation`/`Hunk` types onto this codebase's existing `FileDiff`/
`DiffHunk` (which downstream UI code already depends on, so keep those
public types and only replace the parsing internals). This is the single
biggest concrete size reduction available in the whole backend — a ~700
line hand-rolled parser (plus ~1300 lines of tests for it,
`signed_git/src/lib.rs:3681-4053` and surrounding) collapses to a thin
adapter over a well-tested crate. Budget a spike first: `diffy`'s renamed
path handling and quoted-path unescaping need to be checked against this
project's test fixtures (`signed_git/src/lib.rs:3917-3962`,
octal-escaped/non-ASCII quoted paths) before committing to the swap.

**Outcome of the spike: the swap is sound, and was committed.** Both specific
risks flagged above checked out:

- **Renamed paths.** `diffy` produces `FileOperation::Rename { from, to }` from
  the `rename from`/`rename to` extended headers, and those paths are *not*
  `a/`/`b/`-prefixed, unlike `Create`/`Delete`/`Modify`, which come from the
  `---`/`+++` lines *with* the prefix. The adapter therefore calls
  `FileOperation::strip_prefix(1)` (git's `-p1`) only for the non-rename
  variants — exactly the split `FileOperation`'s own doc comment and
  `diffy`'s `examples/apply.rs` describe. This is the one place the two APIs
  differ in shape, and the one place a naive port would have broken.
- **Quoted-path unescaping.** `diffy` decodes git's full C-style quoting —
  named escapes *and* 3-digit octal — via `escaped_filename`, and rejects
  non-UTF-8 in the `str` variant with `InvalidUtf8Path`. That matches the old
  `gix::quote::ansi_c::undo` + `String::from_utf8` behavior exactly, error
  case included.

Two encoding details had to be matched rather than assumed:

- `HunkRange::start()`/`len()` are the **literal hunk-header numbers**
  (`@@ -1,3 +1,3 @@` → `start == 1`), not 0-based indices — `diffy`'s own
  `diff/mod.rs` adds 1 when *building* a range from an index. So
  `old_start`/`new_start`/`old_lines`/`new_lines` map across directly, and the
  `@@ -0,0 +1 @@` empty-range case falls out for free.
- `Line`'s text **keeps** the trailing newline and has already had the
  `+`/`-`/` ` prefix stripped. The adapter re-derives `DiffLine.old`/`new` by
  counting from the hunk header (context advances both, deletion only old,
  insertion only new, same as before) and strips the line ending the way
  `str::lines` does.

One deliberate behavior difference: `PatchSet` yields a single
`Err("no valid patches found")` for input containing no patch at all, where a
patch with no `diff --git` section used to yield an empty file list.
`patch_diffs` now short-circuits to an empty `CommitDiff` when no line starts
with `diff --git ` — the same guard `diffy`'s internal `find_gitdiff_start`
uses — so `empty_or_unparseable_patch_yields_no_files` still holds.

Note: the earlier estimate above ("~700 line hand-rolled parser") was too
high; the parser itself was 370 lines, and the ~1300 lines of tests for it
remain, now serving as the fixture-by-fixture verification for the
crate-backed implementation.

---

## 6. Remove the `tasks: Vec<Task<...>>` + `push_task` boilerplate — use `Task::detach()`

Verified against the actual pinned GPUI revision
(`~/.cargo/git/checkouts/zed-a70e2ad075855582/1870e26/crates/scheduler/src/executor.rs:375-573`
and `crates/gpui/src/executor.rs:32-63`).

Six different stores carry the exact same field and method, copy-pasted:

```rust
tasks: Vec<Task<Result<(), Error>>>,

fn push_task(&mut self, task: Task<Result<(), Error>>) {
    self.tasks.retain(|task| !task.is_ready());
    self.tasks.push(task);
}
```

at `backend.rs:87-91,164-169`, `checkouts.rs:106-110,180-185`,
`local_repos.rs:13-23`, `profile.rs:72-80,136-141`, `repo.rs:82-86` (plus an
inlined copy of the same retain-then-push at `repo.rs:236-240` and
`repo.rs:373-377`), and `repo_list.rs:56-60,137-142`.

`Task`'s own doc comment (`scheduler/src/executor.rs:375-380`) says exactly
what this boilerplate exists to avoid: "If you drop a task it will be
cancelled immediately. Calling `Task::detach` allows the task to continue
running, but with no way to return a value." `Task::detach(self)`
(`executor.rs:552-559`) does precisely that, and `TaskExt::detach_and_log_err`
(`gpui/src/executor.rs:35-61`, already referenced in this project's own
`.rules` file) additionally logs an `Err` without any manual `match`. None
of these stores' spawned tasks need cancel-on-drop semantics: every
continuation already does `this.update(cx, ...).ok()` or propagates through
`?`, so if the owning entity is gone by the time the task finishes, the
update is a harmless no-op — exactly the "tolerate the entity being gone"
pattern already used everywhere in this codebase (see the `.ok()` calls
throughout `backend.rs`). Storing the task and pruning it on every push
buys nothing here; `.detach()` (or `.detach_and_log_err(cx)` where the
continuation only logs on failure) replaces both the field and the method:

```rust
// today
self.push_task(cx.spawn(async move |this, cx| {
    if let Err(e) = task.await {
        this.update(cx, |_this, cx| cx.emit(BackendEvent::error(e.to_string()))).ok();
    }
    Ok(())
}));

// replacement — no field, no prune, no manual match
cx.spawn(async move |this, cx| {
    if let Err(e) = task.await {
        this.update(cx, |_this, cx| cx.emit(BackendEvent::error(e.to_string()))).ok();
    }
})
.detach();
```

Delete the `tasks` field and `push_task` method from all six stores, and
change every `self.push_task(cx.spawn(...))` call to `cx.spawn(...).detach()`
(or `.detach_and_log_err(cx)` when the closure's only job is to log the
error). The one place that must **not** just detach is `push_repo_from`'s
returned `Task<Result<PushOutcome, Error>>` (`backend.rs:815-902`) — that
task is deliberately returned to the UI caller (so the panel can `.await`
it and show a spinner) and already isn't stored in a `tasks` list today, so
it's unaffected by this cleanup.

`crates/workspace` has the same pattern too, and there it's a real bug, not
just style — see §14.

---

## 7. Render granularity / `cx.notify()` audit

Checked every `cx.notify()` call in `signed_state` (18 call sites) and how
`workspace` views consume each store. Overall this is **already
well-partitioned**, not a smell:

- Every panel (`IssuesView`, `PullRequestsView`, `IssueDetailView`,
  `CommitDiffView`, `RepoDetailView`, `PullRequestDetailView`,
  `NewPullRequestView`, `DiffPane`) is its own `Entity<T>`/`Render` impl —
  `cx.notify()` on a store only invalidates the views actually observing
  that store's `Entity`, not a monolithic root view.
- `IssuesView`/`PullRequestsView` already memoize derived rows behind a
  `(store.version(), filter)` cache key (`issues.rs:68-72`, `323-333`;
  `pull_requests.rs:75-79`, `331-341`), and both use
  `VirtualListScrollHandle` for virtualization — so a store `notify()`
  doesn't force rebuilding or laying out off-screen rows.
- `sync_bootstrap`'s per-percent progress `cx.notify()`
  (`backend.rs:1257-1264`) is already throttled to *distinct percentage
  points* (`if progress.current > 0 && percent != last_percent`,
  `backend.rs:1254`), and nothing in `workspace` reads `Backend::sync_progress()`
  directly (`grep -rn "sync_progress()" crates/workspace` → no matches), so
  this never drives a visible re-render on its own.

### One real waste found: `RepoListStore` re-queries the DB on every sync tick

`RepoListStore`'s backend subscription (`repo_list.rs:76-109`) treats
`BackendEvent::SyncProgress { .. }` as relevant on its own:

```rust
BackendEvent::Synced | BackendEvent::SyncProgress { .. } => true,
```

Every distinct percentage tick of the bootstrap announcements/deletions
sync calls `this.refresh(cx)`, which is debounced 300ms
(`REFRESH_DEBOUNCE`, `repo_list.rs:16`) and coalesced by `RefreshGate` — so
it's not literally one DB round-trip per percent, but it is several
(bounded by sync duration / 300ms) full re-scans of announcements +
deletions + state events + activity + counts (`run_refresh`,
`repo_list.rs:184-294`) while a single sync is still in flight, instead of
one at the end. This is a deliberate trade-off for progressive reveal (the
repo list fills in live instead of jumping once at 100%), so it's not a
bug, but if that progressive reveal isn't a feature you actually want,
dropping `SyncProgress` from the "relevant" match (keep only `Synced`) removes
several redundant background-thread DB scans per sync for free. Worth a
product decision, not just a code fix.

No other store subscribes to `SyncProgress` (`RepoStore`, `CheckoutsStore`
do not — checked their subscription callbacks), so this is fully isolated
to `RepoListStore`.

---

## 8. Relay add/connect: stop round-tripping through strings, stop reconnecting the whole pool

Flagged example (`backend.rs:1071-1074`):

```rust
for url in latest_grasp_list_servers(events) {
    client.add_relay(url.as_str()).await.ok();
}
client.connect().await;
```

Two separate problems, both verified against `nostr-sdk/src/client/url.rs:40-50`
and `nostr-sdk/src/client/api/connect.rs:1-49`:

1. **`.as_str()` is a pointless round trip.** `latest_grasp_list_servers`
   already returns `RelayUrl` values (parsed, validated). `RelayUrlArg`
   (what `add_relay` actually accepts) has a direct `impl From<RelayUrl>`
   and `impl From<&RelayUrl>` (`client/url.rs:40-50`) — passing the
   `RelayUrl` itself skips a second `RelayUrl::parse` that `.as_str()`
   forces (`client/url.rs:26,35`, the `String` variant of `RelayUrlArg`
   re-parses on `try_into_relay_url`). Just pass `url`, not `url.as_str()`.
2. **`client.connect()` connects every relay in the pool, not just the one
   you added.** Verified in `connect.rs:36-48`: `Client::connect()`'s
   `IntoFuture` unconditionally calls `self.client.pool().connect()`, with
   no target selection at all — it iterates every relay currently in the
   pool. Calling it after adding 1-2 new relays re-issues a connect
   attempt to *every* relay already connected too. The `AddRelay` builder
   already has the right primitive: `.and_connect()` (`client/api/add.rs:127-132`),
   which is threaded straight into `pool.add_relay(url, capabilities, connect, opts)`.
   Verified in `pool/mod.rs:157-197` that this is correct **even when the
   relay already exists** in the pool: the pool's `add_relay` checks for an
   existing entry and, if `connect` is `true`, calls `relay.connect()` on
   the existing relay too (`pool/mod.rs:191-194`) — so `.and_connect()` is
   never wrong to use, whether the relay is new or already known.

```rust
// replacement
for url in latest_grasp_list_servers(events) {
    client.add_relay(url).and_connect().await.ok();
}
```

The same two problems repeat at every other relay-add call site — fix all
of them the same way:

- `Backend::bootstrap` (`backend.rs:178-187`): the `BOOTSTRAP_RELAYS` loop
  and the `INDEXER_RELAYS` loop (which also sets `.capabilities(...)`,
  chain `.and_connect()` onto the same builder) both currently defer to one
  trailing `client.connect().await`.
- `connect_repo_relays` (`backend.rs:1446-1449`): today calls `client.add_relay(url).await?;`
  then `client.connect_relay(url).await?;` as two separate round trips —
  collapse to one `client.add_relay(url).and_connect().await?;`.
- `stage_event_on_relay` (`backend.rs:1772-1782`): same fix, and this one
  currently calls the pool-wide `client.connect().await` just to connect
  the single relay it's about to stage an event on.

### Delete the `add_relays` wrapper (`Backend::add_relays`, `backend.rs:1149-1173`)

Its only two callers (`create_repository`, `backend.rs:505-508`;
`publish_local_repo`, `backend.rs:652-655`) do this today:

```rust
this.update(cx, |this, cx| {
    let urls: Vec<String> = servers.iter().map(ToString::to_string).collect();
    this.add_relays(urls, cx);
})?;
```

`servers` is already `Vec<RelayUrl>` at both call sites — stringifying it
only to have `add_relays` parse it straight back into `RelayUrl` inside
`client.add_relay(&url)` is pure waste, on top of the wrapper itself being
another `cx.spawn` + `push_task` + error-emit layer (§6) around what is,
with the fix above, a two-line loop. Both call sites are already inside a
`cx.spawn(async move |this, cx| ...)` with `client` reachable — inline it:

```rust
let client = this.update(cx, |this, _cx| this.client.clone())?;
for relay in &servers {
    client.add_relay(relay).and_connect().await.ok();
}
```

Delete `Backend::add_relays` entirely once both call sites are inlined.

---

## 9. `create_repository`'s flow is backwards: it inits a mirror, then clones it into the real destination

This is a real business-logic flaw, not just a style issue. Today
(`backend.rs:437-501`):

1. `signed_git::init_repository(&path, &name, &description)` — `path` is
   `GitCache::repo_path(&addr)`, the app's **internal mirror cache**
   location (`crates/signed_git/src/lib.rs:29-33`), not anywhere the user
   asked for. This creates a full worktree with an initial commit *there*.
2. `signed_git::clone_repo(&[mirror_url], &destination)` — `destination` is
   `folder.join(dir_name)`, the folder the user actually picked. This
   clones the mirror just created in step 1 into the real target, via a
   `file://` URL (`Url::from_file_path(&path)`, `backend.rs:481-483`).
3. The push (`push_staged_to_grasps`, called with `path` = the **mirror**,
   not `destination`) pushes the mirror's objects to the grasp servers.
4. `origin` gets set on *both* the mirror (`backend.rs:463-466`) and the
   destination (`backend.rs:492-495`).

So a brand-new repository gets initialized twice and checked out twice for
what is, at that point, one README and one commit — and the thing that
actually gets pushed (the mirror) isn't the thing the user is left looking
at (the destination).

Checked `signed_git::init_repository` itself (`signed_git/src/lib.rs:401-477`):
it already creates the target directory (`std::fs::create_dir_all(path)`),
runs `gix::init(path)`, and leaves a fully checked-out worktree with the
README written to disk and the index populated — i.e., it already produces
exactly what step 2's clone is redundantly reproducing. There is no reason
step 1 and step 2 are two different paths.

Compare with `publish_local_repo` (`backend.rs:599-754`), the sibling flow
for an *existing* local repo: it operates on the user's real folder
directly (`signed_git::worktree_ref_state(&path)`, `root_commit(&path)`) —
no mirror, no extra clone. `create_repository` is the odd one out.

**The mirror doesn't need to be pre-populated at creation time at all.**
`GitCache::ensure_clone(addr, clone_urls)` (`signed_git/src/lib.rs:45-63`)
already exists precisely to populate the mirror lazily — open it if it's
there, clone it from the announcement's `clone_urls` if it's not — and
it's already what `RepoDetailView::load_repo` calls for every repo,
including the user's own (`workspace/src/views/repo_detail/mod.rs:425-428`).
By the time the UI navigates to the new repo's detail view after
`create_repository` returns, the push has already succeeded, so
`ensure_clone` will clone straight from the just-pushed grasp server —
exactly the same lazy path every other repo already takes. No special
casing needed.

I checked whether any `workspace` call site compounds this (e.g. by cloning
*again* right after `create_repository` returns) — it doesn't:
`sidebar/create_repo_dialog.rs`'s `create_repository` handler
(`create_repo_dialog.rs:193-222`) just calls `backend.create_repository(...)`
and applies the returned `Announcement`; the flaw is fully contained inside
`Backend::create_repository` itself.

**Replacement:** initialize directly at `destination`, push from
`destination`, set `origin` once:

```rust
let commit = signed_git::init_repository(&destination, &name, &description)?;
// ... build the announcement using `commit` as before ...
// push_staged_to_grasps(..., path = &destination, ...) instead of the mirror path
if let Some(base) = servers.first().and_then(grasp_base_url) {
    signed_git::set_origin(&destination, &format!("{base}/{owner}/{repo_id}.git"))?;
}
```

Delete the mirror `init_repository` call, the `clone_repo` call, the
`Url::from_file_path` mirror-URL construction, and the mirror-side
`ensure_origin` call. This removes a full extra `gix::init` + checkout +
clone from repo creation, and makes `create_repository` consistent with
how `publish_local_repo` already treats the user's working copy as the one
source of truth.

---

## 10. Bootstrap-on-construction should go through `cx.defer`, not run synchronously in `new`

Verified against the pinned GPUI revision
(`crates/gpui/src/app.rs:1999-2005`, `crates/gpui/src/app/context.rs:296-315`).

`App::defer(&mut self, f: impl FnOnce(&mut App) + 'static)` — "Schedules
the given function to be run at the end of the current effect cycle,
**allowing entities that are currently on the stack to be returned to the
app**." That's precisely the situation every one of these constructors is
in: `Self` is still being built inside the `cx.new(|cx| ...)` closure when
it reaches out and kicks off real work. `Context<T>::defer_in` also exists
(`app/context.rs:296-315`) but takes a `&Window` — it's for window-bound
views, not the headless global stores below, none of which are constructed
with a `Window` in scope. For these, the applicable API is the window-less
`cx.defer(...)`, reached through `Context<T>`'s `Deref<Target = App>`
(`app/context.rs:25-34`), capturing a `WeakEntity<Self>` to get back into
`Self` once deferred:

```rust
// today, backend.rs:148-160
let mut this = Self { /* ... */ };
this.bootstrap(cx);
this

// replacement
let mut this = Self { /* ... */ };
let weak = cx.entity().downgrade();
cx.defer(move |cx| {
    weak.update(cx, |this, cx| this.bootstrap(cx)).ok();
});
this
```

The same pattern — a constructor that calls its own bootstrap-ish method,
or reaches into another entity, before returning `Self` — repeats in every
store:

| Store | Constructor call site | What it kicks off synchronously |
|---|---|---|
| `Backend` | `backend.rs:159` | `bootstrap(cx)` — adds/connects `BOOTSTRAP_RELAYS`/`INDEXER_RELAYS`, restores the session |
| `RepoListStore` | `repo_list.rs:120-123` | `subscribe_remote(cx)` (negentropy sync against bootstrap relays) + `refresh_initial(cx)` |
| `RepoStore` | `repo.rs:154-159` | `subscribe_remote`, `connect_announced_relays`, `refresh` — each one reaches into the global `Backend` entity |
| `CheckoutsStore` | `checkouts.rs:172-174` | `refresh(cx)` |
| `LocalReposStore` | `local_repos.rs:44` | `rescan(cx)` |
| `ProfileStore` | `profile.rs:119-121` | spawns the batched profile-fetch loop |

Wrap each of these the same way `Backend::new` is shown above. This isn't
about a currently-observed crash (nothing panics today, because everything
past the initial synchronous field assignment already goes through
`cx.spawn`/`cx.background_spawn`, which only runs later anyway) — it's
about not mixing "construct plain state" with "kick off side effects that
talk to other entities" in the same synchronous call, which is exactly what
`defer` exists to separate, per its own doc comment.

---

## 11. Split independently-observed state into child entities

`Backend::pushing_repos` (`backend.rs:88`) is `Arc<Mutex<HashSet<RepoAddr>>>`
— it bypasses GPUI's entity system entirely. A view that wants to show "is
repository X currently pushing" has no way to `cx.observe` this; it can
only poll a `Mutex` by hand, and any UI update requires some *other*
notify to happen to piggyback on. Meanwhile every view that only cares
about, say, `current_user` still gets re-invoked on `Backend::notify()`
fired for unrelated reasons (a `sync_progress` tick, a new relay connecting),
because the whole `Backend` is one entity and `cx.notify()` invalidates all
of its observers indiscriminately.

GPUI's own model is built for exactly this split: an `Entity<T>` works for
any `T: 'static`, not just `Render`-able view state (see the project's own
GPUI notes: "Whenever you need to store application state that
communicates between different parts of your application, you'll want to
use GPUI's entities"). Where a piece of a bigger store's state changes on
its own schedule and has its own, narrower set of observers, pull it out
into a child entity:

```rust
pub struct Backend {
    client: Client,
    signer: UniversalSigner,
    current_user: Option<PublicKey>,
    sync_progress: Option<(u64, u64)>,
    passphrase_required: bool,
    pushing_repos: Entity<HashSet<RepoAddr>>, // was Arc<Mutex<HashSet<RepoAddr>>>
}
```

A view that only cares whether repo `X` is pushing does
`cx.observe(&backend.read(cx).pushing_repos, |this, pushing, cx| ...)` and
is left alone by every other `Backend` change. `PushGuard`
(`backend.rs:99-110`) becomes a guard that calls
`pushing_repos.update(cx, |set, cx| { set.remove(&addr); cx.notify(); })`
on drop instead of locking a raw `Mutex` — same RAII shape, but now it's a
real, observable GPUI entity instead of a side channel next to the entity
system. Apply the same split to any other `Backend`/store field where the
set of interested observers is a strict subset of the store's full
observer list.

This principle is also the reason **not** to merge `LocalReposStore` and
`RepoListStore` into one entity — see §13.

---

## 12. One debounce at the source, not one per store

Flagged example — the notification pump (`backend.rs:126-146`):

```rust
let mut notifications = pump_client.notifications();
while let Some(notification) = notifications.next().await {
    let ClientNotification::Event { event, .. } = notification else { continue };
    let update = Update::from_event(&event);
    if this.update(cx, |_, cx| cx.emit(BackendEvent::NostrUpdate(update))).is_err() {
        break;
    }
}
```

Every single relay-delivered event is emitted as its own
`BackendEvent::NostrUpdate`, immediately. During a negentropy sync
(exactly the bursty case §6/§7 already discuss), this can be hundreds of
emits in a short window. Four different stores (`RepoStore`,
`RepoListStore`, `CheckoutsStore`, and transitively `ProfileStore`) each
subscribe to `Backend` and independently run their own `RefreshGate`
debounce/coalesce dance in response — the same burst gets debounced four
times, once per listener, instead of once at the point it actually enters
the system.

Centralize it: batch what the pump itself emits, and let each store react
to a batch instead of a stream of singles. The pump already owns the one
place where the burst originates, so it's the natural place to coalesce:

```rust
let pump = cx.spawn(async move |this, cx| {
    let mut notifications = pump_client.notifications();
    let mut pending: Vec<Update> = Vec::new();

    loop {
        let next = cx.background_executor().timer(PUMP_DEBOUNCE).fuse();
        futures::select_biased! {
            notification = notifications.next() => {
                let Some(notification) = notification else { break };
                let ClientNotification::Event { event, .. } = notification else { continue };
                pending.push(Update::from_event(&event));
            }
            _ = next => {
                if pending.is_empty() { continue; }
                let batch = std::mem::take(&mut pending);
                if this.update(cx, |_, cx| cx.emit(BackendEvent::NostrUpdate(batch))).is_err() {
                    break;
                }
            }
        }
    }
    Ok(())
});
```

(Sketch — the real version needs `BackendEvent::NostrUpdate` to carry
`Vec<Update>` instead of `Update`, and every subscriber's relevance check —
`RepoStore`, `RepoListStore`, `CheckoutsStore`, `ProfileStore` — to check
"does *any* update in the batch match" instead of one `Update`. That's a
mechanical change to four match arms.)

This doesn't make each store's own `RefreshGate` fully redundant:
`Published`/`Synced`/`SyncProgress` events are emitted directly by
whichever method triggered them (a local `send`, a sync completing), not
through the pump, and can still arrive close together independently of
relay traffic. But those are one-off, user-triggered events, not the
hundred-events-in-a-burst case — so once the pump absorbs the dominant
source of bursts, each store's debounce window can likely shrink
significantly (or, for stores that only ever see one trigger at a time in
practice, be dropped in favor of "fold into the in-flight run" without a
timer at all). Worth measuring after the pump-side batching lands, rather
than speculatively resizing four timers up front.

---

## 13. `local_repos.rs` + `repo_list.rs`: merge the files, not the entities

These two are structurally near-identical: both hold an `Arc<Vec<T>>`
snapshot, refresh it in the background on a trigger, swap it in with
`cx.notify()`, and carry their own `Global` wrapper + `global()`/`set_global()`
pair + `tasks`/`push_task` boilerplate (§6). That similarity is real and
worth collapsing — but checked who actually reads each one before deciding
*how*:

```
grep -rn "RepoListStore::global" crates/     → 9 call sites
grep -rn "LocalReposStore::global" crates/    → 5 call sites
```

Only **two** places read both together: `CheckoutsStore::new`/`run_refresh`
(`checkouts.rs:126-136`, `checkouts.rs:339-344`) and `SidebarPanel::new`/`refresh`
(`sidebar/mod.rs:55-65`, `sidebar/mod.rs:128-142`). Everywhere else reads
exactly one:

- `RepoListStore` alone: `RepoStore::action_announcement` (`repo.rs:1071-1078`),
  `RepoDetailView::open_upstream` (×2, `mod.rs:1009-1013`, `1034-1044`),
  `RepoDetailView::fork_row` (`mod.rs:2596-2606`),
  `NewPullRequestView::fork_candidates` (`new_pull_request.rs:569-577`),
  `RepoListView::new` (`views/repo_list.rs:111-121`).
- `LocalReposStore` alone: `RepoDetailView::apply_announcement`
  (`mod.rs:1875-1877`), `SidebarPanel::render_repos`'s rescan button
  (`sidebar/mod.rs:306-309`).

Given that, collapsing them into **one `Entity`** (one struct holding both
`Vec`s, one `cx.notify()` for both) would make every one of those ~12
single-store readers pay for the other store's unrelated refreshes —
exactly what §11 says not to do. Wrapping them in a parent that holds two
child entities (`RepoDirectory { local: Entity<LocalRepos>, remote:
Entity<RemoteRepos> }`) avoids that specific problem, but then every one of
those same ~12 call sites has to change from `RepoListStore::global(cx)` to
`RepoDirectory::global(cx).read(cx).remote` — an extra hop added everywhere,
in exchange for saving exactly one `Global` wrapper struct. Not a good
trade for a codebase this size.

**Recommendation:** merge the two **files** into one module
(e.g. `repos.rs`), keeping `LocalReposStore` and `RepoListStore` as two
fully independent structs, each still its own `Entity`/`Global` exactly as
today — same public API, same `global()`/`set_global()` pairs, zero
call-site churn. The merge is justified purely as "these are the app's two
repo-listing stores, they belong next to each other," per the project's own
`.rules` guidance to avoid many small files for closely related logic —
not as a reason to share a notify cycle between two things with almost
entirely disjoint observers.

---

## 14. `crates/workspace` has the same task-list pattern as §6 — and there it's an actual bug

§6 covers `signed_state`'s 6 stores, where the unpruned-`Vec<Task>` pattern
is a style/complexity concern with no observed failure, because
`push_task` always pruned before pushing. `crates/workspace` has the exact
same field-and-push shape in 4 views, but **most of it never prunes**:

```
grep -rn "tasks.push(task)" crates/workspace/   → 17 call sites
grep -rn "tasks.retain"      crates/workspace/   → 1 call site (pull_request_detail.rs:258)
```

- `RepoDetailView.tasks` (`mod.rs:178-179`, doc comment: "finished tasks are
  pruned on every push" — **this is stale/incorrect**, no `.retain()`
  precedes any of its 11 push sites: `mod.rs:384-388`, `532-536`, `650-654`,
  `773-777`, `835-839`, `877-881`, `949-953`, `1061-1065`, `1132-1136`,
  `1219-1223`, `1315-1319`).
- `NewPullRequestView.tasks` (`new_pull_request.rs:80-84`): 5 push sites,
  none pruned (`445-449`, `475-479`, `697-701`, `894-898`, `997-1001`).
- `CommitDiffView` (`diff.rs:407-411`): 1 push site, not pruned.
- `PullRequestDetailView.tasks` (`pull_request_detail.rs:68-72`): the one
  correct one — `load` (`pull_request_detail.rs:256-260`) does
  `self.tasks.retain(|task| !task.is_ready()); self.tasks.push(task);`.

So `RepoDetailView.tasks` and `NewPullRequestView.tasks` grow **unbounded**
for as long as the panel stays open: every file preview, ref switch, commit
load, worktree reload, or fork comparison appends one more `Task` that is
never removed. This is a real memory-growth bug, not just a style
preference — a repo detail panel left open through a long session
accumulates one `Task` per interaction, forever.

Apply the same fix as §6: delete the `tasks` field from all four views and
`.detach()` (or `.detach_and_log_err(cx)`) at every one of the 17 call
sites. Every continuation already tolerates the view being gone
(`this.update_in(cx, ...).ok()`/`?`, same pattern as `signed_state`), so
nothing here needs cancel-on-drop semantics either. Worth noting
`repo_detail/init_dialog.rs`'s `init_repository` (`init_dialog.rs:187-206`)
already does exactly this — `cx.spawn(...).detach()`, no task list at all —
so the fix is bringing the other 4 views in line with a pattern that
already exists once in the same crate.

---

## 15. `Vec<Url>` → `Vec<String>` conversion sprawl — fix the 3 `signed_git` signatures, not the 8 call sites

`Announcement::clone` is `Vec<Url>` (`signed_core/src/model.rs`, `Url` being
`nostr`'s re-export of the `url` crate's `Url`, `nostr/src/types/url.rs:15`,
`pub use url::*;`). Every call site that needs to hand those URLs to
`signed_git` first stringifies them:

```
grep -rn "\.map(ToString::to_string)\.collect" crates/signed_state crates/workspace
```

finds it at `repo.rs:1005-1008` (`merge_pull_request`), `repo.rs:1227`
(`clone_to_folder`), `workspace/repo_detail/mod.rs:397` (`load_repo`),
`new_pull_request.rs:595` and `602` (`choose_fork`, twice — once for the
fork, once for the base), and `pull_request_detail.rs:146-149` and
`738-741` (`load`, `clone_urls_of`). Seven call sites, all producing a
`Vec<String>` that gets handed straight to `signed_git::clone_repo`,
`GitCache::ensure_clone`, or `fetch_repo_refs`.

The root cause is those three functions' signatures, not the call sites.
Verified in `signed_git/src/lib.rs`:

```rust
pub fn clone_repo(clone_urls: &[String], path: &Path) -> Result<()> { ... }        // lib.rs:125
pub fn ensure_clone(&self, addr: &RepoAddr, clone_urls: &[String]) -> Result<...>  // lib.rs:45
pub fn fetch_repo_refs(repo_path: &Path, urls: &[String], refspec: &str) -> ...   // lib.rs:701
```

all three only ever read each URL as `&str` internally, through the shared
`try_each_url(urls: &[String], ...)` helper (`lib.rs:350`), which does
`attempt(url)` where `url: &String` auto-derefs. Checked whether `Url` could
be passed directly instead of allocating a `String` per URL: **yes** —
`url::Url` implements `AsRef<str>` directly (verified in the pinned `url`
crate source, `url-2.5.8/src/lib.rs:2867`). Making the three functions
generic removes the conversion at every call site instead of patching each
one:

```rust
fn try_each_url<U: AsRef<str>, F>(urls: &[U], verb: &str, mut attempt: F) -> Result<()>
where
    F: FnMut(&str) -> Result<()>,
{
    for url in urls {
        match attempt(url.as_ref()) { /* ... */ }
    }
    /* ... */
}

pub fn clone_repo<U: AsRef<str>>(clone_urls: &[U], path: &Path) -> Result<()> { ... }
pub fn ensure_clone<U: AsRef<str>>(&self, addr: &RepoAddr, clone_urls: &[U]) -> Result<gix::Repository> { ... }
pub fn fetch_repo_refs<U: AsRef<str>>(repo_path: &Path, urls: &[U], refspec: &str) -> Result<()> { ... }
```

After this, every one of the 7 call sites above passes `&announcement.clone`
directly (a `&[Url]`), deleting the `.iter().map(ToString::to_string).collect::<Vec<String>>()`
line entirely — no allocation, no `Display`-then-reparse round trip,
7 fewer lines of boilerplate for free. (`about.rs`'s `url.to_string()` calls
for on-screen display, `about.rs:55-103`, are unrelated — that's genuine
`Url → SharedString` rendering, not a `signed_git` call, and stays as-is.)

The `Vec<RelayUrl> → Vec<String>` conversions for `add_relays`/`add_relay`
(§8) are a separate root cause (`RelayUrl` doesn't implement `AsRef<str>`,
checked `nostr/src/types/url.rs`) and are already fixed by §8's move to
`RelayUrlArg`'s native `From<RelayUrl>`/`From<&RelayUrl>` — no further
change needed there.

---

## 16. `.clone()` audit: the dense clusters in `backend.rs` are the correct idiom, not a flaw

Went through every `.clone()` in `create_repository`, `publish_local_repo`,
and `push_repo_from` (the three functions with the highest clone density)
looking for copies that could be replaced by a reference. All of them are
`Client`/`UniversalSigner`/`PathBuf`/`String`/`RelayUrl` values being moved
into a separate `'static async move` block for `cx.background_spawn`, which
Rust's ownership rules require to own its captures — this is exactly the
shadowing-clone pattern the project's own `.rules` file endorses ("Use
variable shadowing to scope clones in async contexts for clarity, minimizing
the lifetime of borrowed references"). `Client` itself is a cheap `Arc`
handle clone (`Client(Arc<InnerClient>)`, verified `nostr-sdk/src/client/mod.rs:74`),
so even the frequent `client.clone()`/`signer.clone()` pairs before each
`background_spawn` are not doing a deep copy. No changes recommended here —
noting this so it's clear the dense clone clusters were checked, not
skipped, and found to be inherent to the async-boundary structure rather
than avoidable duplication.

---

## 17. Business logic that leaked into `crates/workspace` and should move to `signed_core`/`signed_state`

Direct answer to "can the view side be thinner": yes, and not speculatively —
found one confirmed duplicate, one cluster of misplaced domain parsing, and
one mutating-flow split across the view/store boundary. The test used to
tell "fine to stay in the view" from "should move": read-only git/data
queries that only shape *what one specific view renders* (diffs, commit
lists, tree snapshots — already audited clean in §5/§7) are fine where they
are; anything that **parses a Nostr event's domain tags**, **decides what's
NIP-34-valid/eligible**, or **builds the payload of a mutating operation**
is domain logic and belongs in `signed_core`/`signed_state`, reusable and
testable without GPUI.

### Confirmed duplicate: `current_commit_of`

`signed_core/src/model.rs:183-190` (private, used internally by
`pull_request_patches`) and `workspace/repo_detail/pull_request_detail.rs:709-716`
are **the same function, byte-for-byte**:

```rust
fn current_commit_of(event: &Event) -> Option<String> {
    event
        .tags
        .iter()
        .find_map(|tag| match Nip34Tag::parse(tag.as_slice()) {
            Ok(Nip34Tag::CurrentCommit(commit)) => Some(commit.to_string()),
            _ => None,
        })
}
```

It was reimplemented in `workspace` because `signed_core`'s copy is private.
Fix: make `signed_core`'s `current_commit_of` `pub fn`, delete
`workspace`'s copy, import the shared one.

### A whole cluster of NIP-34 tag parsing lives next to it, same shape, same problem

Still in `pull_request_detail.rs`, zero GPUI/UI dependency in any of them:

- `merge_base_of(event: &Event) -> Option<String>` (`pull_request_detail.rs:721-729`)
- `clone_urls_of(event: &Event) -> Option<Vec<String>>` (`pull_request_detail.rs:734-742`)
- `branch_name_of(event: &Event) -> Option<String>` (`pull_request_detail.rs:745-753`)
- `latest_update<'a>(events: impl Iterator<Item = &'a Event>, root: &Event) -> Option<&'a Event>` (`pull_request_detail.rs:756-766`) —
  walks a PR's `GitPullRequestUpdate` events to find the newest revision from
  the root's author, the exact same *shape* of problem `signed_core::model::pull_request_patches`
  already solves for patch series (`model.rs:95-135`, forward/backward
  reply-chain walking).

These all take a plain `&Event` (or an iterator of them) and return plain
data — nothing here needs `Context`/`Window`/`cx`. They belong next to
`Announcement::from_event`, `parse_state`, and `pull_request_patches` in
`signed_core`, as `pub fn`s with their own unit tests (this file's test
module, `pull_request_detail.rs:840+`, already builds fixture events with a
local `signed()`/`pr_root()` helper — `signed_core`'s test module has the
same fixture-building pattern already; the tests move with the functions,
no new test infrastructure needed).

### A mutating flow split across the view/store boundary: patch generation in `submit`

`NewPullRequestView::submit` (`new_pull_request.rs:900-1000`) does this
before calling into the store:

```rust
let patch = cx.background_spawn({
    /* ... */
    async move { format_patch_between(Path::new(&repo_path), &merge_base, &compare_ref) }
}).await;

let patch = match patch {
    Ok(patch) if !patch.is_empty() => patch,
    Ok(_) => { /* "No commits between the branches to propose" */ return Ok(()); }
    Err(error) => { /* "Failed to generate the patch: {error}" */ return Ok(()); }
};

store.update(cx, |store, cx| {
    store.open_pull_request(/* subject, description, branch_name, patch, ... */)
});
```

`RepoStore::open_pull_request` (`repo.rs:554-558`) and `update_pull_request`
(`repo.rs:829-833`) both already take a ready-made `patch: String` — a
reasonable, uniform boundary in general (it's also exactly right for
`pull_request_detail.rs`'s "update PR" dialog, `pull_request_detail.rs:648-700`,
where the patch is literally pasted by the user into a textarea, no git
involved). But for the "compare two branches" flow, *generating* that patch
text — calling `signed_git::format_patch_between`, deciding empty-diff is
an error, and wording that error — is exactly the same kind of "turn git
state into the payload of a Nostr publish" work `Backend::create_repository`/
`publish_local_repo` already do internally (`worktree_ref_state`,
`root_commit`), just for a different event kind. It shouldn't be the one
case where that responsibility sits in the view instead of the store.

**Recommendation:** give `RepoStore` (or a free function in `signed_state`
it calls) a method that takes the two refs instead of a ready-made patch,
e.g. `RepoStore::open_pull_request_from_refs(repo_path, base_ref, compare_ref,
subject, description, draft, cx) -> Task<Result<(), Error>>`, which does
the `format_patch_between` + empty-check + `open_pull_request` sequence
internally and returns one descriptive error on failure. `submit` shrinks to
gathering the text-field values and calling it, then closing the panel —
no `signed_git` import needed in `new_pull_request.rs` at all for this path.

### Borderline, worth doing while touching the same file: `fork_candidates`/`fork_namespace`

`fork_candidates` (`new_pull_request.rs:117-135`) filters/partitions
`&[Announcement]` into "own" vs. "others" fork sources using the
already-domain `Announcement::is_fork_of` predicate (`signed_core/src/model.rs:281-285`,
correctly reused, not reimplemented) — it's pure data transformation with no
GPUI dependency, and has its own private unit tests in `new_pull_request.rs`
building fixture announcements, again duplicating test-fixture machinery
`signed_core`'s own test module already has. `fork_namespace`
(`new_pull_request.rs:108-114`, formats the `refs/fork/<owner>/<id>`
namespace string) is the same shape — small, but it's the one place that
convention is decided, and it pairs naturally with `signed_git`'s ref-naming
conventions. Both are safe, low-risk moves to `signed_core`: unlike
`fork_display_name`/`shorten_owner`/`truncate_label`/the `*_source_item`
builders in the same file (genuine presentation logic — `SharedString`
truncation, `PopupMenuItem` construction — correctly left where they are),
these two don't touch a single GPUI type.

### What's already thin and should stay exactly where it is

For contrast, checked `RepoDetailView`'s git-touching methods
(`load_repo`, `load_commits`, `switch_ref`, `reload_worktree`,
`catch_up_worktree`, `push_unpushed_checkout`) and `NewPullRequestView::reload_compare`
(`new_pull_request.rs:808-897`, computing `merge_base`/commit
list/diff purely to populate the compare pane): these call `signed_git`
directly too, but only to compute **read-only data this one view renders**
— nothing here is parsed from a Nostr event, decides NIP-34 eligibility, or
builds a publish payload. Moving these into `signed_state` would just add
an indirection layer with no reuse benefit, contradicting "keep it simple."
Same verdict as `create_repo_dialog.rs`'s and `init_dialog.rs`'s handlers
(§9): they already do nothing but gather form input and call one `Backend`
method.

---

## Outcome

All 14 items below are implemented and verified, listed in the order they were
done — mechanical removals first, the largest diff (§2) and the riskiest swap
(§5) last.

1. **Delete the fetch/sync dedup cache** (§3). `recent_fetches`,
   `fetch_recently_started`, `fetch_fingerprint` and `FETCH_DEDUP_WINDOW` are
   gone, along with the `DefaultHasher`/`Hash`/`Hasher`/`Instant` imports they
   needed. Each call site keeps its own guard.
2. **Remove the `tasks: Vec<Task<...>>` + `push_task` boilerplate** on both
   sides (§6, §14) — all six `signed_state` stores and all four
   `crates/workspace` views, 17 push sites, most never pruned. In the views
   this was a real unbounded-growth bug, not just style.
3. **Fix the relay add/connect calls** (§8). No `.as_str()`/`ToString` round
   trips; `add_relay(url).and_connect()` instead of a separate, pool-wide
   `connect()`; `Backend::add_relays` deleted.
4. **Fix `bootstrap_user`** (§1) to sync via negentropy and query the local
   database. `grep -rn "fetch_events" crates/` now returns nothing.
5. **Generalize the three `signed_git` URL-list signatures** to
   `U: AsRef<str>` (§15) and drop the seven
   `.iter().map(ToString::to_string).collect()` call sites, which now pass
   `&[Url]` straight through.
6. **Fix `create_repository`'s init/clone ordering** (§9). It initializes and
   pushes directly at the destination; no mirror, no `clone_repo`, no double
   `origin`.
7. **Merge `local_repos.rs` and `repo_list.rs`** into
   `signed_state/src/repos.rs` (§13), keeping both stores independent
   `Entity`/`Global`s — no call-site change beyond `use` paths.
8. **Route construction-time bootstrap through `cx.defer`** in all six stores
   (§10), with a failed weak upgrade logged rather than silently dropped.
9. **Consolidate the send paths** (§2, §4). The eight layers (`Backend::send`,
   `publish_event`, `publish_task`, `send_fire_and_forget`, `broadcast_event`,
   `RepoStore::send`, …) collapsed to direct
   `client.send_event(&event).broadcast()` calls plus one
   `require_relay_accepted` helper. `retract_events` now sends one NIP-09
   deletion per target, with no `k` tag. The rewrite also fixed the
   inconsistent error surfacing this section flagged: the three call sites
   with divergent control flow now set `last_error` on failure like every
   other `RepoStore` mutation.
10. **Split `pushing_repos` into a child entity** (§11). It is now an
    observable `Entity<HashSet<RepoAddr>>`; the old `PushGuard` and the
    `Arc`/`Mutex` around it are gone.
11. **Centralize the notification-pump debounce** (§12). The pump batches into
    a single `BackendEvent::NostrUpdate(Vec<Update>)` behind a 200 ms window,
    and its three subscribers iterate the batch.
12. **Drop progressive reveal** (§7). `RepoListStore` refreshes once per
    completed sync instead of several times mid-sync.
13. **Replace the hand-rolled `git format-patch` parser with
    `diffy::patch_set`** (§5). 370 lines to 216, no test changed.
14. **Move the misplaced `workspace` domain logic** (§17) to
    `signed_core`/`signed_git`, and give `RepoStore` a refs-in-patch-out
    method so `NewPullRequestView::submit` no longer generates patches itself.

### Deviations, corrections, and findings worth keeping

Most items landed exactly as planned. These are the ones that did not, plus
the non-obvious findings that were only ever recorded in the per-item status
notes this section replaced:

- **`RepoStore::publish` was deliberately kept** (§2), a narrow exception to
  "delete `RepoStore::send`": its four callers (`open_issue`, `reply`,
  `set_status`, `publish_applied_status`) have byte-for-byte identical
  sign+send+check+`last_error` post-conditions. The three callers with
  genuinely divergent control flow (`open_pull_request`,
  `update_pull_request`, `publish_patch_series`) call the SDK inline.
- **`fork_namespace` went to `signed_git`, not `signed_core`** as §17
  sketched. It calls `signed_git::sanitize_path_component`, and `signed_git`
  already depends on `signed_core`, so the sketched direction would have been
  a circular crate dependency.
- **`pushing_repos` was made observable with `AsyncApp::on_drop`, not a `Drop`
  impl** (§11). `Drop::drop(&mut self)` has no `cx`, so it cannot update a GPUI
  entity; Zed's own codebase hits the same wall and falls back to a raw
  `Mutex` (`crates/project/src/project.rs`, `RemotelyCreatedModelGuard`).
- **Calling `cx.entity()` before the entity is registered is safe** (§10) —
  what makes the deferred bootstrap sound. `App::new`'s `cx.entities.reserve()`
  bumps the ref count before `build_entity` runs
  (`app/entity_map.rs:114-117`), and the deferred closure only runs after
  `cx.new`'s `insert_entity`.
- **Gossip stays enabled** in `ClientBuilder` for future NIP-17/NIP-65 work;
  every git-domain send bypasses it explicitly with `.broadcast()` (§4).
- **`pool.sync()` requires the relays to already be in the pool**
  (`pool/mod.rs:679-693`), which is why `Backend::bootstrap` adds
  `BOOTSTRAP_RELAYS` before `sync_bootstrap_only`/`bootstrap_user` run — the
  precondition §8's change relies on.
- **`pushing_repos` has no readers today** (§11): it is `push_repo_from`'s
  internal re-entrancy guard. The UI-facing "is pushing" indicator is the
  pre-existing, already-observable `RepoStore::pushing` boolean.
- **`patch_diffs` short-circuits on input with no `diff --git ` line** (§5),
  because `PatchSet` yields `Err("no valid patches found")` for input holding
  no patch at all, where the old parser returned an empty list.
- **A `let _ =` on a `WeakEntity::update` in `ProfileStore::handle_requests`
  became `.ok()`** (§12), found while touching that file. It now follows the
  project's error-handling rule.
- **Two type-inference anchors had to be re-added by hand**, a recurring cost
  of both §6/§14 and §15: removing a `Vec<Task<...>>` field and going generic
  over `AsRef<str>` both strip the anchor from call sites with untyped `&[]`
  literals, fixed with explicit `Task<Result<(), Error>>` and
  `&[] as &[String]` annotations.
- **Not covered by tests:** the deleted send paths (§2) and the
  `create_repository` fix (§9) need a live relay or a live GRASP server, so
  they were verified by compilation plus a line-by-line diff against the old
  control flow. A manual create-repository-then-open-detail-view pass is still
  the recommended pre-ship check for §9.

Verification: `cargo check --workspace`, `cargo clippy --workspace
--all-targets` and `cargo test --workspace` pass — 167 tests, 0 failures —
re-run after each item landed rather than once at the end. Test counts moved
between crates as functions moved (`signed_core` 41 → 48, `signed_git`
67 → 68, `workspace` 14 → 7); no coverage was lost.

Everything **not** listed above (per-repo/per-list `Entity` stores, the
`RefreshGate` debounce/coalesce pattern, `Nip34Tag`/`Coordinate`/`Filter`
usage in `signed_core`, the GRASP push-retry state machine in
`push_staged_to_grasps`, the `UniversalSigner` abstraction, and the dense
`.clone()` clusters audited in §16) was checked and already matches "use the
SDK directly, no unnecessary wrapper" — those were left alone.

# Pull request flow

How a pull request moves through Signed from creation to merge. A PR is a
kind-1618 root event whose content is the markdown description; its changes
live in a NIP-10-chained series of kind-1617 patch events (one per commit),
whose root the PR references via an `e` tag. Revisions publish new patch
events plus kind-1619 updates; statuses (kind 1630-1633) resolve the PR's
state.

## Whole lifecycle

```mermaid
graph TD
    A["New pull request panel"] --> B{"Compare source"}
    B -->|"Local checkout"| C["Pick folder (or auto-prefilled from remembered checkouts)"]
    B -->|"Announced fork"| D["Pick fork repo + branch"]
    D --> D1["Ensure base mirror (GitCache), fetch origin"]
    D1 --> D2["Import fork heads as refs/fork/&lt;owner&gt;/&lt;id&gt;/*"]
    C --> E["Defaults: target = announced HEAD, source = current branch / fork main"]
    E --> F["merge-base + commits + diff of target..source (Files/Commits tabs)"]
    F --> H["Submit: format-patch base..tip at publish time"]
    H --> I["split_patch_series: one part per commit"]
    I --> J{"Any part over 60 KB?"}
    J -->|"Yes"| K["Refuse with message"]
    J -->|"No"| L["tip = last part's From commit"]
    L --> M["Publish kind-1617 patch series: first has t root, later parts e-reply chained"]
    M --> N["Build kind-1618 PR event: c = tip, e = root patch, branch-name, merge-base, clone"]
    N --> O["Sign early - learn the event id"]
    O --> P["Push tip to refs/nostr/event-id: author /prs/ grasp servers first, then the announced servers"]
    P -->|"All rejected"| Q["last_warning banner in PR list"]
    P --> R["Publish kind-1618 PR event"]
    Q --> R
    R --> S{"Draft?"}
    S -->|"Yes"| T["Publish kind-1633 draft status"]
    S -->|"No"| U["PR open"]
    T --> U
    U --> V{"Author updates?"}
    V -->|"Yes"| W["Publish revision patch series: first has t root-revision and e-replies to the original root"]
    W --> X["Publish kind-1619 update: E/P NIP-22 tags, c = new tip"]
    X --> U
    V -->|"No"| Y{"Repository author merges?"}
    Y -->|"Yes"| Z["Apply the series with git am on the mirror clone"]
    Z --> AA["applied = rev-list previous-head..HEAD"]
    AA --> AB["Publish kind-1631 applied status: applied-as-commits plus r per commit, q plus e-reply per patch event"]
    AB --> AC["PR merged"]
    Y -->|"Close instead"| AD["Publish kind-1632 closed status"]
    AD --> AE["PR closed"]
```

Key points of the write side:

- **Compare sources** (NIP-34 / GRASP-06 native, no fork identity on the
  wire):
  - *Local checkout*: both branch selectors list a picked folder's
    branches; all git ops run in that folder. Checkouts of the target repo
    are remembered (folder pick + app clones) and matched implicitly
    (origin URL or EUC against the announcement), so the panel prefills the
    freshest one - no folder dialog for the common case.
  - *Announced fork*: the fork's heads are fetched into the target repo's
    GitCache mirror under `refs/fork/<owner-hex>/<id>/*` (private
    namespace; the browser never sees them). "Merge Into" lists the
    mirror's `refs/remotes/origin/*`, "Pull From" the imported fork
    branches, and every git op - merge-base, range diff/commits,
    format-patch, tip push - runs in the mirror, which holds both
    histories. Fork candidates are announcements related to the target by
    `u` tag or shared EUC, own forks first, without `clone` URLs excluded.
- **GRASP-06 hosting**: the tip is pushed under `refs/nostr/<event-id>`
  (nak's convention) to the *author's* grasp servers first -
  `https://<host>/prs/<author-npub>/<repo-id>.git`, resolved from the
  author's kind-10317 grasp list, falling back to the settings defaults -
  then to the base repository's announced grasp servers. The `clone` tag
  lists those `/prs/` URLs first, then the announced clone URLs (fixed
  before signing; dead URLs are inert, the patches stay the source of
  truth). Contributing therefore never depends on the other project's
  servers accepting a push.
- **Patch series**: each commit becomes its own kind-1617 event so no event
  grows past NIP-34's 60 KB guidance; the PR's `c` tag carries the *last*
  commit of the series (the tip), and each part carries its own
  `commit`/`r` tags.
- **Push before publish**: failure is non-fatal - the patch events remain
  the source of truth - and surfaces as a `last_warning` banner.
- **1619 updates are paste-only today** (no repo path holds the new tip's
  objects), so updates are not pushed; hosting them is deferred until the
  update dialog gains a local-checkout source.

## Creating a pull request - event ordering

```mermaid
sequenceDiagram
    participant User
    participant P as Base mirror (GitCache)
    participant F as Fork grasp server
    participant A as Author grasp (GRASP-06 /prs/)
    participant B as Base repo grasps
    participant R as Nostr relays

    User->>P: ensure mirror (fork mode) / pick local checkout
    P-->>F: fetch fork heads -> refs/fork/... (fork mode)
    User->>P: merge-base, range commits, range diff
    User->>P: submit: format-patch base..compare-ref
    loop each patch of the series
        User->>R: publish kind-1617 (first: t root, later: e reply)
    end
    User->>User: build and sign kind-1618 (clone = /prs/ URLs + announced)
    User->>A: push tip to refs/nostr/event-id (author servers, first)
    User->>B: push tip to refs/nostr/event-id (best-effort)
    A-->>User: accepted or rejected (all rejected -> warning)
    User->>R: publish kind-1618 PR event
    opt draft
        User->>R: publish kind-1633 draft status
    end
```

## Ready to contribute (suggestions)

Local checkouts are matched to announced repositories (remembered records
freshest-first ∪ scanned matches by origin URL or EUC). While a repository's
detail panel is open, each associated checkout is checked off the main
thread: current branch vs its base (announced HEAD, else `main`, else the
first branch), commits ahead, dirty worktrees excluded. A banner in the
repository panel then offers a prefilled New PR panel for the first branch
that is ahead with **no open PR by you** proposing it (`branch-name` tag,
falling back to the `c` tip tag) - NIP-34-native dedupe, refreshed
periodically and whenever the checkouts/announcements change. The panel
never submits anything on its own; suggestions only navigate and prefill.

## Updating and merging

```mermaid
sequenceDiagram
    participant Author
    participant Relays as Nostr relays
    participant Maintainer
    participant Clone as Mirror clone

    Note over Author,Relays: Update - PR author only (paste flow, no push yet)
    Author->>Relays: publish revision patch series (t root-revision, e reply to original root)
    Author->>Relays: publish kind-1619 update (E/P tags, c = new tip)

    Note over Maintainer,Clone: Merge - repository author only (store-only today)
    Maintainer->>Clone: git am the patch series
    Clone-->>Maintainer: applied commits (rev-list previous-head..HEAD)
    Maintainer->>Relays: publish kind-1631 applied status
    Note over Relays: applied-as-commits and r per commit, q and e-reply per applied patch event
```

## Reading side

```mermaid
graph TD
    A["PR root kind-1618"] --> B{"Newest status event by author or maintainer?"}
    B -->|"1633"| C["Draft"]
    B -->|"1631"| D["Applied / merged"]
    B -->|"1632"| E["Closed"]
    B -->|"1630 or none"| F["Open"]
    A --> G{"Newest kind-1619 update by PR author?"}
    G -->|"Yes"| H["tip = update's c tag"]
    G -->|"No"| I["tip = root's c tag"]
    A --> J{"Patch set present?"}
    J -->|"Yes"| K["Root patch via e tag, follow reply chain (newest wins per revision)"]
    J -->|"No"| L["Diff merge-base..tip from the git clone"]
```

Reader rules that keep the flow consistent:

- **Status**: only status events by the root author or a repository
  maintainer count; the newest wins, `Open` is the default.
- **Tip**: only kind-1619 updates by the PR author move the tip - a
  stranger's update is ignored.
- **Diff**: the patch set is preferred (NIP-34 `e`-linked chain); PRs from
  other clients without patch events fall back to diffing
  `merge-base..tip` in the local clone. Fetching tips from `clone` URLs
  (ngit `pr checkout` analog) is not implemented yet.

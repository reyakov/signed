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
    A["New pull request dialog"] --> B{"Patch source"}
    B -->|"Paste"| C["Paste git format-patch output"]
    B -->|"Local checkout"| D["Browse for checkout"]
    D --> E["Defaults: source = current branch, target = announced HEAD"]
    E --> F["Generate: merge-base plus format-patch base..tip"]
    F --> G["Apply check vs mirror clone - non-blocking warning"]
    C --> H["Submit"]
    F --> H
    G --> H
    H --> I["split_patch_series: one part per commit"]
    I --> J{"Any part over 60 KB?"}
    J -->|"Yes"| K["Refuse with message"]
    J -->|"No"| L["tip = last part's From commit"]
    L --> M["Publish kind-1617 patch series: first has t root, later parts e-reply chained"]
    M --> N["Build kind-1618 PR event: c = tip, e = root patch, branch-name, merge-base"]
    N --> O["Sign early - learn the event id"]
    O --> P["Push tip to refs/nostr/event-id on every announced grasp server"]
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

- **Merge base**: only computable in the local-checkout path
  (`signed_git::merge_base`); the paste path publishes none. The dialog
  reuses it at submit only while the patch textarea is unchanged.
- **Patch series**: each commit becomes its own kind-1617 event so no event
  grows past NIP-34's 60 KB guidance; the PR's `c` tag carries the *last*
  commit of the series (the tip), and each part carries its own
  `commit`/`r` tags.
- **Push before publish**: the tip is pushed to every announced grasp
  server under `refs/nostr/<event-id>` (nak's convention) so the announced
  `clone` URLs really can serve the commit. Failure is non-fatal — the
  patch events remain the source of truth — and surfaces as a
  `last_warning` banner.

## Creating a pull request - event ordering

```mermaid
sequenceDiagram
    participant User
    participant App
    participant Checkout as Local checkout
    participant Grasp as Grasp servers
    participant Relays as Nostr relays

    User->>App: pick checkout and branches, Generate
    App->>Checkout: merge-base(source, target)
    Checkout-->>App: base commit
    App->>Checkout: format-patch base..tip
    Checkout-->>App: patch series
    App->>App: split series, check per-part size
    loop each patch of the series
        App->>Relays: publish kind-1617 (first: t root, later: e reply)
    end
    App->>App: build and sign kind-1618 PR event
    App->>Grasp: push tip to refs/nostr/event-id
    Grasp-->>App: accepted or rejected (best-effort)
    App->>Relays: publish kind-1618 PR event
    opt draft
        App->>Relays: publish kind-1633 draft status
    end
```

## Updating and merging

```mermaid
sequenceDiagram
    participant Author
    participant Relays as Nostr relays
    participant Maintainer
    participant Clone as Mirror clone

    Note over Author,Relays: Update - PR author only
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
- **Tip**: only kind-1619 updates by the PR author move the tip — a
  stranger's update is ignored.
- **Diff**: the patch set is preferred (NIP-34 `e`-linked chain); PRs from
  other clients without patch events fall back to diffing
  `merge-base..tip` in the local clone.

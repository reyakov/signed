# TODO

## Fork support

- [x] Fork badge on repo list cards (`repo_list.rs::render_card`).
- [x] "Forked from …" text button in the repo detail header (`repo_detail/mod.rs::render_header`) and About dialog.
- [x] Clicking the upstream opens it as a center panel (shared `open_repo_panel` helper).

## Pull request improvement

### New pull request panel (replaces the dialog)

- [x] "New pull request" (PR list header + repo header `New PR`) opens a center panel instead of the paste dialog:
  - [x] Base/compare branch selectors fed from a user-chosen local checkout (GitHub-style; defaults: announced HEAD for base, checkout's current branch for compare).
  - [x] Files/Commits tabs like the repo panel: diff of `merge-base..compare` (shared `DiffPane` widget, also extracted for the commit diff panel) + virtual commit list with count badge; clicking a commit opens its diff panel.
  - [x] Only two inputs: title (required, gates the Create button) and description (optional).
  - [x] Patch is generated from the checkout at submit time (`format_patch_between` on the stored merge base); panel closes after publishing, errors surface in the PR list banner.
- [x] Removed with the dialog: paste textarea, draft checkbox, branch-name input and the mirror-clone apply-check hint (store behavior unchanged: `open_pull_request` still publishes the series + `branch-name`/`merge-base`/`r` tags and pushes the tip).

### Send patch panel (classic paste flow)

- [x] "Send patch" entry in the repo header PRs dropdown (`RepoAction::SendPatch`) and a "New pull request ▾ Send patch" dropdown replacing the PR list's plain new-PR button.
- [x] `send_patch.rs` center panel: title + optional description + `git format-patch` paste area; submits through `RepoStore::open_pull_request` (no checkout, no `branch-name`/`merge-base`). Synchronous store errors (malformed/oversized patch, sign-in) keep the panel open with an inline error; the panel closes once the publish is underway.

- [x] P1: `branch-name` tag + `r` EUC tag on PR creation; draft checkbox in the new-PR dialog (dialog since replaced by the panel above).
- [x] P1: `RepoStore::update_pull_request` (kind 1619 + root-revision patch) with an author-only "Update" button on the PR detail header.
- [x] P1: `latest_update` filters by PR author.
- [x] P2: local checkout picker in the new-PR dialog (folder picker + source/target branches + Generate): `signed_git::{merge_base, format_patch_between, patch_applies}`; `merge-base` tag now published; best-effort apply check shown under the patch field (superseded by the panel's live compare view).
- [x] P3: push tip to grasp servers under `refs/nostr/<event-id>` before publishing (from the local checkout); multi-commit series published as NIP-10-chained 1617 events with a 60 KB per-patch cap; PR list shows dismissible error/warning banners (incl. push failures).
- [x] P4: merge status tags — `merge_pull_request` publishes 1631 with `applied-as-commits` + `r` per applied commit and `q`/`e`-reply tags per applied patch event.

### Pull request follow-ups

- [ ] GRASP-06 `/prs/<npub>/<id>.git` contributor endpoints + kind-10317 user grasp-list fallback.
- [ ] Merge button in the PR detail view (`merge_pull_request` is store-only today), then fetch-and-merge (`merge-commit`) when the push backend is guaranteed.
- [ ] Local-checkout generation for the update-PR dialog (currently paste-only).
- [ ] Fork-aware compare in the New PR panel: today both branch selectors come from the user-picked local checkout, so a cross-fork PR (GitHub's "compare across forks") requires the fork's branch to exist locally. Add picking the fork repository from announced repos (its 30617 may point at this repo via the `u` tag, or share the EUC) + a branch, fetch it into the `GitCache` mirror, and run the `merge-base`/diff/`format-patch` flow against the base repo's mirror — like `choose_checkout` today but repo-driven.

## Performance: render path

- [ ] Virtualize issue/PR comment threads (`issue_detail.rs::render_comments`, `pull_request_detail.rs::render_comments`). Harder than the list tabs: comment cards have variable heights and live inside a scrolling page together with the body and the comment form, so this needs either measured item sizes or restructuring the whole discussion tab into one virtual list. (Comment bodies are already cached as `SharedString`, so re-renders are cheap element constructions, not byte copies.)

## Performance: relay/subscription behavior

- [ ] Narrow `RepoStore`'s `BackendEvent::NostrUpdate` relevance filter (`crates/signed_state/src/repo.rs:65-98`): any comment/status/label/deletion from anywhere wakes every open repo store; match only events referencing this repo's roots or coordinate.
- [ ] Reconsider `ban_relay_on_mismatch(true)` (`crates/signed_nostr/src/backend.rs:49`): combined with many short-lived auto-close subscriptions, a late event after EOSE can permanently ban a relay for the session.
- [ ] Relays added for a repo stay in the pool forever and grow unboundedly (`crates/signed_state/src/backend.rs`); consider removing repo relays when the last panel for that repo closes.

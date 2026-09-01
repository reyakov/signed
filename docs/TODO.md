# TODO

## Local repository scan

- [ ] Make the scanned directories configurable (currently fixed to Desktop and Documents).

## Create repository dialog

- [ ] Remember the folder picked in the create-repository dialog and default to it next time (currently defaults to Desktop).

## Performance: render path

- [ ] Virtualize issue/PR comment threads (`issue_detail.rs::render_comments`, `pull_request_detail.rs::render_comments`). Harder than the list tabs: comment cards have variable heights and live inside a scrolling page together with the body and the comment form, so this needs either measured item sizes or restructuring the whole discussion tab into one virtual list. (Comment bodies are already cached as `SharedString`, so re-renders are cheap element constructions, not byte copies.)

## Performance: relay/subscription behavior

- [ ] Narrow `RepoStore`'s `BackendEvent::NostrUpdate` relevance filter (`crates/signed_state/src/repo.rs:65-98`): any comment/status/label/deletion from anywhere wakes every open repo store; match only events referencing this repo's roots or coordinate.
- [ ] Reconsider `ban_relay_on_mismatch(true)` (`crates/signed_nostr/src/backend.rs:49`): combined with many short-lived auto-close subscriptions, a late event after EOSE can permanently ban a relay for the session.
- [ ] Relays added for a repo stay in the pool forever and grow unboundedly (`crates/signed_state/src/backend.rs`); consider removing repo relays when the last panel for that repo closes.

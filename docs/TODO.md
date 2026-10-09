# TODO

## UI

- [ ] Add profile panel, replace the empty placeholder behind the sidebar user menu's "View profile".
  - [ ] Show the user's profile (metadata, avatar).
  - [ ] List their published repositories.
  - [ ] List their relays.
  - [ ] Show their recent activity (issues, PRs, patches, comments).
- [ ] Add relay panel, replace the empty placeholder behind the sidebar user menu's "View relays".
  - [ ] Configure lookup relays.
  - [ ] Configure outbox and inbox relays.
  - [ ] Configure bootstrap relays.
  - [ ] Configure grasp servers.

## Maintainer workflow

- [ ] Add close issue / close PR / mark applied-merged actions, `RepoStore::set_status` exists but is only called for drafts.
- [ ] Carry `applied-as-commits` / `merge-commit` tags on status events (nak publishes these).
- [ ] Wire up `Repo::apply_patch` (`git am`) so patches can be applied in-app.

## Repository management

- [ ] Set `maintainers` when announcing a new repo, multi-maintainer repos cannot be created today and maintainer support is read-only.
- [ ] Re-publish announcements to relays that missed the latest version (nak `git sync` reconciles this).
- [ ] Support creating fork announcements (upstream `u` tag), currently parse-only.
- [ ] Publish `web` urls and hashtags; currently parse-only.

## Push

- [ ] Add a fast-forward check and a force option (nak refuses non-FF without `--force`), today it relies entirely on grasp CAS.
- [ ] Allow NIP-34 maintainers to push, not just the owner.

## Contributor workflow

- [ ] Check PRs out into a worktree to build/test, diffs are view-only in the mirror (ngit `pr checkout`, `pr/` branches).

## Identity

- [ ] Support multiple identities with non-destructive switching (ngit: multi-account keyring, aliases, `--signer`); today sign-out deletes the single key.

## Ecosystem

- [ ] Private repository support (GRASP-08, encrypted private relay lists).
- [ ] Git remote helper so plain `git clone/push` with `nostr://` urls works outside the app.

## Minor

- [ ] Open a repo by address (naddr, `npub/identifier`, NIP-05), discovery is currently Explore/Sidebar only.
- [ ] Per-grasp-server announcement freshness view (nak `git status`).
- [ ] Publish PR labels, they are always empty today.

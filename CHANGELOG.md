# Changelog

## Unreleased

### Breaking changes

### Added

- Add an identity import dialog, importing an existing identity by pasting its `nsec` secret key (encrypted with a new passphrase), NIP-49 `ncryptsec` (unlocked with its passphrase), or `bunker://` NIP-46 URI

### Changed

- Increase the default theme corner radii from 2 to 4 and large radii from 6 to 8
- Surface backend errors as error notifications in the workspace instead of silently dropping them
- Break ties in repository activity lists by event id, so same-second events order deterministically
- Restructure the backend around domain types: git operations behind a `Repo` type, the grasp push pipeline behind `GraspPush`, nostr connectivity behind `NostrBackend`, and shared helpers consolidated into `utils`
- Fetch the logged-in user's grasp list, code follows, followed repositories, contacts, profile metadata, mute list, and blossom servers via gossip at login instead of the bootstrap relays
- Connect to grasp relays and load the inbox only after the user's grasp list event arrives

### Fixed

### Removed

- Remove dead code: the unused `login`/`logout` family, the unwired `SyncProgress` pipeline, `merge_pull_request`, inbox mark-read/archive APIs and the wasm32-only code paths

### Deprecated

## v0.2.0-alpha - 2026/09/27

### Added

- Show an avatar in each panel's tab, using the repository owner's profile picture when set and a pixel avatar otherwise
- Add a Tab Bar setting to hide the previous/next tab buttons, hidden by default
- Add an Event Fetching Strategy setting, fetching a repository's activity from its announced relays only (Curated) or from every maintainer's relays as well (Uncensored, the default)

### Changed

- Migrate the GPUI foundation to the published `gpui-pre` crates and GPUI Kit 0.6, off the zed and gpui-component git pins
- Use the pixel avatar as the single fallback for a missing picture, sized and rounded to match the other avatars
- Redesign the dock tab bar, using muted grey active tab, added close buttons, double-click to zoom, and removed panel toolbar
- Connect to bootstrap relays on demand instead of at startup
- Fetch profile metadata in batches of 100 authors, applying every requested profile in a single database query
- Prefetch the 500 most recent profiles at startup instead of 200

### Fixed

- Render every avatar at one consistent size, where a surrounding border had shrunk pictures by two pixels and the pixel avatar ignored an explicit size
- Date repositories from their repository state event, so the explore list and open repository views show the latest push instead of the announcement date
- Fix background tasks outliving their view, so a closed repository panel or pull request view stops fetching and publishing on its own

### Removed

- Remove cover note support, the kind-1624 GitWorkshop and `ngit` extension outside the NIP-34

## v0.1.0-alpha - 2026/09/14

Initial alpha release.

### Added

- Create and unlock a passphrase-protected Nostr identity
- Browse and search NIP-34 repositories with All, Popular and Recent filters
- Scan local Git repositories and detect their NIP-34 and GRASP bindings
- Create, publish, clone and open repositories
- Repository view with files, commit history, branches and tags
- Colour-coded commit diffs
- Issues and pull requests with discussions
- Send patches and push branches
- Inbox for repository activity
- Settings for appearance, theme, GRASP servers and repository scan paths
- Release and packaging infrastructure

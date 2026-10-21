# Changelog

## Unreleased

### Breaking changes

### Added

- Follow the operating system's native theme on macOS, Windows and Linux (KDE, GNOME), applying the OS colors, fonts, radii and accessibility preferences.
- Add an identity import dialog, importing an existing identity by pasting its `nsec` secret key (encrypted with a new passphrase), NIP-49 `ncryptsec` (unlocked with its passphrase), or `bunker://` NIP-46 URI
- Add a dropdown menu on the signed-in user button in the sidebar
- Add sign-out feature, confirmed by an alert dialog before deleting the stored session credential
- Add a quit action, bound to `secondary-q` (cmd on macOS, ctrl on others)
- Allow choosing the default branch when publishing a repository
- Detect when local commits are not on Nostr yet, showing an unsynced count badge on sidebar repository rows and a Sync now banner on the repository view
- Add a step-by-step push checklist while publishing and syncing a repository
- Add a `Reset remote to local` action that republishes the mirror and deletes grasp refs the mirror does not have

### Changed

- Use the built-in light/dark syntax highlighting palette instead of the Signed palette bundled in `signed.json` when the native theme is active
- Increase the default theme corner radii from 2 to 4 and large radii from 6 to 8
- Surface backend errors as error notifications in the workspace instead of silently dropping them
- Break ties in repository activity lists by event id, so same-second events order deterministically
- Restructure the backend around domain types: git operations behind a `Repo` type, the grasp push pipeline behind `GraspPush`, nostr connectivity behind `NostrBackend`, and shared helpers consolidated into `utils`
- Fetch the logged-in user's grasp list, code follows, followed repositories, contacts, profile metadata, mute list, and blossom servers via gossip at login instead of the bootstrap relays
- Connect to grasp relays and load the inbox only after the user's grasp list event arrives
- Refresh the checkouts store only when the checkouts settings change
- Restructure the core crate into focused modules for announcements, pull requests, git events, inbox, and upstream state
- Publish nostr events directly through the client with relay acceptance required, removing the grasp push helper and its per-server retry loop so failures surface immediately
- Poll the repository sync drift check every 180 seconds instead of 60
- Track sync status per view instead of globally
- Merge the `PixelAvatar` into the `Avatar` component. Seed user pixel avatars from the public key instead of the display name, so avatars stay stable when a user renames

### Fixed

- Fix the repository view refreshing its mirror from the network while a push is in progress, so the refresh no longer races the push
- Fix opening a repository failing with a `destination already exists` error when two loads raced to clone its mirror, clones are now serialized and later loads reuse the finished clone
- Fix a repository never opening again after an interrupted clone left an unusable mirror directory behind, which is now removed and re-cloned
- Fix signing in with a `bunker://` URI silently generating a new identity instead of erroring when its master key cannot be parsed
- Fix repository sync failing with `state event in purgatory ... doesn't match push` when grasp servers hold branches or tags the local repository lacks
- Fix sync failing with `could not fetch refs/tags/...^{}` on repositories with annotated tags, where the peel entries `git ls-remote` reports for annotated tags were mistaken for missing refs

### Removed

- Remove the light/dark theme pickers and the font size, corner radius, focus ring and shadow settings from the settings dialog; colors and geometry now come from the native theme
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

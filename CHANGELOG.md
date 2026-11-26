# Changelog

## Unreleased

### Added

- Add a confirmation dialog before deleting a repository
- Browse the files of a commit as of that commit from the Commits list, the short id on a row shows the commit's files in place of the worktree and the branch selector shows the commit until a branch or tag is picked again

### Changed

- Opening a repository no longer runs a forced checkout of its worktree
- Deleting a repository now waits until its deletion events are accepted by a relay
- Keep the Explore list at its scroll position across background refreshes, a new search or filter still starts at the top
- Reuse the rendered file tree, commit diffs, README previews and split pane instead of rebuilding them on every repaint
- Truncate long tab and file tree labels with an ellipsis instead of letting them widen their row
- Open the New issue form in a split panel instead of a dialog, it now stays open until the issue is published and shows a failed publish in the form
- Open issue and pull request details in a new tab instead of the split panel
- Show the panel toolbar on the New issue, New pull request and Send patch split panels
- Show each commit of the Commits list on one row with its author avatar and name, short id and summary

### Fixed

- Fix the status badge on issue and pull request rows sitting below the title instead of centered with it
- Fix opening a local repository or switching its branch discarding uncommitted changes, a checkout now refuses to overwrite uncommitted edits or untracked files and leaves the branch untouched when it fails
- Fix viewing a commit diff crashing when the repository configures an external diff driver
- Fix an unreadable `settings.json` being replaced by defaults on the next settings change, the file is now left untouched and the error is shown in Settings
- Fix the app panicking at startup when the nostr database or a platform directory cannot be opened, a damaged database now falls back to a temporary cache and startup failures quit cleanly
- Fix a pending session restore replacing an identity that was just imported, or signing back in after signing out
- Fix the New pull request panel closing as if the pull request was published when the patch or the pull request event failed to publish
- Fix push and clone actions staying blocked with "already in progress" after their view closed mid-operation
- Fix the reset remote dialog reporting no deleted refs while a grasp server was unreachable, the reset is now blocked until every server can be listed
- Fix exported commit patches turning a file without a trailing newline into one with a newline
- Fix each patch event of a multi-commit series repeating every later commit's patch, and series with CRLF line endings being split at the wrong offsets
- Fix mirror cache directories colliding for repository identifiers that differ only in characters removed by sanitization
- Fix profiles staying stuck after a failed database query, and a repository view showing "Cloning repository" forever when no announcement is found
- Fix commit patch downloads failing silently and dismissed pull request alerts staying on screen
- Fix busy dialogs being dismissed by Escape or a backdrop click while their task was still running
- Fix text typed into a field under a panel appearing up to a quarter second late
- Fix an open dialog taking keyboard focus back from the command palette or a toast button on every repaint
- Fix the remove button of a Settings or grasp server row carrying its hover state over to the next row after a removal
- Fix the file tree's scrollbar sitting inset from the sidebar's right border, it now rides beside it
- Fix the selected filter in the Explore, Issues and Pull requests views highlighting only after the pointer crossed it, it now highlights as soon as the filter is drawn
- Fix the Commits list and its count badge lagging behind the header after a repository opened or after a Sync, they now follow the mirror once it caught up with the servers
- Fix the Sync buttons, the repository menu and the Code button staying disabled or loading after a sync, republish, reset or download finished

## v0.2.0 - 2026/10/21

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

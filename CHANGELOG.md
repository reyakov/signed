# Changelog

## Unreleased

### Breaking changes

### Added

- Show an avatar in each panel's tab, using the repository owner's profile picture when set and a pixel avatar otherwise

### Changed

- Migrate the GPUI foundation to the published `gpui-pre` crates and GPUI Kit 0.6, off the zed and gpui-component git pins
- Use the pixel avatar as the single fallback for a missing picture, sized and rounded to match the other avatars
- Redesign the dock tab bar, using muted grey active tab, added close buttons, double-click to zoom, and removed panel toolbar

### Fixed

- Render every avatar at one consistent size, where a surrounding border had shrunk pictures by two pixels and the pixel avatar ignored an explicit size

### Removed

### Deprecated

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

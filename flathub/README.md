# Flathub Submission for Signed

This directory contains the files needed to submit Signed to Flathub.

## Prerequisites

- Flatpak installed
- `flatpak-builder` installed
- Rust/Cargo installed (for vendoring)

## Quick Start

Run the preparation script from the repo root:

```bash
./script/prepare-flathub
```

This will:
1. Vendor all Rust dependencies (crates.io + git)
2. Generate the metainfo.xml with proper release info
3. Create `su.reya.signed.yml` - the Flatpak manifest for Flathub

## Files Generated

| File | Purpose |
|------|---------|
| `su.reya.signed.yml` | Main Flatpak manifest (submit this to Flathub) |
| `su.reya.signed.metainfo.xml` | AppStream metadata with release info |
| `vendor.tar.gz` | Vendored Rust dependencies |
| `cargo-config.toml` | Cargo configuration for offline builds |
| `release-info.xml` | Release info snippet for metainfo |

## Testing Locally

Before submitting to Flathub, test the build:

```bash
cd flathub

# Build and install locally
flatpak-builder --user --install --force-clean build su.reya.signed.yml

# Test the app
flatpak run su.reya.signed

# Run the Flathub linter (must pass!)
flatpak run --command=flatpak-builder-lint org.flatpak.Builder manifest su.reya.signed.yml
flatpak run --command=flatpak-builder-lint org.flatpak.Builder repo repo
```

## Submitting to Flathub

### 1. Prepare Your Release

Ensure you have:
- [ ] Committed all changes
- [ ] Tagged the release: `git tag -a v0.1.0 -m "Release v0.1.0"`
- [ ] Pushed the tag: `git push origin v0.1.0`
- [ ] Run `./script/prepare-flathub` to regenerate files

### 2. Fork and Submit

```bash
# Fork https://github.com/flathub/flathub on GitHub first

# Clone your fork (use the new-pr branch!)
git clone --branch=new-pr git@github.com:YOUR_USERNAME/flathub.git
cd flathub

# Create a new branch
git checkout -b su.reya.signed

# Copy ONLY the manifest file from your project
cp /path/to/signed/flathub/su.reya.signed.yml .

# Commit and push
git add su.reya.signed.yml
git commit -m "Add su.reya.signed"
git push origin su.reya.signed
```

### 3. Open Pull Request

1. Go to your fork on GitHub
2. Click "Compare & pull request"
3. **Important:** Set base branch to `new-pr` (not `master`!)
4. Fill in the PR template
5. Submit and wait for review

## What Happens Next?

1. Flathub's automated CI will build your app
2. A maintainer will review your submission
3. Once approved, a new repo `flathub/su.reya.signed` will be created
4. You'll get write access to maintain the app
5. Future updates: Push new commits to `flathub/su.reya.signed`

## Updating the App

To release a new version:

1. Update version in workspace `Cargo.toml`
2. Tag the new release: `git tag -a v0.1.0 -m "Release v0.1.0"`
3. Push the tag: `git push origin v0.1.0`
4. Run `./script/prepare-flathub` to regenerate
5. Clone the flathub repo: `git clone https://github.com/flathub/su.reya.signed.git`
6. Update the manifest with new commit/tag and hashes
7. Submit PR to `flathub/su.reya.signed`

## Troubleshooting

### Build fails with "network access not allowed"
- Make sure `CARGO_NET_OFFLINE=true` is set in the manifest
- Ensure `vendor.tar.gz` is properly extracted before building

### Linter complains about metainfo
- Ensure `su.reya.signed.metainfo.xml` has at least one `<release>` entry
- Add accessible screenshot URLs if required

### Missing dependencies
- If new git dependencies are added, re-run `script/prepare-flathub`
- The script vendors all dependencies from `Cargo.lock`

## Resources

- [Flathub Submission Docs](https://docs.flathub.org/docs/for-app-authors/submission)
- [Flatpak Manifest Reference](https://docs.flatpak.org/en/latest/manifests.html)
- [AppStream Metainfo Guide](https://www.freedesktop.org/software/appstream/docs/chap-Metadata.html)

use nostr::prelude::*;

/// Address of a NIP-34 repository announcement: `30617:<owner-pubkey>:<repo-id>`.
///
/// The Rust Nostr SDK's [`Coordinate`] already provides parsing, formatting
/// and hashing for this; the alias keeps the repository-specific vocabulary
/// while reusing the SDK type.
pub type RepoAddr = Coordinate;

/// Build the address of a NIP-34 repository announcement.
pub fn repo_addr(owner: PublicKey, id: impl Into<String>) -> RepoAddr {
    Coordinate::new(Kind::GitRepoAnnouncement, owner).identifier(id)
}

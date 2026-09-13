use nostr::prelude::*;

/// Address of a NIP-34 repository announcement, `30617:<owner-pubkey>:<repo-id>`.
///
/// The Rust Nostr SDK's [`Coordinate`] parses, formats and hashes this,
/// the alias reuses the SDK type while keeping repository-specific vocabulary.
pub type RepoAddr = Coordinate;

pub fn repo_addr(owner: PublicKey, id: impl Into<String>) -> RepoAddr {
    Coordinate::new(Kind::GitRepoAnnouncement, owner).identifier(id)
}

pub fn identifier_from_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '/' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

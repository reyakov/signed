use nostr::prelude::*;

/// Address of a NIP-34 repository announcement, `30617:<owner-pubkey>:<repo-id>`.
///
/// The Rust Nostr SDK's [`Coordinate`] parses, formats and hashes this,
/// the alias reuses the SDK type while keeping repository-specific vocabulary.
pub type RepoAddr = Coordinate;

/// Build the address of a NIP-34 repository announcement.
pub fn repo_addr(owner: PublicKey, id: impl Into<String>) -> RepoAddr {
    Coordinate::new(Kind::GitRepoAnnouncement, owner).identifier(id)
}

/// Derive a repository identifier from a display name
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifier_from_name_slugs_like_gitworkshop() {
        assert_eq!(identifier_from_name("My Repo"), "My-Repo");
        assert_eq!(identifier_from_name("my-repo"), "my-repo");
        assert_eq!(identifier_from_name("Foo_Bar!"), "Foo-Bar-");
        assert_eq!(identifier_from_name("a/b"), "a/b");
        assert_eq!(identifier_from_name("Café"), "Caf-");
    }
}

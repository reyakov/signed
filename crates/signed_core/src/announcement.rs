use std::collections::HashSet;

use nostr::prelude::*;

use crate::RepoAddr;
use crate::upstream::Upstream;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Announcement {
    pub event_id: EventId,
    pub id: String,
    pub owner: PublicKey,
    pub created_at: Timestamp,
    pub name: Option<String>,
    pub description: Option<String>,
    pub web: Vec<Url>,
    pub clone: Vec<Url>,
    pub relays: Vec<RelayUrl>,
    pub euc: Option<String>,
    pub maintainers: Vec<PublicKey>,
    pub upstream: Option<Upstream>,
    pub hashtags: Vec<String>,
}

impl Announcement {
    /// Lists forks of the base, the user's own first.
    pub fn forks_in<'a>(
        announcements: &'a [Announcement],
        base: &RepoAddr,
        base_euc: Option<&str>,
        user: Option<PublicKey>,
    ) -> Vec<&'a Announcement> {
        let (mut own, mut others) = (Vec::new(), Vec::new());
        for announcement in announcements {
            if announcement.clone.is_empty() || !announcement.is_fork_of(base, base_euc) {
                continue;
            }
            if Some(announcement.owner) == user {
                own.push(announcement);
            } else {
                others.push(announcement);
            }
        }
        own.into_iter().chain(others).collect()
    }

    /// Parses a kind 30617 event, rejecting other kinds and missing `d` tags.
    pub fn from_event(event: &Event) -> Option<Self> {
        if event.kind != Kind::GitRepoAnnouncement {
            return None;
        }

        let id = event.tags.identifier()?;

        let mut hashtags: Vec<String> = Vec::new();
        hashtags.extend(event.tags.hashtags().map(|t| t.to_string()));

        let mut name: Option<String> = None;
        let mut description: Option<String> = None;
        let mut web: Vec<Url> = Vec::new();
        let mut clone: Vec<Url> = Vec::new();
        let mut relays: Vec<RelayUrl> = Vec::new();
        let mut euc: Option<String> = None;
        let mut maintainers: Vec<PublicKey> = Vec::new();
        let mut upstream: Option<Upstream> = None;

        for tag in event.tags.iter() {
            match Nip34Tag::parse(tag.as_slice()) {
                Ok(Nip34Tag::Name(value)) => name = Some(value),
                Ok(Nip34Tag::Description(value)) => description = Some(value),
                Ok(Nip34Tag::Web(urls)) => web.extend(urls),
                Ok(Nip34Tag::Clone(urls)) => clone.extend(urls),
                Ok(Nip34Tag::Relays(urls)) => relays.extend(urls),
                Ok(Nip34Tag::EarliestUniqueCommitId(commit)) => euc = Some(commit.to_string()),
                Ok(Nip34Tag::Maintainers(keys)) => maintainers.extend(keys),
                _ => {}
            }

            if upstream.is_none() && tag.kind() == "u" {
                let values = tag.as_slice();
                let raw = values.get(1).map(String::as_str).unwrap_or_default();
                if !raw.is_empty() {
                    upstream = Some(Upstream::parse(raw));
                }
            }
        }

        Some(Self {
            event_id: event.id,
            owner: event.pubkey,
            created_at: event.created_at,
            id,
            name,
            description,
            web,
            clone,
            relays,
            euc,
            maintainers,
            upstream,
            hashtags,
        })
    }

    /// Returns the owner and identifier as a repository address.
    pub fn addr(&self) -> RepoAddr {
        RepoAddr::new(self.owner, self.id.clone())
    }

    /// Returns the name, defaulting to "Untitled".
    pub fn name(&self) -> String {
        self.name.clone().unwrap_or("Untitled".into())
    }

    /// Matches a `u` tag pointing at the base or a shared earliest unique commit.
    pub fn is_fork_of(&self, base: &RepoAddr, base_euc: Option<&str>) -> bool {
        if self.addr() == *base {
            return false;
        }
        if self.upstream.as_ref().and_then(|u| u.addr.as_ref()) == Some(base) {
            return true;
        }
        base_euc.is_some_and(|euc| self.euc.as_deref() == Some(euc))
    }

    /// Returns the description, defaulting to "No description".
    pub fn description(&self) -> String {
        self.description
            .clone()
            .unwrap_or("No description".to_string())
    }

    /// Adds the owner to the maintainers unless a `u` tag marks this a subordinate fork.
    pub fn effective_maintainers(&self) -> Vec<PublicKey> {
        let mut maintainers = self.maintainers.clone();
        if self.upstream.is_none() && !maintainers.contains(&self.owner) {
            maintainers.push(self.owner);
        }
        maintainers
    }

    /// Returns deduplicated `git clone` commands for the advertised URLs.
    pub fn clone_urls(&self) -> Vec<String> {
        let mut seen = HashSet::new();
        self.clone
            .iter()
            .map(|url| format!("git clone {url}"))
            .filter(|command| seen.insert(command.clone()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAINTAINER_HEX: &str = "68d81165918100b7da43fc28f7d1fc12554466e1115886b9e7bb326f65ec4272";

    /// Returns the test signer keys.
    fn keys() -> Keys {
        Keys::new(
            SecretKey::from_hex("0000000000000000000000000000000000000000000000000000000000000001")
                .expect("valid secret key"),
        )
    }

    /// Builds a signed repo announcement event from raw tags.
    fn announcement_event(tags: &[&[&str]]) -> Event {
        let tags: Vec<Tag> = tags
            .iter()
            .map(|t| Tag::parse(t.to_vec()).expect("valid tag"))
            .collect();

        EventBuilder::new(Kind::GitRepoAnnouncement, "")
            .tags(tags)
            .finalize(&keys())
            .expect("signed event")
    }

    /// Parses an event carrying every supported tag.
    #[test]
    fn parses_full_announcement() {
        let event = announcement_event(&[
            &["d", "my-repo"],
            &["name", "My Repo"],
            &["description", "A test repository"],
            &["web", "https://example.com/repo"],
            &["clone", "https://example.com/repo.git"],
            &["relays", "wss://relay.example.com"],
            &["r", "aa231c4c6a5777dc89b42207b499891a344add5c", "euc"],
            &["maintainers", MAINTAINER_HEX],
            &["t", "rust"],
            &["t", "nostr"],
        ]);

        let announcement = Announcement::from_event(&event).expect("parses");

        assert_eq!(announcement.owner, keys().public_key());
        assert_eq!(announcement.id, "my-repo");
        assert_eq!(announcement.name.as_deref(), Some("My Repo"));
        assert_eq!(
            announcement.description.as_deref(),
            Some("A test repository")
        );
        assert_eq!(
            announcement.web,
            vec![Url::parse("https://example.com/repo").unwrap()]
        );
        assert_eq!(
            announcement.clone,
            vec![Url::parse("https://example.com/repo.git").unwrap()]
        );
        assert_eq!(
            announcement.relays,
            vec![RelayUrl::parse("wss://relay.example.com").unwrap()]
        );
        assert_eq!(
            announcement.euc.as_deref(),
            Some("aa231c4c6a5777dc89b42207b499891a344add5c")
        );
        assert_eq!(
            announcement.maintainers,
            vec![PublicKey::from_hex(MAINTAINER_HEX).expect("valid pubkey")]
        );
        assert_eq!(announcement.hashtags, vec!["rust", "nostr"]);
    }

    /// Skips malformed clone, relay, and maintainer values.
    #[test]
    fn drops_malformed_values() {
        let event = announcement_event(&[
            &["d", "my-repo"],
            &["clone", "not a url"],
            &["relays", "wss://good.example.com"],
            &["maintainers", "not-a-pubkey"],
        ]);

        let announcement = Announcement::from_event(&event).expect("parses");

        assert!(announcement.clone.is_empty());
        assert_eq!(
            announcement.relays,
            vec![RelayUrl::parse("wss://good.example.com").unwrap()]
        );
        assert!(announcement.maintainers.is_empty());
    }

    /// Parses the `u` tag into an upstream coordinate.
    #[test]
    fn parses_upstream_tag() {
        let event = announcement_event(&[
            &["d", "my-fork"],
            &[
                "u",
                "30617:68d81165918100b7da43fc28f7d1fc12554466e1115886b9e7bb326f65ec4272:upstream|https://example.com/upstream.git",
                "wss://relay.example.com",
            ],
        ]);

        let announcement = Announcement::from_event(&event).expect("parses");
        let upstream = announcement.upstream.expect("parses the u tag");

        assert_eq!(
            upstream.addr,
            Some(RepoAddr::new(
                PublicKey::from_hex(MAINTAINER_HEX).expect("valid pubkey"),
                "upstream"
            ))
        );
        assert_eq!(
            upstream.raw,
            "30617:68d81165918100b7da43fc28f7d1fc12554466e1115886b9e7bb326f65ec4272:upstream|https://example.com/upstream.git"
        );
        assert_eq!(
            upstream.display().to_string(),
            "30617:68d81165918100b7da43fc28f7d1fc12554466e1115886b9e7bb326f65ec4272:upstream"
        );
    }

    /// Matches a fork through its `u` tag coordinate.
    #[test]
    fn is_fork_of_matches_the_u_tag_coordinate() {
        let base = RepoAddr::new(
            PublicKey::from_hex(MAINTAINER_HEX).expect("valid pubkey"),
            "upstream",
        );
        let event = announcement_event(&[&["d", "my-fork"], &["u", &base.to_string()]]);
        let fork = Announcement::from_event(&event).expect("parses");

        assert!(fork.is_fork_of(&base, None));
    }

    /// Matches forks sharing the base's earliest unique commit.
    #[test]
    fn is_fork_of_matches_a_shared_euc() {
        let euc = "aa231c4c6a5777dc89b42207b499891a344add5c";
        let base_event = announcement_event(&[&["d", "upstream"], &["r", euc, "euc"]]);
        let base = Announcement::from_event(&base_event).expect("parses");
        let base_addr = base.addr();

        let fork_event = announcement_event(&[&["d", "mirror"], &["r", euc, "euc"]]);
        let fork = Announcement::from_event(&fork_event).expect("parses");
        assert!(fork.is_fork_of(&base_addr, base.euc.as_deref()));

        let other_event = announcement_event(&[
            &["d", "other"],
            &["r", "bb231c4c6a5777dc89b42207b499891a344add5c", "euc"],
        ]);
        let other = Announcement::from_event(&other_event).expect("parses");
        assert!(!other.is_fork_of(&base_addr, base.euc.as_deref()));

        assert!(!fork.is_fork_of(&base_addr, None));
    }

    /// Matches permanent forks whose `u` tag points at the base despite a diverged EUC.
    #[test]
    fn is_fork_of_matches_permanent_forks_with_a_diverged_euc() {
        let base = RepoAddr::new(
            PublicKey::from_hex(MAINTAINER_HEX).expect("valid pubkey"),
            "upstream",
        );
        let base_euc = "aa231c4c6a5777dc89b42207b499891a344add5c";
        let event = announcement_event(&[
            &["d", "my-fork"],
            &["u", &base.to_string()],
            &["r", "cc231c4c6a5777dc89b42207b499891a344add5c", "euc"],
        ]);
        let fork = Announcement::from_event(&event).expect("parses");

        assert!(fork.is_fork_of(&base, Some(base_euc)));
    }

    /// Includes the owner among maintainers for primary repositories.
    #[test]
    fn effective_maintainers_include_owner_for_primary_repos() {
        let event = announcement_event(&[&["d", "my-repo"], &["maintainers", MAINTAINER_HEX]]);

        let announcement = Announcement::from_event(&event).expect("parses");
        let maintainers = announcement.effective_maintainers();

        assert_eq!(maintainers.len(), 2);
        assert!(maintainers.contains(&announcement.owner));
        assert!(maintainers.contains(&PublicKey::from_hex(MAINTAINER_HEX).expect("valid pubkey")));
    }

    /// Excludes the owner from maintainers for subordinate forks.
    #[test]
    fn effective_maintainers_exclude_owner_for_subordinate_forks() {
        let event = announcement_event(&[
            &["d", "my-fork"],
            &["u", "30617:abc:upstream|https://example.com/upstream.git"],
            &["maintainers", MAINTAINER_HEX],
        ]);

        let announcement = Announcement::from_event(&event).expect("parses");
        let maintainers = announcement.effective_maintainers();

        assert!(!maintainers.contains(&announcement.owner));
        assert_eq!(
            maintainers,
            vec![PublicKey::from_hex(MAINTAINER_HEX).expect("valid pubkey")]
        );
    }

    const OWNER_KEYS: [&str; 3] = [
        "0000000000000000000000000000000000000000000000000000000000000001",
        "0000000000000000000000000000000000000000000000000000000000000002",
        "0000000000000000000000000000000000000000000000000000000000000003",
    ];

    /// Builds an announcement event signed by the given owner key.
    fn owned_announcement_event(owner: &str, tags: &[&[&str]]) -> Event {
        let keys = Keys::new(SecretKey::from_hex(owner).expect("valid secret key"));
        let tags: Vec<Tag> = tags
            .iter()
            .map(|t| Tag::parse(t.to_vec()).expect("valid tag"))
            .collect();
        EventBuilder::new(Kind::GitRepoAnnouncement, "")
            .tags(tags)
            .finalize(&keys)
            .expect("signed event")
    }

    /// Builds one announcement owned by the owner key at the given index.
    fn owned_announcements(owner_ix: usize, tags: &[&[&str]]) -> Vec<Announcement> {
        vec![
            Announcement::from_event(&owned_announcement_event(OWNER_KEYS[owner_ix], tags))
                .expect("parses"),
        ]
    }

    /// Places the user's own forks before everyone else's.
    #[test]
    fn fork_candidates_orders_own_forks_first() {
        let euc = "aa231c4c6a5777dc89b42207b499891a344add5c";
        let clone = "https://grasp.example/npub1x/my-fork.git";

        let base_addr = RepoAddr::new(
            PublicKey::from_hex(OWNER_KEYS[0]).expect("pubkey"),
            "upstream",
        );
        let all = vec![
            owned_announcements(
                2,
                &[
                    &["d", "other-project"],
                    &["r", "bb231c4c6a5777dc89b42207b499891a344add5c", "euc"],
                ],
            )
            .pop()
            .unwrap(),
            owned_announcements(
                1,
                &[&["d", "my-fork"], &["r", euc, "euc"], &["clone", clone]],
            )
            .pop()
            .unwrap(),
            owned_announcements(
                2,
                &[
                    &["d", "their-fork"],
                    &["u", &base_addr.to_string()],
                    &["clone", clone],
                ],
            )
            .pop()
            .unwrap(),
        ];

        let user = PublicKey::from_hex(OWNER_KEYS[1]).expect("pubkey");
        let forks = Announcement::forks_in(&all, &base_addr, Some(euc), Some(user));

        let ids: Vec<&str> = forks.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, vec!["my-fork", "their-fork"]);
    }

    /// Excludes the base itself, unrelated repos, and repos without clone URLs.
    #[test]
    fn fork_candidates_excludes_base_unrelated_and_unfetchable() {
        let euc = "aa231c4c6a5777dc89b42207b499891a344add5c";
        let base_owner = PublicKey::from_hex(OWNER_KEYS[0]).expect("pubkey");
        let base_addr = RepoAddr::new(base_owner, "upstream");

        let mut all = vec![
            owned_announcements(0, &[&["d", "upstream"], &["r", euc, "euc"]])
                .pop()
                .unwrap(),
            owned_announcements(1, &[&["d", "no-clone-fork"], &["r", euc, "euc"]])
                .pop()
                .unwrap(),
            owned_announcements(
                2,
                &[
                    &["d", "other"],
                    &["r", "cc231c4c6a5777dc89b42207b499891a344add5c", "euc"],
                ],
            )
            .pop()
            .unwrap(),
            owned_announcements(
                2,
                &[
                    &["d", "mirror"],
                    &["r", euc, "euc"],
                    &["clone", "https://grasp.example/x/mirror.git"],
                ],
            )
            .pop()
            .unwrap(),
        ];

        let forks = Announcement::forks_in(&all, &base_addr, Some(euc), Some(base_owner));
        assert_eq!(forks.len(), 1);
        assert_eq!(forks[0].id, "mirror");

        all.push(
            owned_announcements(
                2,
                &[
                    &["d", "u-fork"],
                    &["u", &base_addr.to_string()],
                    &["clone", "https://grasp.example/x/u-fork.git"],
                ],
            )
            .pop()
            .unwrap(),
        );
        let forks = Announcement::forks_in(&all, &base_addr, None, Some(base_owner));
        let ids: Vec<&str> = forks.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, vec!["u-fork"]);
    }
}

use nostr::prelude::*;
use signed_core::{Announcement, RepoAddr};

const MAINTAINER_HEX: &str = "68d81165918100b7da43fc28f7d1fc12554466e1115886b9e7bb326f65ec4272";

fn keys() -> Keys {
    Keys::new(
        SecretKey::from_hex("0000000000000000000000000000000000000000000000000000000000000001")
            .expect("valid secret key"),
    )
}

fn announcement_event(tags: &[&[&str]]) -> Event {
    let tags: Vec<Tag> = tags
        .iter()
        .map(|t| Tag::parse(t.to_vec()).expect("valid tag"))
        .collect();

    EventBuilder::new(Kind::GitRepoAnnouncement)
        .tags(tags)
        .finalize(&keys())
        .expect("signed event")
}

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

#[test]
fn effective_maintainers_include_owner_for_primary_repos() {
    let event = announcement_event(&[&["d", "my-repo"], &["maintainers", MAINTAINER_HEX]]);

    let announcement = Announcement::from_event(&event).expect("parses");
    let maintainers = announcement.effective_maintainers();

    assert_eq!(maintainers.len(), 2);
    assert!(maintainers.contains(&announcement.owner));
    assert!(maintainers.contains(&PublicKey::from_hex(MAINTAINER_HEX).expect("valid pubkey")));
}

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

fn owned_announcement_event(owner: &str, tags: &[&[&str]]) -> Event {
    let keys = Keys::new(SecretKey::from_hex(owner).expect("valid secret key"));
    let tags: Vec<Tag> = tags
        .iter()
        .map(|t| Tag::parse(t.to_vec()).expect("valid tag"))
        .collect();
    EventBuilder::new(Kind::GitRepoAnnouncement)
        .tags(tags)
        .finalize(&keys)
        .expect("signed event")
}

fn owned_announcements(owner_ix: usize, tags: &[&[&str]]) -> Vec<Announcement> {
    vec![
        Announcement::from_event(&owned_announcement_event(OWNER_KEYS[owner_ix], tags))
            .expect("parses"),
    ]
}

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

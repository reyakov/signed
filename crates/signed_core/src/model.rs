use std::collections::HashSet;

use nostr::prelude::*;

use crate::RepoAddr;

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

// The `u` tag of a fork announcement, per NIP-34.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Upstream {
    pub raw: String,
    // `None` for the git-URL form.
    pub addr: Option<RepoAddr>,
}

impl Upstream {
    fn parse(raw: &str) -> Self {
        let coordinate = raw.split('|').next().unwrap_or(raw);
        let addr = coordinate
            .parse::<Coordinate>()
            .ok()
            .filter(|coordinate| coordinate.kind == Kind::GitRepoAnnouncement)
            .map(RepoAddr::from);
        Self {
            raw: raw.to_owned(),
            addr,
        }
    }

    pub fn display(&self) -> String {
        match &self.addr {
            Some(addr) => addr.to_string(),
            None => self.raw.clone(),
        }
    }
}

pub trait GitEvent {
    fn activity_subject(&self) -> String;

    fn current_commit(&self) -> Option<String>;

    fn merge_base(&self) -> Option<String>;

    fn clone_urls(&self) -> Option<Vec<Url>>;

    fn branch_name(&self) -> Option<String>;

    fn is_git_activity(&self) -> bool;

    // Matches both NIP-10 lowercase `e` and NIP-22 uppercase `E` root pointers.
    fn references_root(&self, root: &EventId) -> bool;
}

impl GitEvent for Event {
    fn activity_subject(&self) -> String {
        let subject = self
            .tags
            .iter()
            .find_map(|tag| match Nip34Tag::parse(tag.as_slice()) {
                Ok(Nip34Tag::Subject(subject)) => Some(subject),
                _ => None,
            });

        subject
            .or_else(|| {
                self.content
                    .lines()
                    .map(str::trim)
                    .find(|line| !line.is_empty())
                    .map(|value| value.to_string())
            })
            .unwrap_or("Untitled".to_string())
    }

    fn current_commit(&self) -> Option<String> {
        self.tags
            .iter()
            .find_map(|tag| match Nip34Tag::parse(tag.as_slice()) {
                Ok(Nip34Tag::CurrentCommit(commit)) => Some(commit.to_string()),
                _ => None,
            })
    }

    fn merge_base(&self) -> Option<String> {
        self.tags
            .iter()
            .find_map(|tag| match Nip34Tag::parse(tag.as_slice()) {
                Ok(Nip34Tag::MergeBase(commit)) => Some(commit.to_string()),
                _ => None,
            })
    }

    fn clone_urls(&self) -> Option<Vec<Url>> {
        self.tags
            .iter()
            .find_map(|tag| match Nip34Tag::parse(tag.as_slice()) {
                Ok(Nip34Tag::Clone(urls)) => Some(urls),
                _ => None,
            })
    }

    fn branch_name(&self) -> Option<String> {
        self.tags
            .iter()
            .find_map(|tag| match Nip34Tag::parse(tag.as_slice()) {
                Ok(Nip34Tag::BranchName(name)) => Some(name),
                _ => None,
            })
    }

    fn is_git_activity(&self) -> bool {
        match self.kind {
            Kind::GitIssue | Kind::GitPatch | Kind::GitPullRequest => true,
            Kind::Comment => crate::filters::is_git_comment(self),
            Kind::GitStatusOpen
            | Kind::GitStatusApplied
            | Kind::GitStatusClosed
            | Kind::GitStatusDraft => crate::filters::is_git_status(self),
            _ => false,
        }
    }

    fn references_root(&self, root: &EventId) -> bool {
        let root = root.to_hex();
        self.tags
            .iter()
            .any(|tag| matches!(tag.kind(), "e" | "E") && tag.content() == Some(root.as_str()))
    }
}

impl<T: GitEvent + ?Sized> GitEvent for &T {
    fn activity_subject(&self) -> String {
        (*self).activity_subject()
    }

    fn current_commit(&self) -> Option<String> {
        (*self).current_commit()
    }

    fn merge_base(&self) -> Option<String> {
        (*self).merge_base()
    }

    fn clone_urls(&self) -> Option<Vec<Url>> {
        (*self).clone_urls()
    }

    fn branch_name(&self) -> Option<String> {
        (*self).branch_name()
    }

    fn is_git_activity(&self) -> bool {
        (*self).is_git_activity()
    }

    fn references_root(&self, root: &EventId) -> bool {
        (*self).references_root(root)
    }
}

pub struct PullRequest<'a>(pub &'a Event);

impl<'a> PullRequest<'a> {
    pub fn new(event: &'a Event) -> Self {
        Self(event)
    }

    // The PR references its root patch via an `e` tag.
    pub fn patches(&self, patches: impl IntoIterator<Item = &'a Event>) -> Vec<&'a Event> {
        let pr = self.0;
        let patches: Vec<&'a Event> = patches.into_iter().collect();

        if let Some(root_id) = pr.tags.event_ids().next()
            && let Some(root) = patches.iter().find(|patch| patch.id == root_id)
        {
            return Self::forward_series(root, &patches);
        }

        let Some(tip) = pr.current_commit() else {
            return Vec::new();
        };
        let Some(last) = patches
            .iter()
            .filter(|patch| Self::patch_produces_commit(patch, &tip))
            .max_by_key(|patch| patch.created_at)
            .copied()
        else {
            return Vec::new();
        };

        let mut series = vec![last];
        loop {
            let Some(prev_id) = series.last().unwrap().tags.event_ids().next() else {
                break;
            };
            let Some(prev) = patches
                .iter()
                .find(|patch| patch.id == prev_id && !series.contains(patch))
                .copied()
            else {
                break;
            };
            series.push(prev);
        }
        series.reverse();
        series
    }

    // Falls back to the root event's content when no patch set is found.
    pub fn patch(&self, patches: impl IntoIterator<Item = &'a Event>) -> String {
        let pr = self.0;
        let patches: Vec<&'a Event> = patches.into_iter().collect();
        let series = self.patches(patches.iter().copied());
        if series.is_empty() {
            return pr.content.clone();
        }
        series
            .iter()
            .map(|patch| patch.content.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }

    // A pull request's tip is only mutable by its author per NIP-34,
    // updates from anyone else are ignored even if they are newer.
    pub fn latest_update(
        events: impl Iterator<Item = &'a Event>,
        root: &Event,
    ) -> Option<&'a Event> {
        let root_hex = root.id.to_hex();
        events
            .filter(|e| e.kind == Kind::GitPullRequestUpdate)
            .filter(|e| e.pubkey == root.pubkey)
            .filter(|e| {
                e.tags
                    .iter()
                    .any(|t| t.kind() == "E" && t.content() == Some(root_hex.as_str()))
            })
            .max_by_key(|e| e.created_at)
    }

    fn forward_series(root: &'a Event, patches: &[&'a Event]) -> Vec<&'a Event> {
        let mut series = vec![root];
        loop {
            let next = patches
                .iter()
                .filter(|patch| !series.contains(patch))
                .filter(|patch| {
                    patch
                        .tags
                        .event_ids()
                        .any(|id| id == series.last().unwrap().id)
                })
                .max_by_key(|patch| patch.created_at);
            let Some(next) = next else {
                break;
            };
            series.push(next);
        }
        series
    }

    // Lets clients find existing patches for a specific commit.
    fn patch_produces_commit(patch: &Event, commit: &str) -> bool {
        patch
            .tags
            .iter()
            .any(|tag| match Nip34Tag::parse(tag.as_slice()) {
                Ok(Nip34Tag::Commit(c) | Nip34Tag::Reference(c)) => c.to_string() == commit,
                _ => false,
            })
    }
}

impl Announcement {
    // The user's own forks are listed first.
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

    /// Parse a kind `30617` event.
    ///
    /// Returns `None` when the kind is wrong or the `d` tag is missing.
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

            // The SDK's `Nip34Tag` does not model the `u` tag, so parse it manually.
            // Only the first `u` tag is used.
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

    pub fn addr(&self) -> RepoAddr {
        RepoAddr::new(self.owner, self.id.clone())
    }

    pub fn name(&self) -> String {
        self.name.clone().unwrap_or("Untitled".into())
    }

    // The `u` tag pointing at `base` also covers permanent forks whose EUC diverged.
    pub fn is_fork_of(&self, base: &RepoAddr, base_euc: Option<&str>) -> bool {
        if self.addr() == *base {
            return false;
        }
        if self.upstream.as_ref().and_then(|u| u.addr.as_ref()) == Some(base) {
            return true;
        }
        base_euc.is_some_and(|euc| self.euc.as_deref() == Some(euc))
    }

    pub fn description(&self) -> String {
        self.description
            .clone()
            .unwrap_or("No description".to_string())
    }

    // A `u` tag marking the repository as a subordinate fork excludes the
    // announcement author from the maintainers, per NIP-34.
    pub fn effective_maintainers(&self) -> Vec<PublicKey> {
        let mut maintainers = self.maintainers.clone();
        if self.upstream.is_none() && !maintainers.contains(&self.owner) {
            maintainers.push(self.owner);
        }
        maintainers
    }

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

        EventBuilder::new(Kind::GitRepoAnnouncement, "")
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

    fn pr_event(content: &str, tags: Vec<Tag>) -> Event {
        EventBuilder::new(Kind::GitPullRequest, content)
            .tags(tags)
            .finalize(&keys())
            .expect("signed event")
    }

    fn patch_event(content: &str, tags: Vec<Tag>, created_at: u64) -> Event {
        EventBuilder::new(Kind::GitPatch, content)
            .tags(tags)
            .custom_created_at(Timestamp::from(created_at))
            .finalize(&keys())
            .expect("signed event")
    }

    #[test]
    fn pull_request_patch_joins_the_whole_patch_set() {
        let root = patch_event("patch-one", vec![], 100);
        let second = patch_event("patch-two", vec![Tag::event(root.id)], 200);
        let pr = pr_event("description", vec![Tag::event(root.id)]);

        assert_eq!(
            PullRequest::new(&pr).patch([&root, &second]),
            "patch-one\npatch-two"
        );
        assert_eq!(
            PullRequest::new(&pr).patches([&root, &second]),
            vec![&root, &second]
        );
    }

    #[test]
    fn pull_request_patches_walks_the_reply_chain_in_order() {
        let root = patch_event("patch-one", vec![], 100);
        let second = patch_event("patch-two", vec![Tag::event(root.id)], 200);
        let third = patch_event("patch-three", vec![Tag::event(second.id)], 300);
        let pr = pr_event("description", vec![Tag::event(root.id)]);

        let series = PullRequest::new(&pr).patches([&third, &root, &second]);
        assert_eq!(
            series
                .iter()
                .map(|p| p.content.as_str())
                .collect::<Vec<_>>(),
            vec!["patch-one", "patch-two", "patch-three"]
        );
    }

    #[test]
    fn pull_request_patches_finds_the_set_via_the_tip_commit() {
        let root = patch_event("patch-one", vec![], 100);
        let tip = "1111111111111111111111111111111111111111";
        let last = patch_event(
            "patch-two",
            vec![
                Tag::event(root.id),
                Tag::parse(["r", tip]).expect("valid tag"),
            ],
            200,
        );
        let pr = pr_event(
            "description",
            vec![Tag::parse(["c", tip]).expect("valid tag")],
        );

        let series = PullRequest::new(&pr).patches([&root, &last]);
        assert_eq!(
            series
                .iter()
                .map(|p| p.content.as_str())
                .collect::<Vec<_>>(),
            vec!["patch-one", "patch-two"]
        );
    }

    const COMMIT_HEX: &str = "1111111111111111111111111111111111111111";
    const OTHER_ROOT_HEX: &str = "2222222222222222222222222222222222222222";

    fn signed_at(kind: Kind, tags: Vec<Tag>, created_at: u64) -> Event {
        EventBuilder::new(kind, "")
            .tags(tags)
            .custom_created_at(Timestamp::from(created_at))
            .finalize(&keys())
            .expect("signed event")
    }

    fn pr_root() -> Event {
        signed_at(
            Kind::GitPullRequest,
            vec![
                Tag::parse(["c", COMMIT_HEX]).expect("valid tag"),
                Tag::parse(["branch-name", "feature/x"]).expect("valid tag"),
            ],
            100,
        )
    }

    #[test]
    fn latest_update_picks_newest_revision_of_the_root() {
        let root = pr_root();
        let root_hex = root.id.to_hex();

        let revision = |created_at: u64| {
            signed_at(
                Kind::GitPullRequestUpdate,
                vec![Tag::parse(["E", &root_hex]).expect("valid tag")],
                created_at,
            )
        };
        let unrelated = signed_at(
            Kind::GitPullRequestUpdate,
            vec![Tag::parse(["E", OTHER_ROOT_HEX]).expect("valid tag")],
            999,
        );

        let events = [unrelated, revision(200), root.clone(), revision(300)];
        let latest = PullRequest::latest_update(events.iter(), &root).expect("an update");

        assert_eq!(latest.created_at.as_secs(), 300);
        assert_eq!(latest.kind, Kind::GitPullRequestUpdate);
    }

    #[test]
    fn latest_update_ignores_other_authors() {
        let root = pr_root();
        let root_hex = root.id.to_hex();
        let other = Keys::new(
            SecretKey::from_hex("0000000000000000000000000000000000000000000000000000000000000002")
                .expect("valid secret key"),
        );
        let stranger = EventBuilder::new(Kind::GitPullRequestUpdate, "")
            .tags([Tag::parse(["E", &root_hex]).expect("valid tag")])
            .custom_created_at(Timestamp::from(999))
            .finalize(&other)
            .expect("signed event");

        assert!(PullRequest::latest_update([&stranger, &root].into_iter(), &root).is_none());
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
        EventBuilder::new(Kind::GitRepoAnnouncement, "")
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
}

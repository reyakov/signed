use std::collections::HashSet;

use nostr::prelude::*;

use crate::{RepoAddr, repo_addr};

/// Parsed NIP-34 repository announcement, plain data ready for the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Announcement {
    /// ID of the announcement event itself.
    pub event_id: EventId,
    /// Repository ID, the `d` tag.
    pub id: String,
    /// Author of the announcement event.
    pub owner: PublicKey,
    /// When the announcement was published, used for latest-wins resolution.
    pub created_at: Timestamp,
    pub name: Option<String>,
    pub description: Option<String>,
    /// Webpage URLs for browsing.
    pub web: Vec<Url>,
    /// URLs for `git clone`.
    pub clone: Vec<Url>,
    /// Relays the repository monitors for patches and issues.
    pub relays: Vec<RelayUrl>,
    /// Earliest unique commit ID, the `r` tag with `euc` marker.
    pub euc: Option<String>,
    /// Other recognized maintainers.
    pub maintainers: Vec<PublicKey>,
    /// Marks the repository as a subordinate fork of the upstream, per NIP-34.
    pub upstream: Option<Upstream>,
    /// Hashtags labelling the repository, the `t` tags.
    pub hashtags: Vec<String>,
}

/// The `u` tag of a fork announcement, per NIP-34.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Upstream {
    /// Raw first value of the `u` tag, a coordinate or git URL.
    pub raw: String,
    /// Upstream repository coordinate when the `u` tag names a NIP-34 repository.
    /// `None` for the git-URL form.
    pub addr: Option<RepoAddr>,
    /// Relay hint for the upstream, if the `u` tag carries one.
    pub relay_hint: Option<RelayUrl>,
}

impl Upstream {
    /// Parse the `u` tag values.
    fn parse(raw: &str, relay_hint: Option<&str>) -> Self {
        let coordinate = raw.split('|').next().unwrap_or(raw);
        let addr = coordinate
            .parse::<Coordinate>()
            .ok()
            .filter(|c| c.kind == Kind::GitRepoAnnouncement);
        Self {
            raw: raw.to_owned(),
            addr,
            relay_hint: relay_hint.and_then(|hint| RelayUrl::parse(hint).ok()),
        }
    }

    /// Text for display.
    pub fn display(&self) -> String {
        match &self.addr {
            Some(addr) => addr.to_string(),
            None => self.raw.clone(),
        }
    }
}

/// Subject of a NIP-34 issue or pull request event.
/// Taken from the `subject` tag, else the first non-empty line of the content.
pub fn activity_subject(event: &Event) -> String {
    let subject = event
        .tags
        .iter()
        .find_map(|tag| match Nip34Tag::parse(tag.as_slice()) {
            Ok(Nip34Tag::Subject(subject)) => Some(subject),
            _ => None,
        });

    subject
        .or_else(|| {
            event
                .content
                .lines()
                .map(str::trim)
                .find(|line| !line.is_empty())
                .map(|value| value.to_string())
        })
        .unwrap_or("Untitled".to_string())
}

/// The patch set of a pull request.
///
/// Returns an empty list when no patch event can be linked to the PR.
pub fn pull_request_patches<'a>(
    pr: &Event,
    patches: impl IntoIterator<Item = &'a Event>,
) -> Vec<&'a Event> {
    let patches: Vec<&'a Event> = patches.into_iter().collect();

    // The PR references its root patch via an `e` tag.
    // Follow the NIP-10 reply chain forward from there.
    // Each patch replies to the previous one, and among several replies the newest wins.
    if let Some(root_id) = pr.tags.event_ids().next()
        && let Some(root) = patches.iter().find(|patch| patch.id == root_id)
    {
        return forward_series(root, &patches);
    }

    // The PR has no `e` tag.
    // The last patch of the set carries the tip commit in its `commit` or `r` tag.
    // Walk the reply chain backward to the root.
    let Some(tip) = current_commit_of(pr) else {
        return Vec::new();
    };
    let Some(last) = patches
        .iter()
        .filter(|patch| patch_produces_commit(patch, &tip))
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

/// The patch content of a pull request.
pub fn pull_request_patch<'a>(pr: &Event, patches: impl IntoIterator<Item = &'a Event>) -> String {
    let patches: Vec<&'a Event> = patches.into_iter().collect();
    let series = pull_request_patches(pr, patches.iter().copied());
    if series.is_empty() {
        return pr.content.clone();
    }
    series
        .iter()
        .map(|patch| patch.content.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The chain of patches replying to `root` via NIP-10 `e` tags, oldest first.
fn forward_series<'a>(root: &'a Event, patches: &[&'a Event]) -> Vec<&'a Event> {
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

/// The `c` tag of an event, the tip of the proposed branch, as hex.
pub fn current_commit_of(event: &Event) -> Option<String> {
    event
        .tags
        .iter()
        .find_map(|tag| match Nip34Tag::parse(tag.as_slice()) {
            Ok(Nip34Tag::CurrentCommit(commit)) => Some(commit.to_string()),
            _ => None,
        })
}

/// The `merge-base` tag of an event, the base commit a pull request diffs against.
pub fn merge_base_of(event: &Event) -> Option<String> {
    event
        .tags
        .iter()
        .find_map(|tag| match Nip34Tag::parse(tag.as_slice()) {
            Ok(Nip34Tag::MergeBase(commit)) => Some(commit.to_string()),
            _ => None,
        })
}

/// The `clone` tag of an event, URLs the tip commit can be fetched from.
pub fn clone_urls_of(event: &Event) -> Option<Vec<Url>> {
    event
        .tags
        .iter()
        .find_map(|tag| match Nip34Tag::parse(tag.as_slice()) {
            Ok(Nip34Tag::Clone(urls)) => Some(urls),
            _ => None,
        })
}

/// The `branch-name` tag of an event, the proposed branch's name.
pub fn branch_name_of(event: &Event) -> Option<String> {
    event
        .tags
        .iter()
        .find_map(|tag| match Nip34Tag::parse(tag.as_slice()) {
            Ok(Nip34Tag::BranchName(name)) => Some(name),
            _ => None,
        })
}

/// The newest `GitPullRequestUpdate` revising `root`, from the root's own author.
///
/// A pull request's tip is only mutable by its author, per NIP-34; updates
/// from anyone else are ignored even if they are newer.
pub fn latest_update<'a>(
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

/// The announced forks of `base` a new pull request compare can be built from.
///
/// The user's own forks are listed first.
pub fn fork_candidates<'a>(
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

/// Whether `patch` produces `commit`, found via its `commit` or `r` tag.
///
/// It lets clients find existing patches for a specific commit.
fn patch_produces_commit(patch: &Event, commit: &str) -> bool {
    patch
        .tags
        .iter()
        .any(|tag| match Nip34Tag::parse(tag.as_slice()) {
            Ok(Nip34Tag::Commit(c) | Nip34Tag::Reference(c)) => c.to_string() == commit,
            _ => false,
        })
}

impl Announcement {
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
                    upstream = Some(Upstream::parse(raw, values.get(2).map(String::as_str)));
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

    /// The repository address of this announcement.
    pub fn addr(&self) -> RepoAddr {
        repo_addr(self.owner, self.id.clone())
    }

    /// The name of the repository, or a default if none is provided.
    pub fn name(&self) -> String {
        self.name.clone().unwrap_or("Untitled".into())
    }

    /// Whether this announcement is a fork of the repository at `base`.
    /// Its `u` tag points at `base`, which also covers permanent forks whose EUC diverged.
    ///
    /// Or it shares `base`'s earliest unique commit and is not the base itself.
    pub fn is_fork_of(&self, base: &RepoAddr, base_euc: Option<&str>) -> bool {
        if self.addr() == *base {
            return false;
        }
        if self.upstream.as_ref().and_then(|u| u.addr.as_ref()) == Some(base) {
            return true;
        }
        base_euc.is_some_and(|euc| self.euc.as_deref() == Some(euc))
    }

    /// The description of the repository, or a default if none is provided.
    pub fn description(&self) -> String {
        self.description
            .clone()
            .unwrap_or("No description".to_string())
    }

    /// The effective maintainers of this repository,
    /// the announced `maintainers` plus the announcement author.
    ///
    /// A `u` tag that marks the repository as a subordinate fork excludes them, per NIP-34.
    pub fn effective_maintainers(&self) -> Vec<PublicKey> {
        let mut maintainers = self.maintainers.clone();
        if self.upstream.is_none() && !maintainers.contains(&self.owner) {
            maintainers.push(self.owner);
        }
        maintainers
    }

    /// The `git clone` URLs for this repository, deduplicated.
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

    /// Build a signed kind `30617` event from raw tag values.
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
    fn requires_d_tag() {
        let event = announcement_event(&[&["name", "No id"]]);

        assert!(Announcement::from_event(&event).is_none());
    }

    #[test]
    fn ignores_other_kinds() {
        let event = EventBuilder::new(Kind::GitIssue, "")
            .finalize(&keys())
            .expect("signed event");

        assert!(Announcement::from_event(&event).is_none());
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

        // An invalid URL keeps the whole clone tag from being parsed.
        assert!(announcement.clone.is_empty());
        assert_eq!(
            announcement.relays,
            vec![RelayUrl::parse("wss://good.example.com").unwrap()]
        );
        assert!(announcement.maintainers.is_empty());
    }

    #[test]
    fn ignores_unknown_tags() {
        let event = announcement_event(&[&["d", "my-repo"], &["t", "label"], &["subject", "n/a"]]);

        let announcement = Announcement::from_event(&event).expect("parses");

        assert_eq!(announcement.id, "my-repo");
        assert!(announcement.name.is_none());
        assert!(announcement.web.is_empty());
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

        // The coordinate part resolves to a repository address.
        // The raw value keeps the `|git-url` suffix.
        assert_eq!(
            upstream.addr,
            Some(crate::repo_addr(
                PublicKey::from_hex(MAINTAINER_HEX).expect("valid pubkey"),
                "upstream"
            ))
        );
        assert_eq!(
            upstream.raw,
            "30617:68d81165918100b7da43fc28f7d1fc12554466e1115886b9e7bb326f65ec4272:upstream|https://example.com/upstream.git"
        );
        assert_eq!(
            upstream.relay_hint,
            Some(RelayUrl::parse("wss://relay.example.com").expect("valid relay"))
        );
        assert_eq!(
            upstream.display().to_string(),
            "30617:68d81165918100b7da43fc28f7d1fc12554466e1115886b9e7bb326f65ec4272:upstream"
        );
    }

    #[test]
    fn parses_git_url_upstream() {
        // The `u` tag may reference a non-nostr upstream by git URL only.
        // There is no repository address to navigate to.
        let event = announcement_event(&[
            &["d", "my-fork"],
            &["u", "https://example.com/upstream.git"],
        ]);

        let announcement = Announcement::from_event(&event).expect("parses");
        let upstream = announcement.upstream.expect("parses the u tag");

        assert_eq!(upstream.addr, None);
        assert_eq!(
            upstream.display().to_string(),
            "https://example.com/upstream.git"
        );
    }

    #[test]
    fn is_fork_of_matches_the_u_tag_coordinate() {
        // The base repository, announced by the `u` tag's owner.
        let base = crate::repo_addr(
            PublicKey::from_hex(MAINTAINER_HEX).expect("valid pubkey"),
            "upstream",
        );
        let event = announcement_event(&[&["d", "my-fork"], &["u", &base.to_string()]]);
        let fork = Announcement::from_event(&event).expect("parses");

        // A `u` tag pointing at the base address marks a fork.
        // This holds even when neither side announces an EUC.
        assert!(fork.is_fork_of(&base, None));
    }

    #[test]
    fn is_fork_of_matches_a_shared_euc() {
        let euc = "aa231c4c6a5777dc89b42207b499891a344add5c";
        // The base repo has no `u` tag. It announces the family EUC.
        let base_event = announcement_event(&[&["d", "upstream"], &["r", euc, "euc"]]);
        let base = Announcement::from_event(&base_event).expect("parses");
        let base_addr = base.addr();

        // A fork with no `u` tag, a pure mirror or cross-hosted clone, shares the EUC.
        // Clients of the family can then find it.
        let fork_event = announcement_event(&[&["d", "mirror"], &["r", euc, "euc"]]);
        let fork = Announcement::from_event(&fork_event).expect("parses");
        assert!(fork.is_fork_of(&base_addr, base.euc.as_deref()));

        // An unrelated repository with a different EUC is not a fork.
        let other_event = announcement_event(&[
            &["d", "other"],
            &["r", "bb231c4c6a5777dc89b42207b499891a344add5c", "euc"],
        ]);
        let other = Announcement::from_event(&other_event).expect("parses");
        assert!(!other.is_fork_of(&base_addr, base.euc.as_deref()));

        // Without a base EUC there is nothing to compare against.
        assert!(!fork.is_fork_of(&base_addr, None));
    }

    #[test]
    fn is_fork_of_matches_permanent_forks_with_a_diverged_euc() {
        // A permanent fork re-announces its EUC, the first commit after the fork.
        // Only the `u` tag still relates it to the base.
        let base = crate::repo_addr(
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
    fn is_fork_of_excludes_the_base_itself() {
        let euc = "aa231c4c6a5777dc89b42207b499891a344add5c";
        let event = announcement_event(&[&["d", "upstream"], &["r", euc, "euc"]]);
        let base = Announcement::from_event(&event).expect("parses");
        let base_addr = base.addr();

        // The base announcement matches its own EUC but is not a fork of itself.
        assert!(!base.is_fork_of(&base_addr, base.euc.as_deref()));
    }

    #[test]
    fn effective_maintainers_include_owner_for_primary_repos() {
        let event = announcement_event(&[&["d", "my-repo"], &["maintainers", MAINTAINER_HEX]]);

        let announcement = Announcement::from_event(&event).expect("parses");
        let maintainers = announcement.effective_maintainers();

        // The owner asserts themselves as a maintainer of the primary project, per NIP-34.
        // Announced co-maintainers are included too.
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

        // A `u` tag marks the repository as a subordinate fork.
        // The author is then not a maintainer of the primary project, per NIP-34.
        assert!(!maintainers.contains(&announcement.owner));
        assert_eq!(
            maintainers,
            vec![PublicKey::from_hex(MAINTAINER_HEX).expect("valid pubkey")]
        );
    }

    /// Build a signed PR event with the given tags and content.
    fn pr_event(content: &str, tags: Vec<Tag>) -> Event {
        EventBuilder::new(Kind::GitPullRequest, content)
            .tags(tags)
            .finalize(&keys())
            .expect("signed event")
    }

    #[test]
    fn pull_request_patch_prefers_linked_patch_event() {
        let patch = EventBuilder::new(Kind::GitPatch, "patch-content")
            .finalize(&keys())
            .expect("signed event");
        let pr = pr_event("description", vec![Tag::event(patch.id)]);

        assert_eq!(pull_request_patch(&pr, [&patch]), "patch-content");
    }

    #[test]
    fn pull_request_patch_falls_back_to_inline_content() {
        // Older PRs carried the patch in the content and link no patch event.
        let pr = pr_event("patch-inline", vec![]);

        assert_eq!(pull_request_patch(&pr, [] as [&Event; 0]), "patch-inline");
    }

    #[test]
    fn pull_request_patch_ignores_unrelated_patch_events() {
        let patch = EventBuilder::new(Kind::GitPatch, "patch-content")
            .finalize(&keys())
            .expect("signed event");
        let pr = pr_event("description", vec![]);

        assert_eq!(pull_request_patch(&pr, [&patch]), "description");
    }

    /// Build a signed patch event with a controlled `created_at`.
    fn patch_event(content: &str, tags: Vec<Tag>, created_at: u64) -> Event {
        EventBuilder::new(Kind::GitPatch, content)
            .tags(tags)
            .custom_created_at(Timestamp::from(created_at))
            .finalize(&keys())
            .expect("signed event")
    }

    #[test]
    fn pull_request_patch_joins_the_whole_patch_set() {
        // A PR references the root patch, per NIP-34.
        // Later patches of the set reply to the previous one via NIP-10 `e` tags.
        let root = patch_event("patch-one", vec![], 100);
        let second = patch_event("patch-two", vec![Tag::event(root.id)], 200);
        let pr = pr_event("description", vec![Tag::event(root.id)]);

        assert_eq!(
            pull_request_patch(&pr, [&root, &second]),
            "patch-one\npatch-two"
        );
        assert_eq!(
            pull_request_patches(&pr, [&root, &second]),
            vec![&root, &second]
        );
    }

    #[test]
    fn pull_request_patches_walks_the_reply_chain_in_order() {
        let root = patch_event("patch-one", vec![], 100);
        let second = patch_event("patch-two", vec![Tag::event(root.id)], 200);
        let third = patch_event("patch-three", vec![Tag::event(second.id)], 300);
        let pr = pr_event("description", vec![Tag::event(root.id)]);

        let series = pull_request_patches(&pr, [&third, &root, &second]);
        assert_eq!(
            series
                .iter()
                .map(|p| p.content.as_str())
                .collect::<Vec<_>>(),
            vec!["patch-one", "patch-two", "patch-three"]
        );
    }

    #[test]
    fn pull_request_patches_ignores_unrelated_replies() {
        let root = patch_event("patch-one", vec![], 100);
        let other = patch_event("other-patch", vec![Tag::event(root.id)], 250);
        // A patch replying to a different root is not part of the set.
        let stranger = patch_event("stranger", vec![], 150);
        let pr = pr_event("description", vec![Tag::event(root.id)]);

        let series = pull_request_patches(&pr, [&root, &other, &stranger]);
        assert_eq!(
            series
                .iter()
                .map(|p| p.content.as_str())
                .collect::<Vec<_>>(),
            vec!["patch-one", "other-patch"]
        );
    }

    #[test]
    fn pull_request_patches_finds_the_set_via_the_tip_commit() {
        // PRs without an `e` tag fall back to the patch producing the tip commit.
        // Walk the reply chain backward to the root.
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

        let series = pull_request_patches(&pr, [&root, &last]);
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

    /// Build a signed event of `kind` with the given tags and `created_at`.
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
    fn reads_current_commit_and_branch_name() {
        let pr = pr_root();
        assert_eq!(current_commit_of(&pr).as_deref(), Some(COMMIT_HEX));
        assert_eq!(branch_name_of(&pr).as_deref(), Some("feature/x"));
    }

    #[test]
    fn returns_none_without_pr_tags() {
        let pr = signed_at(Kind::GitPullRequest, vec![], 100);
        assert_eq!(current_commit_of(&pr), None);
        assert_eq!(branch_name_of(&pr), None);
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
        // An update revising a different PR must be ignored even though it is newer.
        let unrelated = signed_at(
            Kind::GitPullRequestUpdate,
            vec![Tag::parse(["E", OTHER_ROOT_HEX]).expect("valid tag")],
            999,
        );

        let events = [unrelated, revision(200), root.clone(), revision(300)];
        let latest = latest_update(events.iter(), &root).expect("an update");

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

        // The tip of a PR is only mutable by its author.
        // A newer update from anyone else must not win.
        assert!(latest_update([&stranger, &root].into_iter(), &root).is_none());
    }

    #[test]
    fn latest_update_ignores_roots_without_revisions() {
        let root = pr_root();
        assert!(latest_update([&root].into_iter(), &root).is_none());
    }

    const OWNER_KEYS: [&str; 3] = [
        "0000000000000000000000000000000000000000000000000000000000000001",
        "0000000000000000000000000000000000000000000000000000000000000002",
        "0000000000000000000000000000000000000000000000000000000000000003",
    ];

    /// Build a signed kind-30617 event for `owner` with the given tags.
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

        let base_addr = crate::repo_addr(
            PublicKey::from_hex(OWNER_KEYS[0]).expect("pubkey"),
            "upstream",
        );
        // Newest first, as RepoListStore keeps them.
        // Unrelated repo, the user's fork with the shared EUC, another fork with a `u` tag.
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
        let forks = fork_candidates(&all, &base_addr, Some(euc), Some(user));

        // The user's fork comes first, then the other author's.
        let ids: Vec<&str> = forks.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, vec!["my-fork", "their-fork"]);
    }

    #[test]
    fn fork_candidates_excludes_base_unrelated_and_unfetchable() {
        let euc = "aa231c4c6a5777dc89b42207b499891a344add5c";
        let base_owner = PublicKey::from_hex(OWNER_KEYS[0]).expect("pubkey");
        let base_addr = crate::repo_addr(base_owner, "upstream");

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

        let forks = fork_candidates(&all, &base_addr, Some(euc), Some(base_owner));
        assert_eq!(forks.len(), 1);
        assert_eq!(forks[0].id, "mirror");

        // Without a base EUC only `u`-tag forks match.
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
        let forks = fork_candidates(&all, &base_addr, None, Some(base_owner));
        let ids: Vec<&str> = forks.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, vec!["u-fork"]);
    }
}

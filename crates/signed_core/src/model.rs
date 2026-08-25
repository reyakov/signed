use gpui::SharedString;
use nostr::prelude::*;

/// Parsed NIP-34 repository announcement (plain data, ready for the UI).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Announcement {
    /// Repository ID (`d` tag).
    pub id: String,
    /// Author of the announcement event.
    pub owner: PublicKey,
    /// When the announcement was published (for latest-wins resolution).
    pub created_at: Timestamp,
    pub name: Option<SharedString>,
    pub description: Option<SharedString>,
    /// Webpage URLs for browsing.
    pub web: Vec<Url>,
    /// URLs for `git clone`.
    pub clone: Vec<Url>,
    /// Relays the repository monitors for patches and issues.
    pub relays: Vec<RelayUrl>,
    /// Earliest unique commit ID (`r` tag with `euc` marker).
    pub euc: Option<String>,
    /// Other recognized maintainers.
    pub maintainers: Vec<PublicKey>,
    /// Value of a `u` tag, if any: this repository is a subordinate fork of
    /// the referenced upstream (NIP-34).
    pub upstream: Option<String>,
    /// Hashtags labelling the repository (`t` tags).
    pub hashtags: Vec<String>,
}

/// Subject of a NIP-34 issue or pull request event: the `subject` tag,
/// falling back to the first non-empty line of the content.
pub fn activity_subject(event: &Event) -> SharedString {
    let subject = event
        .tags
        .iter()
        .find_map(|tag| match Nip34Tag::parse(tag.as_slice()) {
            Ok(Nip34Tag::Subject(subject)) => Some(subject),
            _ => None,
        });

    subject
        .map(SharedString::from)
        .or_else(|| {
            event
                .content
                .lines()
                .map(str::trim)
                .find(|line| !line.is_empty())
                .map(SharedString::from)
        })
        .unwrap_or(SharedString::from("Untitled"))
}

/// The patch set of a pull request: the root patch event (kind `1617`) the
/// PR references via its `e` tag, plus every patch of the set chained to it
/// with NIP-10 `e` reply tags, in series order (oldest first). When the PR
/// has no `e` tag, falls back to the patch producing the PR's tip commit
/// (its `commit`/`r` tag, per NIP-34) and walks the reply chain backward to
/// the root.
///
/// Returns an empty list when no patch event can be linked to the PR.
pub fn pull_request_patches<'a>(
    pr: &Event,
    patches: impl IntoIterator<Item = &'a Event>,
) -> Vec<&'a Event> {
    let patches: Vec<&'a Event> = patches.into_iter().collect();

    // The PR references its root patch via an `e` tag; follow the NIP-10
    // reply chain forward from there (each patch of the set replies to the
    // previous one). Among several replies (a revision), the newest wins.
    if let Some(root_id) = pr.tags.event_ids().next()
        && let Some(root) = patches.iter().find(|patch| patch.id == root_id)
    {
        return forward_series(root, &patches);
    }

    // No `e` tag: the last patch of the set carries the PR's tip commit in
    // its `commit`/`r` tag; walk the reply chain backward to the root.
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

/// The patch content of a pull request: the contents of every patch event of
/// its patch set (see [`pull_request_patches`]) joined in series order,
/// falling back to the PR's own content for older PRs that carried the
/// patch inline.
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

/// The chain of patches replying to `root` (NIP-10 `e` tags), oldest first.
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

/// The `c` tag of an event (tip of the proposed branch), as hex.
fn current_commit_of(event: &Event) -> Option<String> {
    event
        .tags
        .iter()
        .find_map(|tag| match Nip34Tag::parse(tag.as_slice()) {
            Ok(Nip34Tag::CurrentCommit(commit)) => Some(commit.to_string()),
            _ => None,
        })
}

/// Whether `patch` produces `commit` (its `commit` or `r` tag), so clients
/// can find existing patches for a specific commit.
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
    /// Parse a kind `30617` event. Returns `None` if the kind is wrong or the `d` tag is missing.
    pub fn from_event(event: &Event) -> Option<Self> {
        if event.kind != Kind::GitRepoAnnouncement {
            return None;
        }

        let id = event.tags.identifier()?;

        let mut hashtags: Vec<String> = Vec::new();
        hashtags.extend(event.tags.hashtags().map(|t| t.to_string()));

        let mut name: Option<SharedString> = None;
        let mut description: Option<SharedString> = None;
        let mut web: Vec<Url> = Vec::new();
        let mut clone: Vec<Url> = Vec::new();
        let mut relays: Vec<RelayUrl> = Vec::new();
        let mut euc: Option<String> = None;
        let mut maintainers: Vec<PublicKey> = Vec::new();
        let mut upstream: Option<String> = None;

        for tag in event.tags.iter() {
            match Nip34Tag::parse(tag.as_slice()) {
                Ok(Nip34Tag::Name(value)) => name = Some(value.into()),
                Ok(Nip34Tag::Description(value)) => description = Some(value.into()),
                Ok(Nip34Tag::Web(urls)) => web.extend(urls),
                Ok(Nip34Tag::Clone(urls)) => clone.extend(urls),
                Ok(Nip34Tag::Relays(urls)) => relays.extend(urls),
                Ok(Nip34Tag::EarliestUniqueCommitId(commit)) => euc = Some(commit.to_string()),
                Ok(Nip34Tag::Maintainers(keys)) => maintainers.extend(keys),
                _ => {}
            }

            // The `u` tag is not modelled by the SDK's `Nip34Tag`; parse it
            // manually (first value wins).
            if upstream.is_none() && tag.kind() == "u" {
                upstream = tag.content().map(str::to_owned);
            }
        }

        Some(Self {
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
    pub fn addr(&self) -> crate::RepoAddr {
        crate::repo_addr(self.owner, self.id.clone())
    }

    /// The description of the repository, or a default if none is provided.
    pub fn description(&self) -> SharedString {
        self.description
            .clone()
            .unwrap_or(SharedString::from("No description"))
    }

    /// The effective maintainers of this repository: the announced
    /// `maintainers` plus the announcement author, who asserts themselves as
    /// a maintainer of the primary project unless a `u` tag marks this
    /// repository as a subordinate fork (NIP-34).
    pub fn effective_maintainers(&self) -> Vec<PublicKey> {
        let mut maintainers = self.maintainers.clone();
        if self.upstream.is_none() && !maintainers.contains(&self.owner) {
            maintainers.push(self.owner);
        }
        maintainers
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
            &["u", "30617:abc:upstream|https://example.com/upstream.git"],
        ]);

        let announcement = Announcement::from_event(&event).expect("parses");

        assert_eq!(
            announcement.upstream.as_deref(),
            Some("30617:abc:upstream|https://example.com/upstream.git")
        );
    }

    #[test]
    fn effective_maintainers_include_owner_for_primary_repos() {
        let event = announcement_event(&[&["d", "my-repo"], &["maintainers", MAINTAINER_HEX]]);

        let announcement = Announcement::from_event(&event).expect("parses");
        let maintainers = announcement.effective_maintainers();

        // The owner asserts themselves as a maintainer of the primary
        // project (NIP-34), alongside the announced co-maintainers.
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

        // A `u` tag marks the repository as a subordinate fork: the author
        // is not a maintainer of the primary project (NIP-34).
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
        // Older PRs carried the patch in the content; no linked patch event.
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
        // NIP-34: a PR references the root patch; later patches of the set
        // reply to the previous one (NIP-10 `e` tags).
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
        // PRs without an `e` tag: the last patch of the set carries the tip
        // commit in its `r` tag; walk the reply chain backward to the root.
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
}

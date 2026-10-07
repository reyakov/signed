use nostr::prelude::*;

use crate::git_event::GitEvent;

pub struct PullRequest<'a>(pub &'a Event);

impl<'a> PullRequest<'a> {
    /// Wraps a pull request root event.
    pub fn new(event: &'a Event) -> Self {
        Self(event)
    }

    /// Returns the ordered patch series, found via the root reference or the tip commit.
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

    /// Joins the patch series with newlines, falling back to the event content.
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

    /// Returns the root author's newest update, since only the author may move the tip.
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

    /// Walks reply links outward from the root patch, taking the newest child first.
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

    /// Checks the commit against the patch's `c` and `r` tags.
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Returns the test signer keys.
    fn keys() -> Keys {
        Keys::new(
            SecretKey::from_hex("0000000000000000000000000000000000000000000000000000000000000001")
                .expect("valid secret key"),
        )
    }

    /// Builds a signed pull request event.
    fn pr_event(content: &str, tags: Vec<Tag>) -> Event {
        EventBuilder::new(Kind::GitPullRequest, content)
            .tags(tags)
            .finalize(&keys())
            .expect("signed event")
    }

    /// Builds a signed patch event with a fixed timestamp.
    fn patch_event(content: &str, tags: Vec<Tag>, created_at: u64) -> Event {
        EventBuilder::new(Kind::GitPatch, content)
            .tags(tags)
            .custom_created_at(Timestamp::from(created_at))
            .finalize(&keys())
            .expect("signed event")
    }

    /// Joins the full patch series into one diff.
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

    /// Orders patches by walking the reply chain.
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

    /// Finds the patch series through the declared tip commit.
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

    /// Builds a signed event with a fixed timestamp.
    fn signed_at(kind: Kind, tags: Vec<Tag>, created_at: u64) -> Event {
        EventBuilder::new(kind, "")
            .tags(tags)
            .custom_created_at(Timestamp::from(created_at))
            .finalize(&keys())
            .expect("signed event")
    }

    /// Builds the pull request root used by the update tests.
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

    /// Picks the newest update targeting the root.
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

    /// Ignores updates from authors other than the root owner.
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
}

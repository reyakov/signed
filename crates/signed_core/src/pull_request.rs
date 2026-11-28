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
        let mut tail = last;

        loop {
            let Some(prev_id) = tail.tags.event_ids().next() else {
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
            tail = prev;
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
        let mut tail = root;

        loop {
            let next = patches
                .iter()
                .filter(|patch| !series.contains(patch))
                .filter(|patch| patch.tags.event_ids().any(|id| id == tail.id))
                .max_by_key(|patch| patch.created_at);

            let Some(next) = next else {
                break;
            };

            series.push(next);
            tail = next;
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

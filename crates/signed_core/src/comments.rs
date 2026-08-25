use std::collections::{HashMap, HashSet};

use nostr::prelude::*;

/// A NIP-22 comment thread: a top-level comment on the root event and its
/// nested replies (oldest first at every level).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommentThread {
    /// The thread's top-level comment.
    pub comment: Event,
    /// Replies to [`Self::comment`], nested recursively.
    pub replies: Vec<CommentThread>,
}

/// The direct parent of a comment (NIP-22 lowercase `e` tag), or `None` for
/// comments without one.
fn comment_parent(event: &Event) -> Option<EventId> {
    event
        .tags
        .iter()
        .find(|tag| tag.kind() == "e")
        .and_then(Tag::content)
        .and_then(|id| EventId::parse(id).ok())
}

/// Group the comments on a root event (issue / patch / PR) into NIP-22
/// threads. A comment whose parent is the root itself starts a thread; other
/// comments nest under their parent comment. Threads and replies are ordered
/// oldest-first. Replies whose parent comment is missing (e.g. not fetched)
/// are placed as top-level threads so they are not dropped.
pub fn comment_threads(root: &Event, comments: &[Event]) -> Vec<CommentThread> {
    // Index comments by their parent id. Comments without a parent tag are
    // treated as replying to the root event itself.
    let mut children: HashMap<EventId, Vec<&Event>> = HashMap::new();
    for comment in comments {
        let parent = comment_parent(comment).unwrap_or(root.id);
        children.entry(parent).or_default().push(comment);
    }
    for list in children.values_mut() {
        list.sort_by_key(|event| event.created_at);
    }

    let mut visited: HashSet<EventId> = HashSet::new();

    fn build(
        id: EventId,
        children: &HashMap<EventId, Vec<&Event>>,
        visited: &mut HashSet<EventId>,
    ) -> Vec<CommentThread> {
        let Some(list) = children.get(&id) else {
            return Vec::new();
        };
        let mut threads = Vec::new();
        for event in list {
            // Guards against malformed reply cycles.
            if visited.insert(event.id) {
                threads.push(CommentThread {
                    comment: (*event).clone(),
                    replies: build(event.id, children, visited),
                });
            }
        }
        threads
    }

    let mut threads = build(root.id, &children, &mut visited);

    // Orphan replies: their parent comment is unknown, so they never appear
    // in the tree rooted at the root event; surface them as top-level threads.
    let mut orphans: Vec<&Event> = comments
        .iter()
        .filter(|event| !visited.contains(&event.id))
        .collect();
    orphans.sort_by_key(|event| event.created_at);
    for comment in orphans {
        if visited.insert(comment.id) {
            threads.push(CommentThread {
                comment: comment.clone(),
                replies: build(comment.id, &children, &mut visited),
            });
        }
    }

    threads
}

#[cfg(test)]
mod tests {
    use super::*;

    fn comment(keys: &Keys, parent: Option<&Event>, content: &str, created_at: u64) -> Event {
        let tags = parent
            .map(|parent| vec![Tag::parse(["e", &parent.id.to_hex()]).expect("valid e tag")])
            .unwrap_or_default();
        EventBuilder::new(Kind::Comment, content)
            .tags(tags)
            .custom_created_at(Timestamp::from(created_at))
            .finalize(keys)
            .expect("signed event")
    }

    fn flatten(threads: &[CommentThread]) -> Vec<String> {
        let mut out = Vec::new();
        for thread in threads {
            out.push(thread.comment.content.clone());
            out.extend(flatten(&thread.replies));
        }
        out
    }

    #[test]
    fn nests_replies_under_their_parents() {
        let keys = Keys::generate();
        let root = EventBuilder::new(Kind::GitIssue, "issue")
            .finalize(&keys)
            .expect("signed event");

        let a = comment(&keys, Some(&root), "a", 100);
        let a1 = comment(&keys, Some(&a), "a1", 200);
        let a2 = comment(&keys, Some(&a), "a2", 300);
        let b = comment(&keys, Some(&root), "b", 150);

        let threads = comment_threads(&root, &[a2, b, a, a1]);

        assert_eq!(flatten(&threads), vec!["a", "a1", "a2", "b"]);
    }

    #[test]
    fn comments_without_a_parent_tag_attach_to_the_root() {
        let keys = Keys::generate();
        let root = EventBuilder::new(Kind::GitIssue, "issue")
            .finalize(&keys)
            .expect("signed event");

        // Old-style comments carried no `e` tag at all.
        let orphan = comment(&keys, None, "no parent", 100);

        let threads = comment_threads(&root, &[orphan]);

        assert_eq!(flatten(&threads), vec!["no parent"]);
    }

    #[test]
    fn orphan_replies_are_surfaced_as_top_level_threads() {
        let keys = Keys::generate();
        let root = EventBuilder::new(Kind::GitIssue, "issue")
            .finalize(&keys)
            .expect("signed event");
        let a = comment(&keys, Some(&root), "a", 100);

        // `missing` is not in the comment set; its reply should still show up.
        let missing = EventBuilder::new(Kind::Comment, "missing")
            .finalize(&keys)
            .expect("signed event");
        let reply_to_missing = comment(&keys, Some(&missing), "reply to missing", 200);

        let threads = comment_threads(&root, &[a, reply_to_missing]);

        assert_eq!(flatten(&threads), vec!["a", "reply to missing"]);
    }
}

use std::collections::HashSet;

use nostr::prelude::*;

pub struct ThreadResolver<'a, L: ?Sized> {
    lookup: &'a L,
}

impl<'a, L> ThreadResolver<'a, L>
where
    L: Fn(EventId) -> Option<Event> + ?Sized,
{
    /// Creates a resolver over the given event lookup.
    pub fn new(lookup: &'a L) -> Self {
        Self { lookup }
    }

    /// Maps a git event kind to the thread root it notifies about.
    pub fn notification_root(&self, event: &Event) -> Option<EventId> {
        match event.kind {
            Kind::GitIssue | Kind::GitPullRequest => Some(event.id),
            Kind::GitPatch => Some(match self.first_e_id(event) {
                Some(parent) => self.resolve_thread_root(parent),
                None => event.id,
            }),
            Kind::Comment => match nip22::extract_root(event) {
                Some(CommentTarget::Event { id, .. }) => Some(self.resolve_thread_root(id)),
                _ => None,
            },
            Kind::GitPullRequestUpdate => self
                .first_uppercase_e_id(event)
                .map(|root| self.resolve_thread_root(root)),
            Kind::GitStatusOpen
            | Kind::GitStatusApplied
            | Kind::GitStatusClosed
            | Kind::GitStatusDraft => self
                .nip10_root_id(event)
                .map(|root| self.resolve_thread_root(root)),
            _ => None,
        }
    }

    /// Follows parent pointers until a git issue or pull request root is reached.
    pub fn resolve_thread_root(&self, id: EventId) -> EventId {
        let mut seen = HashSet::new();
        let mut root = id;

        loop {
            if !seen.insert(root) {
                return id;
            }

            let Some(event) = (self.lookup)(root) else {
                return root;
            };

            if matches!(event.kind, Kind::GitIssue | Kind::GitPullRequest) {
                return root;
            }

            match self.parent_id(&event) {
                Some(parent) => root = parent,
                None => return root,
            }
        }
    }

    /// Mirrors gitworkshop's `getParentId` parent resolution.
    fn parent_id(&self, event: &Event) -> Option<EventId> {
        for marker in ["reply", "root"] {
            if let Some(id) = event
                .tags
                .iter()
                .find_map(|tag| self.e_tag_with_marker(tag, marker))
            {
                return Some(id);
            }
        }

        if let Some(id) = event.tags.iter().find_map(|tag| {
            if tag.kind() != "e" {
                return None;
            }

            let slice = tag.as_slice();
            let is_mention = slice.len() == 4 && slice[3] == "mention";

            if is_mention {
                return None;
            }

            tag.content()
                .and_then(|content| EventId::from_hex(content).ok())
        }) {
            return Some(id);
        }

        self.first_uppercase_e_id(event)
    }

    /// Reads the NIP-10 `root` marker, falling back to the first `e` tag.
    fn nip10_root_id(&self, event: &Event) -> Option<EventId> {
        event
            .tags
            .iter()
            .find_map(|tag| self.e_tag_with_marker(tag, "root"))
            .or_else(|| self.first_e_id(event))
    }

    /// Reads the first lowercase `e` tag id.
    fn first_e_id(&self, event: &Event) -> Option<EventId> {
        self.first_tag_id(event, "e")
    }

    /// Reads the first uppercase `E` tag id.
    fn first_uppercase_e_id(&self, event: &Event) -> Option<EventId> {
        self.first_tag_id(event, "E")
    }

    /// Reads the first event id from tags of the given kind.
    fn first_tag_id(&self, event: &Event, name: &str) -> Option<EventId> {
        event.tags.iter().find_map(|tag| {
            if tag.kind() != name {
                return None;
            }
            tag.content()
                .and_then(|content| EventId::from_hex(content).ok())
        })
    }

    /// Parses a four-value `e` tag carrying the given marker.
    fn e_tag_with_marker(&self, tag: &Tag, marker: &str) -> Option<EventId> {
        let slice = tag.as_slice();
        if tag.kind() != "e" || slice.len() != 4 || slice[3] != marker {
            return None;
        }
        tag.content()
            .and_then(|content| EventId::from_hex(content).ok())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Derives deterministic signer keys from a seed byte.
    fn keys(seed: u8) -> Keys {
        let mut hex = "00000000000000000000000000000000000000000000000000000000000000".to_string();
        hex.push_str(&format!("{seed:02x}"));
        Keys::new(SecretKey::from_hex(&hex).expect("valid secret key"))
    }

    /// Builds a signed event with a fixed timestamp.
    fn signed(author: &Keys, kind: Kind, tags: Vec<Tag>, created_at: u64) -> Event {
        EventBuilder::new(kind, "")
            .tags(tags)
            .custom_created_at(Timestamp::from_secs(created_at))
            .finalize(author)
            .expect("signed event")
    }

    /// Builds a plain lowercase `e` tag for the event.
    fn e_tag(event: &Event) -> Tag {
        Tag::parse(["e", &event.id.to_hex()]).expect("valid e tag")
    }

    /// Builds a four-value `e` tag with the given marker.
    fn marked_e_tag(event: &Event, marker: &str) -> Tag {
        Tag::parse(["e", &event.id.to_hex(), "wss://relay.example.com", marker])
            .expect("valid e tag")
    }

    /// Builds an uppercase `E` tag for the event.
    fn uppercase_e_tag(event: &Event) -> Tag {
        Tag::parse(["E", &event.id.to_hex()]).expect("valid E tag")
    }

    /// Builds an event lookup closure over a slice.
    fn lookup(events: &[Event]) -> impl Fn(EventId) -> Option<Event> + '_ {
        move |id| events.iter().find(|event| event.id == id).cloned()
    }

    /// Builds a git issue event.
    fn issue(author: &Keys, at: u64) -> Event {
        signed(author, Kind::GitIssue, Vec::new(), at)
    }

    /// Resolves a comment to its uppercase `E` root.
    #[test]
    fn comment_resolves_to_its_uppercase_root() {
        let issue = issue(&keys(1), 100);
        let comment = signed(
            &keys(2),
            Kind::Comment,
            vec![
                uppercase_e_tag(&issue),
                Tag::parse(["K", "1621"]).expect("valid K tag"),
            ],
            200,
        );
        let events = [issue.clone(), comment.clone()];
        assert_eq!(
            ThreadResolver::new(&lookup(&events)).notification_root(&comment),
            Some(issue.id)
        );
    }

    /// Resolves a child patch to its root patch.
    #[test]
    fn child_patch_resolves_to_the_root_patch() {
        let root_patch = signed(&keys(1), Kind::GitPatch, Vec::new(), 100);
        let child_patch = signed(&keys(1), Kind::GitPatch, vec![e_tag(&root_patch)], 200);
        let events = [root_patch.clone(), child_patch.clone()];
        assert_eq!(
            ThreadResolver::new(&lookup(&events)).notification_root(&child_patch),
            Some(root_patch.id)
        );
    }

    /// Resolves a status through its `root` marker tag.
    #[test]
    fn status_resolves_via_the_root_marker() {
        let issue = issue(&keys(1), 100);
        let status = signed(
            &keys(2),
            Kind::GitStatusClosed,
            vec![marked_e_tag(&issue, "root")],
            200,
        );
        let events = [issue.clone(), status.clone()];
        assert_eq!(
            ThreadResolver::new(&lookup(&events)).notification_root(&status),
            Some(issue.id)
        );
    }

    /// Follows a nested comment chain to the issue root.
    #[test]
    fn nested_comment_chain_follows_to_the_root() {
        let issue = issue(&keys(1), 100);
        let reply = signed(&keys(2), Kind::Comment, vec![uppercase_e_tag(&issue)], 200);
        let nested = signed(&keys(3), Kind::Comment, vec![uppercase_e_tag(&reply)], 300);
        let events = [issue.clone(), reply, nested.clone()];
        assert_eq!(
            ThreadResolver::new(&lookup(&events)).notification_root(&nested),
            Some(issue.id)
        );
    }
}

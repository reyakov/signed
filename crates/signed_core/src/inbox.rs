use std::collections::{HashMap, HashSet};
use std::time::Duration;

use nostr::prelude::*;
use serde::{Deserialize, Serialize};

use crate::{GitEvent, RepoAddr};

const ADVANCE_WINDOW: Duration = Duration::from_secs(3 * 24 * 60 * 60);
const MARK_ALL_WINDOW: Duration = Duration::from_secs(10 * 24 * 60 * 60);

#[derive(Debug, Clone)]
pub struct InboxItem {
    pub root: EventId,
    pub root_event: Option<Event>,
    pub address: Option<RepoAddr>,
    pub events: Vec<Event>,
    pub own_events: Vec<Event>,
    pub unread_ids: Vec<EventId>,
    pub archived: bool,
}

impl InboxItem {
    pub fn title(&self) -> String {
        self.root_event
            .as_ref()
            .or_else(|| self.own_events.first())
            .or_else(|| self.events.first())
            .map(|event| event.activity_subject())
            .unwrap_or_else(|| "Untitled".to_string())
    }

    pub fn kind(&self) -> Option<Kind> {
        self.root_event
            .as_ref()
            .or_else(|| self.own_events.first())
            .or_else(|| self.events.first())
            .map(|event| event.kind)
    }

    pub fn latest_activity(&self) -> Timestamp {
        self.root_event
            .as_ref()
            .into_iter()
            .chain(self.own_events.first())
            .chain(self.events.first())
            .map(|event| event.created_at)
            .max()
            .unwrap_or_default()
    }

    pub fn timeline(&self, limit: usize) -> Vec<Event> {
        let mut seen: HashSet<EventId> = HashSet::new();
        let mut events: Vec<Event> = Vec::new();

        if let Some(root) = &self.root_event {
            seen.insert(root.id);
            events.push(root.clone());
        }

        let mut rest: Vec<Event> = self
            .own_events
            .iter()
            .chain(self.events.iter())
            .filter(|event| seen.insert(event.id))
            .cloned()
            .collect();

        rest.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| b.id.to_hex().cmp(&a.id.to_hex()))
        });
        rest.truncate(limit.saturating_sub(events.len()));
        events.extend(rest);

        events.sort_by_key(|event| event.created_at);
        events
    }

    pub fn is_unread(&self) -> bool {
        !self.archived && !self.unread_ids.is_empty()
    }

    pub fn apply_state(&mut self, state: &InboxReadState) {
        self.unread_ids = self
            .events
            .iter()
            .rev()
            .filter(|event| !state.is_read(event))
            .map(|event| event.id)
            .collect();

        // A thread without notification events is never archived.
        self.archived =
            !self.events.is_empty() && self.events.iter().all(|event| state.is_archived(event));
    }
}

pub struct ThreadResolver<'a, L: ?Sized> {
    lookup: &'a L,
}

impl<'a, L> ThreadResolver<'a, L>
where
    L: Fn(EventId) -> Option<Event> + ?Sized,
{
    pub fn new(lookup: &'a L) -> Self {
        Self { lookup }
    }

    // Kind → root mapping:
    // - issue (1621) / PR (1618): itself
    // - patch (1617): its `e` parent patch, else itself
    // - NIP-22 comment (1111): uppercase `E` root pointer
    // - PR update (1619): uppercase `E`
    // - statuses (1630-1633): NIP-10 root `e`
    // Returns `None` when the event is not git-related, or when its root is a
    // coordinate rather than an event.
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

    // Follow NIP-10/NIP-22 parent pointers until a root item is reached.
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

    // Mirrors gitworkshop's `getParentId`.
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

    fn nip10_root_id(&self, event: &Event) -> Option<EventId> {
        event
            .tags
            .iter()
            .find_map(|tag| self.e_tag_with_marker(tag, "root"))
            .or_else(|| self.first_e_id(event))
    }

    fn first_e_id(&self, event: &Event) -> Option<EventId> {
        self.first_tag_id(event, "e")
    }

    fn first_uppercase_e_id(&self, event: &Event) -> Option<EventId> {
        self.first_tag_id(event, "E")
    }

    fn first_tag_id(&self, event: &Event, name: &str) -> Option<EventId> {
        event.tags.iter().find_map(|tag| {
            if tag.kind() != name {
                return None;
            }
            tag.content()
                .and_then(|content| EventId::from_hex(content).ok())
        })
    }

    fn e_tag_with_marker(&self, tag: &Tag, marker: &str) -> Option<EventId> {
        let slice = tag.as_slice();
        if tag.kind() != "e" || slice.len() != 4 || slice[3] != marker {
            return None;
        }
        tag.content()
            .and_then(|content| EventId::from_hex(content).ok())
    }
}

pub fn group<E, O, L>(
    events: E,
    own: O,
    me: PublicKey,
    state: &InboxReadState,
    lookup: &L,
) -> Vec<InboxItem>
where
    E: IntoIterator<Item = Event>,
    O: IntoIterator<Item = Event>,
    L: Fn(EventId) -> Option<Event>,
{
    let resolver = ThreadResolver::new(lookup);
    let mut groups: HashMap<EventId, Vec<Event>> = HashMap::new();
    for event in events {
        if event.pubkey == me {
            continue;
        }
        let Some(root) = resolver.notification_root(&event) else {
            continue;
        };
        groups.entry(root).or_default().push(event);
    }

    let mut own_groups: HashMap<EventId, Vec<Event>> = HashMap::new();
    for event in own {
        let root = resolver.notification_root(&event).unwrap_or(event.id);
        own_groups.entry(root).or_default().push(event);
    }

    let mut roots: Vec<EventId> = groups.keys().chain(own_groups.keys()).copied().collect();
    roots.sort();
    roots.dedup();

    let mut items: Vec<InboxItem> = roots
        .into_iter()
        .map(|root| {
            let mut events = groups.remove(&root).unwrap_or_default();
            let mut own_events = own_groups.remove(&root).unwrap_or_default();
            utils::sort_newest_first(&mut events);
            utils::sort_newest_first(&mut own_events);

            let root_event = lookup(root);

            let mut item = InboxItem {
                root,
                address: root_event
                    .as_ref()
                    .and_then(|event| event.tags.coordinates().next())
                    .map(RepoAddr::from),
                root_event,
                events,
                own_events,
                unread_ids: Vec::new(),
                archived: false,
            };
            item.apply_state(state);
            item
        })
        .collect();

    items.sort_by(|a, b| {
        b.latest_activity()
            .cmp(&a.latest_activity())
            .then_with(|| b.root.to_hex().cmp(&a.root.to_hex()))
    });

    items
}

// High-water-mark model: events at or before the cutoff
// are covered without an entry in the id set.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboxReadState {
    #[serde(default)]
    pub read_before: Timestamp,
    #[serde(default)]
    pub read_ids: HashSet<EventId>,
    #[serde(default)]
    pub archived_before: Timestamp,
    #[serde(default)]
    pub archived_ids: HashSet<EventId>,
}

impl InboxReadState {
    pub fn is_read(&self, event: &Event) -> bool {
        event.created_at <= self.read_before || self.read_ids.contains(&event.id)
    }

    pub fn is_archived(&self, event: &Event) -> bool {
        event.created_at <= self.archived_before || self.archived_ids.contains(&event.id)
    }

    pub fn mark_read(&mut self, event: &Event) {
        if event.created_at > self.read_before {
            self.read_ids.insert(event.id);
        }
    }

    pub fn mark_archived(&mut self, event: &Event) {
        if event.created_at > self.archived_before {
            self.archived_ids.insert(event.id);
        }
    }

    pub fn mark_all_read(&mut self, all: &[Event], me: PublicKey, now: Timestamp) {
        let cutoff = now - MARK_ALL_WINDOW;
        self.read_before = cutoff;
        self.read_ids = all
            .iter()
            .filter(|event| event.pubkey != me && event.created_at > cutoff)
            .map(|event| event.id)
            .collect();
    }

    // Advance the cutoff to the newest point that keeps unread events unread,
    // then prune the id set.
    pub fn advance_read(&mut self, all: &[Event], me: PublicKey, now: Timestamp) {
        let cutoff = advance_cutoff(all, me, now, self.read_before, |event| self.is_read(event));
        self.read_before = cutoff;
        prune_ids(&mut self.read_ids, all, cutoff);
    }

    pub fn advance_archived(&mut self, all: &[Event], me: PublicKey, now: Timestamp) {
        let cutoff = advance_cutoff(all, me, now, self.archived_before, |event| {
            self.is_archived(event)
        });
        self.archived_before = cutoff;
        prune_ids(&mut self.archived_ids, all, cutoff);
    }
}

fn advance_cutoff<M>(
    all: &[Event],
    me: PublicKey,
    now: Timestamp,
    current: Timestamp,
    is_marked: M,
) -> Timestamp
where
    M: Fn(&Event) -> bool,
{
    let fallback = now - ADVANCE_WINDOW;

    let oldest = all
        .iter()
        .filter(|event| event.pubkey != me && !is_marked(event))
        .map(|event| event.created_at)
        .min();

    let candidate = match oldest {
        Some(at) if at < fallback => at - 1,
        _ => fallback,
    };

    candidate.max(current)
}

fn prune_ids(ids: &mut HashSet<EventId>, all: &[Event], cutoff: Timestamp) {
    let created_at: HashMap<EventId, Timestamp> = all
        .iter()
        .map(|event| (event.id, event.created_at))
        .collect();
    ids.retain(|id| created_at.get(id).is_some_and(|at| *at >= cutoff));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(seed: u8) -> Keys {
        let mut hex = "00000000000000000000000000000000000000000000000000000000000000".to_string();
        hex.push_str(&format!("{seed:02x}"));
        Keys::new(SecretKey::from_hex(&hex).expect("valid secret key"))
    }

    fn signed(author: &Keys, kind: Kind, tags: Vec<Tag>, created_at: u64) -> Event {
        EventBuilder::new(kind, "")
            .tags(tags)
            .custom_created_at(Timestamp::from_secs(created_at))
            .finalize(author)
            .expect("signed event")
    }

    fn e_tag(event: &Event) -> Tag {
        Tag::parse(["e", &event.id.to_hex()]).expect("valid e tag")
    }

    fn marked_e_tag(event: &Event, marker: &str) -> Tag {
        Tag::parse(["e", &event.id.to_hex(), "wss://relay.example.com", marker])
            .expect("valid e tag")
    }

    fn uppercase_e_tag(event: &Event) -> Tag {
        Tag::parse(["E", &event.id.to_hex()]).expect("valid E tag")
    }

    fn lookup(events: &[Event]) -> impl Fn(EventId) -> Option<Event> + '_ {
        move |id| events.iter().find(|event| event.id == id).cloned()
    }

    fn issue(author: &Keys, at: u64) -> Event {
        signed(author, Kind::GitIssue, Vec::new(), at)
    }

    fn titled_issue(author: &Keys, title: &str, at: u64) -> Event {
        signed(
            author,
            Kind::GitIssue,
            vec![Tag::parse(["subject", title]).expect("valid subject tag")],
            at,
        )
    }

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

    #[test]
    fn group_merges_own_events_into_the_matching_thread() {
        let me = keys(1);
        let issue = titled_issue(&me, "Add retry logic", 100);
        let mine = signed(
            &me,
            Kind::Comment,
            vec![
                uppercase_e_tag(&issue),
                Tag::parse(["K", "1621"]).expect("K tag"),
            ],
            150,
        );
        let reply = signed(
            &keys(2),
            Kind::Comment,
            vec![
                uppercase_e_tag(&issue),
                Tag::parse(["K", "1621"]).expect("K tag"),
            ],
            200,
        );

        let context = [issue.clone(), mine.clone(), reply.clone()];
        let items = group(
            [reply.clone()],
            [issue.clone(), mine.clone()],
            me.public_key(),
            &InboxReadState::default(),
            &lookup(&context),
        );

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].root, issue.id);
        assert_eq!(
            items[0].root_event.as_ref().map(|event| event.id),
            Some(issue.id)
        );
        assert_eq!(items[0].kind(), Some(Kind::GitIssue));
        assert_eq!(items[0].title(), "Add retry logic");
        assert_eq!(items[0].events, vec![reply.clone()]);
        assert_eq!(items[0].own_events, vec![mine.clone(), issue.clone()]);
        assert_eq!(
            items[0]
                .timeline(5)
                .iter()
                .map(|event| event.id)
                .collect::<Vec<_>>(),
            vec![issue.id, mine.id, reply.id]
        );
    }
}

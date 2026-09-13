use std::collections::{HashMap, HashSet};
use std::time::Duration;

use nostr::prelude::*;
use serde::{Deserialize, Serialize};

use crate::{COVER_NOTE_KIND, RepoAddr, activity_subject};

/// Window before `now` that an advanced cutoff retreats to.
const ADVANCE_WINDOW: Duration = Duration::from_secs(3 * 24 * 60 * 60);

/// Window before `now` that a mark-all cutoff retreats to.
const MARK_ALL_WINDOW: Duration = Duration::from_secs(10 * 24 * 60 * 60);

/// A thread of notification and own-activity events sharing one root.
#[derive(Debug, Clone)]
pub struct InboxItem {
    /// The root issue, patch or pull request the events belong to.
    pub root: EventId,
    /// The root event itself, when it is known locally.
    pub root_event: Option<Event>,
    /// Kind of the root event, when it is known locally.
    pub root_kind: Option<Kind>,
    /// Repository the root belongs to, from the root's `a` tag.
    pub address: Option<RepoAddr>,
    /// Notification events directed at the user, newest first.
    pub events: Vec<Event>,
    /// The user's own events in the thread, newest first.
    pub own_events: Vec<Event>,
    /// Unread event ids, oldest first.
    pub unread_ids: Vec<EventId>,
    /// Whether every notification event in the thread is archived.
    pub archived: bool,
}

impl InboxItem {
    /// Title of the thread, read from its root issue/patch/PR when known.
    pub fn title(&self) -> String {
        self.root_event
            .as_ref()
            .or_else(|| self.own_events.first())
            .or_else(|| self.events.first())
            .map(activity_subject)
            .unwrap_or_else(|| "Untitled".to_string())
    }

    pub fn kind(&self) -> Option<Kind> {
        self.root_kind.or_else(|| {
            self.root_event
                .as_ref()
                .or_else(|| self.own_events.first())
                .or_else(|| self.events.first())
                .map(|event| event.kind)
        })
    }

    /// Timestamp of the newest event in the thread.
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

    /// Up to `limit` events of the thread, oldest first.
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

    /// Whether the thread has an unread event still visible in the inbox.
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

/// Root issue, patch or pull request of a notification event.
///
/// Returns `None` when the event is not git-related, or when its root is a
/// coordinate rather than an event.
///
/// - issue (1621) / PR (1618): itself
/// - patch (1617): its `e` parent patch, else itself
/// - NIP-22 comment (1111): uppercase `E` root pointer
/// - PR update (1619): uppercase `E`
/// - statuses (1630-1633) / cover note (1624): NIP-10 root `e`
pub fn notification_root<L>(event: &Event, lookup: &L) -> Option<EventId>
where
    L: Fn(EventId) -> Option<Event>,
{
    if event.kind == COVER_NOTE_KIND {
        return nip10_root_id(event).map(|root| resolve_thread_root(root, lookup));
    }
    match event.kind {
        Kind::GitIssue | Kind::GitPullRequest => Some(event.id),
        Kind::GitPatch => Some(match first_e_id(event) {
            Some(parent) => resolve_thread_root(parent, lookup),
            None => event.id,
        }),
        Kind::Comment => match nip22::extract_root(event) {
            Some(CommentTarget::Event { id, .. }) => Some(resolve_thread_root(id, lookup)),
            _ => None,
        },
        Kind::GitPullRequestUpdate => {
            first_uppercase_e_id(event).map(|root| resolve_thread_root(root, lookup))
        }
        Kind::GitStatusOpen
        | Kind::GitStatusApplied
        | Kind::GitStatusClosed
        | Kind::GitStatusDraft => {
            nip10_root_id(event).map(|root| resolve_thread_root(root, lookup))
        }
        _ => None,
    }
}

/// Group notification events and the user's own events into one item per thread.
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
    let mut groups: HashMap<EventId, Vec<Event>> = HashMap::new();
    for event in events {
        if event.pubkey == me {
            continue;
        }
        let Some(root) = notification_root(&event, lookup) else {
            continue;
        };
        groups.entry(root).or_default().push(event);
    }

    let mut own_groups: HashMap<EventId, Vec<Event>> = HashMap::new();
    for event in own {
        let root = notification_root(&event, lookup).unwrap_or(event.id);
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
            sort_newest_first(&mut events);
            sort_newest_first(&mut own_events);

            let root_event = lookup(root);

            let mut item = InboxItem {
                root,
                root_kind: root_event.as_ref().map(|event| event.kind),
                address: root_event
                    .as_ref()
                    .and_then(|event| event.tags.coordinates().next()),
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

/// Sort thread events newest first, ties broken by id.
fn sort_newest_first(events: &mut [Event]) {
    events.sort_by(|a, b| {
        b.created_at
            .cmp(&a.created_at)
            .then_with(|| b.id.to_hex().cmp(&a.id.to_hex()))
    });
}

/// Read and archive state of the inbox, a high-water-mark model.
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
    /// Whether `event` is at or before the read cutoff, or marked read.
    pub fn is_read(&self, event: &Event) -> bool {
        event.created_at <= self.read_before || self.read_ids.contains(&event.id)
    }

    /// Whether `event` is at or before the archived cutoff, or marked archived.
    pub fn is_archived(&self, event: &Event) -> bool {
        event.created_at <= self.archived_before || self.archived_ids.contains(&event.id)
    }

    /// Mark one event read. Events at or before the cutoff are already read.
    pub fn mark_read(&mut self, event: &Event) {
        if event.created_at > self.read_before {
            self.read_ids.insert(event.id);
        }
    }

    /// Mark one event archived. Events at or before the cutoff are already archived.
    pub fn mark_archived(&mut self, event: &Event) {
        if event.created_at > self.archived_before {
            self.archived_ids.insert(event.id);
        }
    }

    /// Mark every non-self event read, anchoring the cutoff ten days back.
    pub fn mark_all_read(&mut self, all: &[Event], me: PublicKey, now: Timestamp) {
        let cutoff = now - MARK_ALL_WINDOW;
        self.read_before = cutoff;
        self.read_ids = all
            .iter()
            .filter(|event| event.pubkey != me && event.created_at > cutoff)
            .map(|event| event.id)
            .collect();
    }

    /// Advance the read cutoff to the newest point that keeps unread events
    /// unread, then prune the id set.
    pub fn advance_read(&mut self, all: &[Event], me: PublicKey, now: Timestamp) {
        let cutoff = advance_cutoff(all, me, now, self.read_before, |event| self.is_read(event));
        self.read_before = cutoff;
        prune_ids(&mut self.read_ids, all, cutoff);
    }

    /// Advance the archived cutoff, mirroring [`Self::advance_read`].
    pub fn advance_archived(&mut self, all: &[Event], me: PublicKey, now: Timestamp) {
        let cutoff = advance_cutoff(all, me, now, self.archived_before, |event| {
            self.is_archived(event)
        });
        self.archived_before = cutoff;
        prune_ids(&mut self.archived_ids, all, cutoff);
    }
}

/// Newest cutoff that keeps unread events unread, never earlier than `current`.
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

/// Drop ids whose event is unknown or now covered by the cutoff.
fn prune_ids(ids: &mut HashSet<EventId>, all: &[Event], cutoff: Timestamp) {
    let created_at: HashMap<EventId, Timestamp> = all
        .iter()
        .map(|event| (event.id, event.created_at))
        .collect();
    ids.retain(|id| created_at.get(id).is_some_and(|at| *at >= cutoff));
}

/// Follow NIP-10/NIP-22 parent pointers until a root item is reached.
fn resolve_thread_root(id: EventId, lookup: &impl Fn(EventId) -> Option<Event>) -> EventId {
    let mut seen = HashSet::new();
    let mut root = id;

    loop {
        if !seen.insert(root) {
            return id;
        }

        let Some(event) = lookup(root) else {
            return root;
        };

        if matches!(event.kind, Kind::GitIssue | Kind::GitPullRequest) {
            return root;
        }

        match parent_id(&event) {
            Some(parent) => root = parent,
            None => return root,
        }
    }
}

/// Parent of a thread event, mirroring gitworkshop's `getParentId`.
fn parent_id(event: &Event) -> Option<EventId> {
    for marker in ["reply", "root"] {
        if let Some(id) = event
            .tags
            .iter()
            .find_map(|tag| e_tag_with_marker(tag, marker))
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

    first_uppercase_e_id(event)
}

/// NIP-10 root of an event: the `e` tag marked `root`, else the first `e` tag.
fn nip10_root_id(event: &Event) -> Option<EventId> {
    event
        .tags
        .iter()
        .find_map(|tag| e_tag_with_marker(tag, "root"))
        .or_else(|| first_e_id(event))
}

/// First `e` tag id, in document order.
fn first_e_id(event: &Event) -> Option<EventId> {
    first_tag_id(event, "e")
}

/// First uppercase `E` tag id, in document order.
fn first_uppercase_e_id(event: &Event) -> Option<EventId> {
    first_tag_id(event, "E")
}

fn first_tag_id(event: &Event, name: &str) -> Option<EventId> {
    event.tags.iter().find_map(|tag| {
        if tag.kind() != name {
            return None;
        }
        tag.content()
            .and_then(|content| EventId::from_hex(content).ok())
    })
}

/// Event id from a four-element `e` tag carrying `marker`.
fn e_tag_with_marker(tag: &Tag, marker: &str) -> Option<EventId> {
    let slice = tag.as_slice();
    if tag.kind() != "e" || slice.len() != 4 || slice[3] != marker {
        return None;
    }
    tag.content()
        .and_then(|content| EventId::from_hex(content).ok())
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
            notification_root(&comment, &lookup(&events)),
            Some(issue.id)
        );
    }

    #[test]
    fn child_patch_resolves_to_the_root_patch() {
        let root_patch = signed(&keys(1), Kind::GitPatch, Vec::new(), 100);
        let child_patch = signed(&keys(1), Kind::GitPatch, vec![e_tag(&root_patch)], 200);
        let events = [root_patch.clone(), child_patch.clone()];
        assert_eq!(
            notification_root(&child_patch, &lookup(&events)),
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
        assert_eq!(notification_root(&status, &lookup(&events)), Some(issue.id));
    }

    #[test]
    fn nested_comment_chain_follows_to_the_root() {
        let issue = issue(&keys(1), 100);
        let reply = signed(&keys(2), Kind::Comment, vec![uppercase_e_tag(&issue)], 200);
        let nested = signed(&keys(3), Kind::Comment, vec![uppercase_e_tag(&reply)], 300);
        let events = [issue.clone(), reply, nested.clone()];
        assert_eq!(notification_root(&nested, &lookup(&events)), Some(issue.id));
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
        // The own events are kept apart from the notifications, newest first.
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

    #[test]
    fn mark_all_read_marks_known_recent_events() {
        let me = keys(1);
        let now = Timestamp::from_secs(1_000_000_000);
        let recent = issue(&keys(2), now.as_secs() - 1000);
        let old = issue(&keys(2), now.as_secs() - 5 * 24 * 60 * 60);
        let ancient = issue(&keys(2), now.as_secs() - 20 * 24 * 60 * 60);
        let mine = issue(&keys(1), now.as_secs() - 100);

        let mut state = InboxReadState::default();
        state.mark_all_read(
            &[recent.clone(), old.clone(), ancient.clone(), mine.clone()],
            me.public_key(),
            now,
        );

        assert_eq!(state.read_before, now - MARK_ALL_WINDOW);
        assert_eq!(state.read_ids, HashSet::from([recent.id, old.id]));
        assert!(state.is_read(&recent));
        assert!(state.is_read(&ancient));
        assert!(!state.is_read(&mine));
    }

    #[test]
    fn advance_read_never_moves_the_cutoff_backwards() {
        let me = keys(1);
        let unread = issue(&keys(2), 1_000);
        let all = [unread];
        let now = Timestamp::from_secs(1_000_000_000);

        let mut state = InboxReadState {
            read_before: Timestamp::from_secs(999_999_999),
            ..Default::default()
        };
        state.advance_read(&all, me.public_key(), now);

        assert_eq!(state.read_before, Timestamp::from_secs(999_999_999));
    }

    #[test]
    fn advance_read_moves_before_the_oldest_unread_and_prunes_ids() {
        let me = keys(1);
        let now = Timestamp::from_secs(1_000_000_000);
        let five_days = 5 * 24 * 60 * 60;
        let old_unread = issue(&keys(2), now.as_secs() - five_days);
        // Read ids that fall before and after the new cutoff.
        let stale = signed(
            &keys(2),
            Kind::GitIssue,
            Vec::new(),
            now.as_secs() - five_days - 1000,
        );
        let fresh = signed(
            &keys(2),
            Kind::GitIssue,
            Vec::new(),
            now.as_secs() - 100_000,
        );

        let mut state = InboxReadState {
            read_ids: HashSet::from([stale.id, fresh.id]),
            ..Default::default()
        };
        state.advance_read(
            &[old_unread.clone(), stale.clone(), fresh.clone()],
            me.public_key(),
            now,
        );

        assert_eq!(state.read_before, old_unread.created_at - 1);
        assert_eq!(state.read_ids, HashSet::from([fresh.id]));
    }

    #[test]
    fn mark_archived_skips_events_at_or_before_the_cutoff() {
        let now = Timestamp::from_secs(1_000_000_000);
        let event = issue(&keys(2), now.as_secs() - 1000);

        let mut state = InboxReadState {
            archived_before: now,
            ..Default::default()
        };
        state.mark_archived(&event);
        assert!(state.archived_ids.is_empty());

        let mut state = InboxReadState::default();
        state.mark_archived(&event);
        assert_eq!(state.archived_ids, HashSet::from([event.id]));
    }
}

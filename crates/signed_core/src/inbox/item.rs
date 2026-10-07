use std::collections::HashSet;

use nostr::prelude::*;

use super::InboxReadState;
use crate::{GitEvent, RepoAddr};

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
    /// Returns the thread title from the root, own events, or events.
    pub fn title(&self) -> String {
        self.root_event
            .as_ref()
            .or_else(|| self.own_events.first())
            .or_else(|| self.events.first())
            .map(|event| event.activity_subject())
            .unwrap_or_else(|| "Untitled".to_string())
    }

    /// Returns the kind of the thread's primary event.
    pub fn kind(&self) -> Option<Kind> {
        self.root_event
            .as_ref()
            .or_else(|| self.own_events.first())
            .or_else(|| self.events.first())
            .map(|event| event.kind)
    }

    /// Returns the newest timestamp among the root, own, and other events.
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

    /// Builds a chronological timeline capped at `limit` events, root first.
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

    /// Returns whether the thread has unread events and is not archived.
    pub fn is_unread(&self) -> bool {
        !self.archived && !self.unread_ids.is_empty()
    }

    /// Recomputes unread ids and the archived flag from the read state.
    pub fn apply_state(&mut self, state: &InboxReadState) {
        self.unread_ids = self
            .events
            .iter()
            .rev()
            .filter(|event| !state.is_read(event))
            .map(|event| event.id)
            .collect();

        self.archived =
            !self.events.is_empty() && self.events.iter().all(|event| state.is_archived(event));
    }
}

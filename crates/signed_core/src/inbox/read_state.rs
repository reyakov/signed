use std::collections::{HashMap, HashSet};
use std::time::Duration;

use nostr::prelude::*;
use serde::{Deserialize, Serialize};

const ADVANCE_WINDOW: Duration = Duration::from_secs(3 * 24 * 60 * 60);
const MARK_ALL_WINDOW: Duration = Duration::from_secs(10 * 24 * 60 * 60);

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
    /// Returns whether the event falls under the read high-water mark or id set.
    pub fn is_read(&self, event: &Event) -> bool {
        event.created_at <= self.read_before || self.read_ids.contains(&event.id)
    }

    /// Returns whether the event falls under the archived high-water mark or id set.
    pub fn is_archived(&self, event: &Event) -> bool {
        event.created_at <= self.archived_before || self.archived_ids.contains(&event.id)
    }

    /// Records the event as read when it is past the current cutoff.
    pub fn mark_read(&mut self, event: &Event) {
        if event.created_at > self.read_before {
            self.read_ids.insert(event.id);
        }
    }

    /// Records the event as archived when it is past the current cutoff.
    pub fn mark_archived(&mut self, event: &Event) {
        if event.created_at > self.archived_before {
            self.archived_ids.insert(event.id);
        }
    }

    /// Marks all events from other authors within the window as read.
    pub fn mark_all_read(&mut self, all: &[Event], me: PublicKey, now: Timestamp) {
        let cutoff = now - MARK_ALL_WINDOW;
        self.read_before = cutoff;
        self.read_ids = all
            .iter()
            .filter(|event| event.pubkey != me && event.created_at > cutoff)
            .map(|event| event.id)
            .collect();
    }

    /// Advances the read cutoff as far as possible, then prunes the id set.
    pub fn advance_read(&mut self, all: &[Event], me: PublicKey, now: Timestamp) {
        let cutoff = advance_cutoff(all, me, now, self.read_before, |event| self.is_read(event));
        self.read_before = cutoff;
        prune_ids(&mut self.read_ids, all, cutoff);
    }

    /// Advances the archived cutoff as far as possible, then prunes the id set.
    pub fn advance_archived(&mut self, all: &[Event], me: PublicKey, now: Timestamp) {
        let cutoff = advance_cutoff(all, me, now, self.archived_before, |event| {
            self.is_archived(event)
        });
        self.archived_before = cutoff;
        prune_ids(&mut self.archived_ids, all, cutoff);
    }
}

/// Computes the newest cutoff that leaves unmarked events uncovered.
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

/// Drops ids whose events predate the cutoff.
fn prune_ids(ids: &mut HashSet<EventId>, all: &[Event], cutoff: Timestamp) {
    let created_at: HashMap<EventId, Timestamp> = all
        .iter()
        .map(|event| (event.id, event.created_at))
        .collect();
    ids.retain(|id| created_at.get(id).is_some_and(|at| *at >= cutoff));
}

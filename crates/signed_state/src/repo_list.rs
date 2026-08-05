use std::collections::HashMap;

use anyhow::Error;
use gpui::{Context, Subscription, Task};
use nostr_sdk::prelude::*;
use signed_core::{Announcement, filters};

use crate::backend::{Backend, BackendEvent};

/// Store listing repository announcements (global discovery or per-author).
pub struct RepoListStore {
    pub announcements: Vec<Announcement>,
    author: Option<PublicKey>,
    refreshing: bool,
    refresh_dirty: bool,
    tasks: Vec<Task<Result<(), Error>>>,
    _subscription: Subscription,
}

impl RepoListStore {
    /// Create a store. If `author` is `None`, all announcements are listed.
    pub fn new(author: Option<PublicKey>, cx: &mut Context<Self>) -> Self {
        let subscription = cx.subscribe(&Backend::global(cx), |this, _backend, event, cx| {
            let relevant = match event {
                BackendEvent::NostrUpdate(update) => {
                    update.kind == Kind::GitRepoAnnouncement
                        && this.author.is_none_or(|a| a == update.author)
                }
                BackendEvent::Published(event) => {
                    event.kind == Kind::GitRepoAnnouncement
                        && this.author.is_none_or(|a| a == event.pubkey)
                }
                _ => false,
            };

            if relevant {
                this.refresh(cx);
            }
        });

        let mut store = Self {
            announcements: Vec::new(),
            author,
            refreshing: false,
            refresh_dirty: false,
            _subscription: subscription,
            tasks: Vec::new(),
        };

        store.subscribe_remote(cx);
        store.refresh(cx);
        store
    }

    /// Scope the list to an author (or clear the scope with `None`).
    pub fn set_author(&mut self, author: Option<PublicKey>, cx: &mut Context<Self>) {
        self.author = author;
        self.subscribe_remote(cx);
        self.refresh(cx);
    }

    fn subscribe_remote(&mut self, cx: &mut Context<Self>) {
        let author = self.author;

        Backend::global(cx).update(cx, |backend, cx| {
            let filter = match author {
                Some(a) => filters::announcements_by(a),
                None => filters::all_announcements(500),
            };
            backend.subscribe(filter, cx);
        });
    }

    /// Re-query the local database. Latest announcement per repository wins.
    ///
    /// Debounced: concurrent requests are coalesced into a single re-query
    /// after the running one finishes.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.refreshing {
            self.refresh_dirty = true;
            return;
        }
        self.refreshing = true;

        let client = Backend::global(cx).read(cx).client();
        let author = self.author;

        let task = cx.spawn(async move |this, cx| {
            loop {
                let filter = match author {
                    Some(a) => filters::announcements_by(a),
                    None => filters::all_announcements(500),
                };

                let events = match client.database().query(filter).await {
                    Ok(events) => events,
                    Err(_) => {
                        return this.update(cx, |this, _cx| {
                            this.refreshing = false;
                        });
                    }
                };

                let again = this.update(cx, |this, cx| {
                    let mut by_repo: HashMap<(String, String), Announcement> = HashMap::new();

                    for event in events {
                        let Some(announcement) = Announcement::from_event(&event) else {
                            continue;
                        };

                        let key = (announcement.owner.to_hex(), announcement.id.clone());

                        match by_repo.get(&key) {
                            Some(existing) if existing.created_at >= announcement.created_at => {}
                            _ => {
                                by_repo.insert(key, announcement);
                            }
                        }
                    }

                    let mut announcements: Vec<Announcement> = by_repo.into_values().collect();
                    announcements.sort_by_key(|a| std::cmp::Reverse(a.created_at));

                    this.announcements = announcements;
                    cx.notify();

                    if this.refresh_dirty {
                        this.refresh_dirty = false;
                        true
                    } else {
                        this.refreshing = false;
                        false
                    }
                })?;

                if !again {
                    break;
                }
            }

            Ok(())
        });

        self.tasks.push(task);
    }
}

use std::collections::{HashMap, HashSet};

use anyhow::Error;
use gpui::{AppContext, Context, Task};
use nostr_sdk::prelude::*;
use signed_core::{Deletions, Filters, GitEvent, InboxItem, InboxReadState, inbox};

use crate::backend::Backend;

#[derive(Default)]
pub struct Inbox {
    state: InboxReadState,
    loaded: bool,
}

impl Inbox {
    pub fn state(&self) -> &InboxReadState {
        &self.state
    }

    pub fn is_loaded(&self) -> bool {
        self.loaded
    }

    pub fn mark_all_read(&mut self, all: &[Event], me: PublicKey, cx: &mut Context<Self>) {
        self.state.mark_all_read(all, me, Timestamp::now());
        self.persist(cx);
        cx.notify();
    }

    pub(crate) fn activate(&mut self, me: PublicKey, client: Client, cx: &mut Context<Self>) {
        self.state = InboxReadState::default();
        self.loaded = false;
        cx.notify();

        let backend = Backend::global(cx);
        let work = cx.background_spawn(async move { Self::load_state(&client, me).await });

        cx.spawn(async move |this, cx| {
            let loaded = work.await;

            this.update(cx, |this, cx| {
                if backend.read(cx).current_user() != Some(me) {
                    return;
                }

                match loaded {
                    Ok(Some(state)) => this.state = state,
                    Ok(None) => this.state = InboxReadState::default(),
                    Err(error) => log::warn!("failed to load inbox state: {error}"),
                }

                this.loaded = true;
                cx.notify();
            })?;

            Ok::<(), Error>(())
        })
        .detach();
    }

    pub(crate) fn reset(&mut self, cx: &mut Context<Self>) {
        self.state = InboxReadState::default();
        self.loaded = false;
        cx.notify();
    }

    fn persist(&mut self, cx: &mut Context<Self>) {
        let backend = Backend::global(cx);
        let (me, client) = {
            let backend = backend.read(cx);
            (backend.current_user(), backend.client())
        };

        let Some(me) = me else {
            return;
        };

        let state = self.state.clone();

        let task: Task<Result<(), Error>> = cx.background_spawn(async move {
            if let Err(error) = Self::save_state(&client, me, &state).await {
                log::warn!("failed to save inbox state: {error}");
            }
            Ok(())
        });

        task.detach();
    }

    pub async fn query(
        client: &Client,
        me: PublicKey,
        state: &InboxReadState,
    ) -> Result<(Vec<InboxItem>, usize), Error> {
        let deletion_events = client.database().query(Filters::deletions()).await?;
        let deletions = Deletions::from_events(deletion_events);

        let (notification_events, mut by_id) =
            Self::fetch_notifications(client, me, &deletions).await?;

        let mut activity = Vec::new();
        for event in client
            .database()
            .query(Filters::authored_activity(me))
            .await?
        {
            if deletions.is_deleted(&event) || !event.is_git_activity() {
                continue;
            }
            by_id.entry(event.id).or_insert_with(|| event.clone());
            activity.push(event);
        }

        let items = inbox::group(notification_events, activity, me, state, &|id| {
            by_id.get(&id).cloned()
        });

        let unread_count = items.iter().filter(|item| item.is_unread()).count();

        Ok((items, unread_count))
    }
}

impl Inbox {
    fn inbox_state_d_tag(me: PublicKey) -> String {
        format!("signed-inbox-state:{}", me.to_hex())
    }

    async fn load_state(client: &Client, me: PublicKey) -> Result<Option<InboxReadState>, Error> {
        let filter = Filter::new()
            .kind(Kind::ApplicationSpecificData)
            .identifier(Self::inbox_state_d_tag(me));

        let events = client.database().query(filter).await?;

        let Some(event) = events.into_iter().max_by_key(|event| event.created_at) else {
            return Ok(None);
        };

        match serde_json::from_str(&event.content) {
            Ok(state) => Ok(Some(state)),
            Err(error) => {
                log::warn!("ignoring unreadable inbox state {}: {error}", event.id);
                Ok(None)
            }
        }
    }

    // Signed with a random key: the state is local-only, authorship does not
    // matter and no key material needs to be kept.
    async fn save_state(
        client: &Client,
        me: PublicKey,
        state: &InboxReadState,
    ) -> Result<(), Error> {
        let event = EventBuilder::new(Kind::ApplicationSpecificData, serde_json::to_string(state)?)
            .tags([Tag::identifier(Self::inbox_state_d_tag(me))])
            .finalize(&Keys::generate())?;

        client.database().save_event(&event).await?;

        Ok(())
    }

    async fn fetch_notifications(
        client: &Client,
        me: PublicKey,
        deletions: &Deletions,
    ) -> Result<(Vec<Event>, HashMap<EventId, Event>), Error> {
        let mut notifications: Vec<Event> = Vec::new();
        let mut by_id: HashMap<EventId, Event> = HashMap::new();

        for filter in Filters::notifications(me) {
            for event in client.database().query(filter).await? {
                if deletions.is_deleted(&event) {
                    continue;
                }

                if by_id.insert(event.id, event.clone()).is_none() {
                    notifications.push(event);
                }
            }
        }

        let mut pending: Vec<EventId> = notifications
            .iter()
            .flat_map(Self::event_references)
            .collect();

        let mut seen: HashSet<EventId> = by_id.keys().copied().collect();

        loop {
            pending.retain(|id| seen.insert(*id));

            if pending.is_empty() {
                break;
            }

            let ancestors = client
                .database()
                .query(Filter::new().ids(pending.iter().copied()))
                .await?;

            let mut next = Vec::new();

            for event in ancestors {
                if deletions.is_deleted(&event) {
                    continue;
                }
                next.extend(Self::event_references(&event).filter(|id| !seen.contains(id)));
                by_id.entry(event.id).or_insert(event);
            }

            pending = next;
        }

        Ok((notifications, by_id))
    }

    fn event_references(event: &Event) -> impl Iterator<Item = EventId> + '_ {
        event.tags.iter().filter_map(|tag| {
            if tag.kind() != "e" && tag.kind() != "E" {
                return None;
            }
            tag.content()
                .and_then(|content| EventId::from_hex(content).ok())
        })
    }
}

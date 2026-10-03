use nostr::prelude::*;

pub fn sort_newest_first(events: &mut [Event]) {
    events.sort_by(|a, b| {
        b.created_at
            .cmp(&a.created_at)
            .then_with(|| b.id.to_hex().cmp(&a.id.to_hex()))
    });
}

pub fn sort_oldest_first(events: &mut [Event]) {
    events.sort_by_key(|e| e.created_at);
}

pub fn latest<I>(events: I) -> Option<Event>
where
    I: IntoIterator<Item = Event>,
{
    events.into_iter().max_by_key(|e| e.created_at)
}

pub fn relative_time(timestamp: Timestamp) -> String {
    let now = Timestamp::now().as_secs();
    let secs = now.saturating_sub(timestamp.as_secs());

    if secs < 60 {
        "just now".to_string()
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86_400 {
        format!("{}h ago", secs / 3600)
    } else if secs < 30 * 86_400 {
        format!("{}d ago", secs / 86_400)
    } else if secs < 365 * 86_400 {
        format!("{}mo ago", secs / (30 * 86_400))
    } else {
        format!("{}y ago", secs / (365 * 86_400))
    }
}

pub fn relative_time_secs(secs: i64) -> String {
    relative_time(Timestamp::from_secs(secs.max(0) as u64))
}

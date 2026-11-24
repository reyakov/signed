use gpui::{App, Entity};
use metadata::ProfileDirectory;
use nostr::prelude::PublicKey;

pub mod event_list;
pub mod panels;
pub mod progress;

pub mod widgets;

/// Logs a discarded `Result` with its source location instead of dropping it silently.
#[track_caller]
pub fn log_dropped<T>(result: Result<T, anyhow::Error>) {
    if let Err(error) = result {
        let location = std::panic::Location::caller();
        log::warn!(
            "{}:{}: dropped result: {error}",
            location.file(),
            location.line()
        );
    }
}

/// Tracks a detail view's author, commenters, and owner for profile prefetch.
pub fn track_event_participants(
    author: Option<PublicKey>,
    owner: Option<PublicKey>,
    comments: impl Iterator<Item = PublicKey>,
    directory: &Entity<ProfileDirectory>,
    cx: &mut App,
) {
    let mut keys: Vec<PublicKey> = author.into_iter().collect();
    keys.extend(comments);
    keys.extend(owner);
    directory.update(cx, |directory, cx| directory.track(keys, cx));
}

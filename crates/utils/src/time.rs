use nostr::prelude::*;

/// Format a timestamp as a short relative time (e.g. "3h ago").
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

/// Format a unix timestamp in seconds as a short relative time (e.g. "3h ago").
pub fn relative_time_secs(secs: i64) -> String {
    relative_time(Timestamp::from_secs(secs.max(0) as u64))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_relative_time() {
        let now = Timestamp::now();

        assert_eq!(relative_time(now), "just now");
        assert_eq!(relative_time(now - 300), "5m ago");
        assert_eq!(relative_time(now - 7_200), "2h ago");
        assert_eq!(relative_time(now - 3 * 86_400), "3d ago");
        assert_eq!(relative_time(now - 60 * 86_400), "2mo ago");
        assert_eq!(relative_time(now - 800 * 86_400), "2y ago");
    }

    #[test]
    fn clamps_future_timestamps() {
        assert_eq!(relative_time(Timestamp::now() + 600), "just now");
    }
}

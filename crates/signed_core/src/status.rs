use nostr::prelude::*;

/// Status of a root patch, pull request or issue (kinds `1630..=1633`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RepoStatus {
    Open,
    Applied,
    Closed,
    Draft,
}

impl RepoStatus {
    pub fn from_kind(kind: Kind) -> Option<Self> {
        match kind {
            Kind::GitStatusOpen => Some(Self::Open),
            Kind::GitStatusApplied => Some(Self::Applied),
            Kind::GitStatusClosed => Some(Self::Closed),
            Kind::GitStatusDraft => Some(Self::Draft),
            _ => None,
        }
    }

    pub fn kind(self) -> Kind {
        match self {
            Self::Open => Kind::GitStatusOpen,
            Self::Applied => Kind::GitStatusApplied,
            Self::Closed => Kind::GitStatusClosed,
            Self::Draft => Kind::GitStatusDraft,
        }
    }
}

/// Check whether a status event references the given root event via an `e` tag.
pub fn references_root(event: &Event, root: &EventId) -> bool {
    let root_hex: String = root.to_hex();
    event
        .tags
        .iter()
        .any(|t| t.kind() == "e" && t.content() == Some(root_hex.as_str()))
}

/// Resolve the status of a root event per NIP-34:
/// the most recent status event from the root author or a maintainer wins.
/// Defaults to [`RepoStatus::Open`].
pub fn resolve_status<'a, I>(
    status_events: I,
    root_author: &PublicKey,
    maintainers: &[PublicKey],
) -> RepoStatus
where
    I: IntoIterator<Item = &'a Event>,
{
    status_events
        .into_iter()
        .filter(|e| RepoStatus::from_kind(e.kind).is_some())
        .filter(|e| &e.pubkey == root_author || maintainers.contains(&e.pubkey))
        .max_by_key(|e| e.created_at)
        .and_then(|e| RepoStatus::from_kind(e.kind))
        .unwrap_or(RepoStatus::Open)
}

use nostr::prelude::*;

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

    pub fn resolve<'a, I>(
        status_events: I,
        root_author: &PublicKey,
        maintainers: &[PublicKey],
    ) -> Self
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
}

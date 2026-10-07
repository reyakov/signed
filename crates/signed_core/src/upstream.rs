use nostr::prelude::*;

use crate::RepoAddr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Upstream {
    pub raw: String,
    pub addr: Option<RepoAddr>,
}

impl Upstream {
    /// Parses a raw `u` tag value, extracting the coordinate when present.
    pub(crate) fn parse(raw: &str) -> Self {
        let coordinate = raw.split('|').next().unwrap_or(raw);
        let addr = coordinate
            .parse::<Coordinate>()
            .ok()
            .filter(|coordinate| coordinate.kind == Kind::GitRepoAnnouncement)
            .map(RepoAddr::from);
        Self {
            raw: raw.to_owned(),
            addr,
        }
    }

    /// Shows the coordinate when parsed, otherwise the raw value.
    pub fn display(&self) -> String {
        match &self.addr {
            Some(addr) => addr.to_string(),
            None => self.raw.clone(),
        }
    }
}

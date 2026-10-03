use std::fmt;
use std::str::FromStr;

use nostr::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RepoAddr(Coordinate);

impl RepoAddr {
    pub fn new(owner: PublicKey, identifier: impl Into<String>) -> Self {
        Self(Coordinate::new(Kind::GitRepoAnnouncement, owner).identifier(identifier))
    }

    pub fn identifier_from_name(name: &str) -> String {
        name.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '/' {
                    c
                } else {
                    '-'
                }
            })
            .collect()
    }

    pub fn kind(&self) -> Kind {
        self.0.kind
    }

    pub fn public_key(&self) -> PublicKey {
        self.0.public_key
    }

    pub fn identifier(&self) -> &str {
        &self.0.identifier
    }

    pub fn coordinate(&self) -> &Coordinate {
        &self.0
    }

    pub fn announcement_filter(&self) -> Filter {
        Filter::new()
            .kind(Kind::GitRepoAnnouncement)
            .author(self.public_key())
            .identifier(self.identifier())
    }

    pub fn state_filter(&self) -> Filter {
        Filter::new()
            .kind(Kind::RepoState)
            .author(self.public_key())
            .identifier(self.identifier())
    }

    // Statuses may omit the `a` tag per NIP-34; those are not matched here.
    pub fn activity_filter(&self) -> Filter {
        Filter::new()
            .kinds(crate::filters::ACTIVITY_KINDS)
            .coordinate(&self.0)
    }

    pub fn deletion_filters(&self) -> Vec<Filter> {
        vec![
            Filter::new()
                .kinds([Kind::EventDeletion, Kind::RequestToVanish])
                .author(self.public_key()),
            Filter::new().kind(Kind::EventDeletion).coordinate(&self.0),
        ]
    }
}

impl fmt::Display for RepoAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl FromStr for RepoAddr {
    type Err = <Coordinate as FromStr>::Err;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Coordinate::from_str(s).map(RepoAddr)
    }
}

impl From<Coordinate> for RepoAddr {
    fn from(coordinate: Coordinate) -> Self {
        RepoAddr(coordinate)
    }
}

impl From<RepoAddr> for Coordinate {
    fn from(addr: RepoAddr) -> Self {
        addr.0
    }
}

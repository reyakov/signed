use std::collections::HashSet;

use nostr::prelude::*;

use crate::RepoAddr;
use crate::upstream::Upstream;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Announcement {
    pub event_id: EventId,
    pub id: String,
    pub owner: PublicKey,
    pub created_at: Timestamp,
    pub name: Option<String>,
    pub description: Option<String>,
    pub web: Vec<Url>,
    pub clone: Vec<Url>,
    pub relays: Vec<RelayUrl>,
    pub euc: Option<String>,
    pub maintainers: Vec<PublicKey>,
    pub upstream: Option<Upstream>,
    pub hashtags: Vec<String>,
}

impl Announcement {
    /// Lists forks of the base, the user's own first.
    pub fn forks_in<'a>(
        announcements: &'a [Announcement],
        base: &RepoAddr,
        base_euc: Option<&str>,
        user: Option<PublicKey>,
    ) -> Vec<&'a Announcement> {
        let (mut own, mut others) = (Vec::new(), Vec::new());
        for announcement in announcements {
            if announcement.clone.is_empty() || !announcement.is_fork_of(base, base_euc) {
                continue;
            }
            if Some(announcement.owner) == user {
                own.push(announcement);
            } else {
                others.push(announcement);
            }
        }
        own.into_iter().chain(others).collect()
    }

    /// Parses a kind 30617 event, rejecting other kinds and missing `d` tags.
    pub fn from_event(event: &Event) -> Option<Self> {
        if event.kind != Kind::GitRepoAnnouncement {
            return None;
        }

        let id = event.tags.identifier()?;

        let mut hashtags: Vec<String> = Vec::new();
        hashtags.extend(event.tags.hashtags().map(|t| t.to_string()));

        let mut name: Option<String> = None;
        let mut description: Option<String> = None;
        let mut web: Vec<Url> = Vec::new();
        let mut clone: Vec<Url> = Vec::new();
        let mut relays: Vec<RelayUrl> = Vec::new();
        let mut euc: Option<String> = None;
        let mut maintainers: Vec<PublicKey> = Vec::new();
        let mut upstream: Option<Upstream> = None;

        for tag in event.tags.iter() {
            match Nip34Tag::parse(tag.as_slice()) {
                Ok(Nip34Tag::Name(value)) => name = Some(value),
                Ok(Nip34Tag::Description(value)) => description = Some(value),
                Ok(Nip34Tag::Web(urls)) => web.extend(urls),
                Ok(Nip34Tag::Clone(urls)) => clone.extend(urls),
                Ok(Nip34Tag::Relays(urls)) => relays.extend(urls),
                Ok(Nip34Tag::EarliestUniqueCommitId(commit)) => euc = Some(commit.to_string()),
                Ok(Nip34Tag::Maintainers(keys)) => maintainers.extend(keys),
                _ => {}
            }

            if upstream.is_none() && tag.kind() == "u" {
                let values = tag.as_slice();
                let raw = values.get(1).map(String::as_str).unwrap_or_default();
                if !raw.is_empty() {
                    upstream = Some(Upstream::parse(raw));
                }
            }
        }

        Some(Self {
            event_id: event.id,
            owner: event.pubkey,
            created_at: event.created_at,
            id,
            name,
            description,
            web,
            clone,
            relays,
            euc,
            maintainers,
            upstream,
            hashtags,
        })
    }

    /// Returns the owner and identifier as a repository address.
    pub fn addr(&self) -> RepoAddr {
        RepoAddr::new(self.owner, self.id.clone())
    }

    /// Matches a `u` tag pointing at the base or a shared earliest unique commit.
    pub fn is_fork_of(&self, base: &RepoAddr, base_euc: Option<&str>) -> bool {
        if self.addr() == *base {
            return false;
        }

        if self.upstream.as_ref().and_then(|u| u.addr.as_ref()) == Some(base) {
            return true;
        }

        base_euc.is_some_and(|euc| self.euc.as_deref() == Some(euc))
    }

    /// Returns the description, defaulting to "No description".
    pub fn description(&self) -> &str {
        self.description.as_deref().unwrap_or("No description")
    }

    /// Adds the owner to the maintainers unless a `u` tag marks this a subordinate fork.
    pub fn effective_maintainers(&self) -> Vec<PublicKey> {
        let mut maintainers = self.maintainers.clone();
        if self.upstream.is_none() && !maintainers.contains(&self.owner) {
            maintainers.push(self.owner);
        }
        maintainers
    }

    /// Returns deduplicated `git clone` commands for the advertised URLs.
    pub fn clone_urls(&self) -> Vec<String> {
        let mut seen = HashSet::new();
        self.clone
            .iter()
            .map(|url| format!("git clone {url}"))
            .filter(|command| seen.insert(command.clone()))
            .collect()
    }
}

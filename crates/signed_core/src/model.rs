use nostr::prelude::*;

/// Parsed NIP-34 repository announcement (plain data, ready for the UI).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Announcement {
    /// Author of the announcement event.
    pub owner: PublicKey,
    /// When the announcement was published (for latest-wins resolution).
    pub created_at: Timestamp,
    /// Repository ID (`d` tag).
    pub id: String,
    pub name: Option<String>,
    pub description: Option<String>,
    /// Webpage URLs for browsing.
    pub web: Vec<String>,
    /// URLs for `git clone`.
    pub clone: Vec<String>,
    /// Relays the repository monitors for patches and issues.
    pub relays: Vec<String>,
    /// Earliest unique commit ID (`r` tag with `euc` marker).
    pub euc: Option<String>,
    /// Other recognized maintainers.
    pub maintainers: Vec<PublicKey>,
}

impl Announcement {
    /// Parse a kind `30617` event. Returns `None` if the kind is wrong or the `d` tag is missing.
    pub fn from_event(event: &Event) -> Option<Self> {
        if event.kind != Kind::GitRepoAnnouncement {
            return None;
        }

        let mut id: Option<String> = None;
        let mut name: Option<String> = None;
        let mut description: Option<String> = None;
        let mut web: Vec<String> = Vec::new();
        let mut clone: Vec<String> = Vec::new();
        let mut relays: Vec<String> = Vec::new();
        let mut euc: Option<String> = None;
        let mut maintainers: Vec<PublicKey> = Vec::new();

        for tag in event.tags.iter() {
            let values: &[String] = tag.as_slice();
            match tag.kind() {
                "d" => id = tag.content().map(str::to_owned),
                "name" => name = tag.content().map(str::to_owned),
                "description" => description = tag.content().map(str::to_owned),
                "web" => web.extend(values.iter().skip(1).cloned()),
                "clone" => clone.extend(values.iter().skip(1).cloned()),
                "relays" => relays.extend(values.iter().skip(1).cloned()),
                "r" => {
                    if values.get(2).map(String::as_str) == Some("euc") {
                        euc = tag.content().map(str::to_owned);
                    }
                }
                "maintainers" => {
                    maintainers.extend(
                        values
                            .iter()
                            .skip(1)
                            .filter_map(|v| PublicKey::from_hex(v).ok()),
                    );
                }
                _ => {}
            }
        }

        Some(Self {
            owner: event.pubkey,
            created_at: event.created_at,
            id: id?,
            name,
            description,
            web,
            clone,
            relays,
            euc,
            maintainers,
        })
    }

    /// The repository address of this announcement.
    pub fn addr(&self) -> crate::RepoAddr {
        crate::RepoAddr::new(self.owner, self.id.clone())
    }
}

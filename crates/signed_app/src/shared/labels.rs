use std::hash::{Hash, Hasher};
use std::path::Path;

use gpui::{App, Entity, SharedString};
use metadata::ProfileDirectory;
use nostr::prelude::{PublicKey, RelayUrl, ToBech32};
use signed_core::{Announcement, RepoAddr};
use signed_state::{RepoStore, ResolvedLocalRepo};

/// Display labels for repository announcements.
pub trait AnnouncementExt {
    /// The announcement's name, falling back to its id.
    fn display_name(&self) -> SharedString;

    /// The `nostr://` clone URL of the announcement, crediting `nip05` when known.
    fn clone_url(&self, nip05: Option<&str>) -> SharedString;

    /// The panel-stack key that identifies this repository.
    fn panel_key(&self) -> SharedString;
}

impl AnnouncementExt for Announcement {
    fn panel_key(&self) -> SharedString {
        SharedString::from(format!("repo:{}", self.addr()))
    }

    fn display_name(&self) -> SharedString {
        self.name
            .as_deref()
            .map(SharedString::from)
            .unwrap_or_else(|| SharedString::from(self.id.clone()))
    }

    fn clone_url(&self, nip05: Option<&str>) -> SharedString {
        let owner = self.owner;
        let user = nip05
            .map(str::to_owned)
            .unwrap_or_else(|| owner.to_bech32().unwrap_or_else(|_| owner.to_string()));

        let mut url = format!("nostr://{user}");
        if let Some(hint) = self.relays.first().and_then(RelayUrl::domain) {
            url.push('/');
            url.push_str(hint);
        }
        url.push('/');
        url.push_str(&self.id);

        SharedString::from(url)
    }
}

/// The label of a repository's upstream, when its address is in this list.
pub trait AnnouncementsExt {
    /// The display name of the announcement addressing `addr`, falling back to its identifier.
    fn upstream_display_name(&self, addr: &RepoAddr) -> SharedString;
}

impl AnnouncementsExt for [Announcement] {
    fn upstream_display_name(&self, addr: &RepoAddr) -> SharedString {
        self.iter()
            .find(|announcement| announcement.addr() == *addr)
            .map(AnnouncementExt::display_name)
            .unwrap_or_else(|| SharedString::from(addr.identifier().to_owned()))
    }
}

/// Short hex ids for display.
pub trait ShortIdExt {
    /// The first eight hex characters of an object id, for row labels.
    fn short_id(&self) -> SharedString;
}

impl ShortIdExt for str {
    fn short_id(&self) -> SharedString {
        SharedString::from(self.get(..8).unwrap_or(self).to_owned())
    }
}

/// The panel-stack key of a locally resolved repository.
pub trait ResolvedLocalRepoExt {
    /// The announced repository's key, or the local path's when unannounced.
    fn panel_key(&self) -> SharedString;
}

impl ResolvedLocalRepoExt for ResolvedLocalRepo {
    fn panel_key(&self) -> SharedString {
        match self.announcement.as_ref() {
            Some(announcement) => announcement.panel_key(),
            None => SharedString::from(format!("repo-local:{}", self.path.display())),
        }
    }
}

/// Text and path helpers shared by the repository views.
pub trait StrExt {
    /// Whether a file path has a markdown extension.
    fn is_markdown_path(&self) -> bool;

    /// Computes a source hash from the given text.
    fn source_hash(&self) -> u64;
}

impl StrExt for str {
    fn is_markdown_path(&self) -> bool {
        Path::new(self)
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| {
                matches!(
                    ext.to_ascii_lowercase().as_str(),
                    "md" | "markdown" | "mdown" | "mkdn"
                )
            })
    }

    fn source_hash(&self) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.hash(&mut hasher);
        hasher.finish()
    }
}

/// The display name of a repository store.
pub trait RepoStoreExt {
    /// The announcement's name or ID for announced repositories, the directory name for local ones.
    fn display_name(&self) -> SharedString;
}

impl RepoStoreExt for RepoStore {
    fn display_name(&self) -> SharedString {
        if self.addr().is_none() {
            return self
                .path
                .as_ref()
                .map(|path| {
                    SharedString::from(
                        path.file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_else(|| path.display().to_string()),
                    )
                })
                .unwrap_or_default();
        }

        self.announcement
            .as_ref()
            .map(AnnouncementExt::display_name)
            .unwrap_or_default()
    }
}

/// Normalises a relay URL typed by the user.
pub trait ServerInputExt {
    /// Returns the typed text, with a `wss://` scheme added when none is given, and the parsed relay.
    fn normalize_server(&self) -> Option<(String, RelayUrl)>;
}

impl ServerInputExt for str {
    fn normalize_server(&self) -> Option<(String, RelayUrl)> {
        let text = if self.contains("://") {
            self.to_owned()
        } else {
            format!("wss://{self}")
        };
        RelayUrl::parse(&text).ok().map(|relay| (text, relay))
    }
}

/// The host label shown for a relay.
pub trait RelayHostExt {
    /// The host of a relay URL, which is what the server lists show; the scheme is implied.
    fn host_label(&self) -> SharedString;
}

impl RelayHostExt for RelayUrl {
    fn host_label(&self) -> SharedString {
        self.domain()
            .map(SharedString::from)
            .unwrap_or_else(|| SharedString::from(self.to_string()))
    }
}

/// Tracks a detail view's author, commenters, and owner for profile prefetch.
pub trait ProfileDirectoryExt {
    /// Prefetches the profiles of `author`, `comments`, and `owner`.
    fn track_participants(
        &self,
        author: Option<PublicKey>,
        owner: Option<PublicKey>,
        comments: impl Iterator<Item = PublicKey>,
        cx: &mut App,
    );
}

impl ProfileDirectoryExt for Entity<ProfileDirectory> {
    fn track_participants(
        &self,
        author: Option<PublicKey>,
        owner: Option<PublicKey>,
        comments: impl Iterator<Item = PublicKey>,
        cx: &mut App,
    ) {
        let mut keys: Vec<PublicKey> = author.into_iter().collect();
        keys.extend(comments);
        keys.extend(owner);
        self.update(cx, |directory, cx| directory.track(keys, cx));
    }
}

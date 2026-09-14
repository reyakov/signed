use std::path::Path;

use anyhow::Result;
use gix::bstr::ByteSlice;
use nostr::prelude::*;

/// The kind of NIP-34 relationship a local repository has on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Nip34Kind {
    /// Bound to a NIP-34 coordinate, by `nak`'s `nip34.json` or `ngit`'s `nostr.repo`.
    Initialized,
    /// Cloned from a `nostr://` remote but never initialized locally.
    Cloned,
    /// Nostr tooling touched the repository but no binding is recoverable.
    ToolingOnly,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GraspSignals {
    pub nip34_json: bool,
    pub nip34_excluded: bool,
    pub nostr_repo_config: bool,
    pub nostr_remote: bool,
    pub grasp_remote: bool,
    /// `nip34_grasp_remote` is the `nak`-specific `nip34/grasp/<host>` remote name.
    pub nip34_grasp_remote: bool,
    pub nip34_state_refs: bool,
    pub nostr_cache: bool,
    pub nostr_aux_config: bool,
    pub maintainers_yaml: bool,
}

impl GraspSignals {
    pub fn any(&self) -> bool {
        *self != Self::default()
    }
}

/// What a local repository's on-disk state says about its NIP-34 binding.
#[derive(Debug, Clone, PartialEq)]
pub struct Nip34Binding {
    pub kind: Nip34Kind,
    pub signals: GraspSignals,
    /// Coordinate owner and identifier, from `nip34.json` or `nostr.repo`.
    pub owner: Option<PublicKey>,
    pub identifier: Option<String>,
    pub grasp_urls: Vec<String>,
}

#[derive(serde::Deserialize)]
struct Nip34Json {
    identifier: Option<String>,
    owner: Option<String>,
}

pub fn detect_nip34(repo_path: &Path) -> Option<Nip34Binding> {
    let repo = gix::open(repo_path).ok()?;
    let common_dir = repo.common_dir().to_path_buf();
    let workdir = repo.workdir().map(Path::to_path_buf);

    let mut signals = GraspSignals::default();
    let mut owner: Option<PublicKey> = None;
    let mut identifier: Option<String> = None;
    let mut grasp_urls: Vec<String> = Vec::new();

    if let Some(workdir) = &workdir {
        if let Ok(bytes) = std::fs::read(workdir.join("nip34.json"))
            && let Ok(config) = serde_json::from_slice::<Nip34Json>(&bytes)
        {
            signals.nip34_json = true;
            identifier = config.identifier.and_then(non_empty);
            owner = config
                .owner
                .as_deref()
                .and_then(|value| PublicKey::parse(value).ok());
        }

        if workdir.join("maintainers.yaml").is_file() {
            signals.maintainers_yaml = true;
        }
    }

    if let Ok(exclude) = std::fs::read_to_string(common_dir.join("info/exclude"))
        && exclude.contains("nip34.json")
    {
        signals.nip34_excluded = true;
    }

    // `ngit` keeps its repository event cache in the Git common directory.
    if common_dir.join("nostr-cache.lmdb").is_file() {
        signals.nostr_cache = true;
    }

    // `ngit` reads and writes `nostr.repo` at repository-local scope only.
    if let Ok(config) = gix::config::File::from_path_no_includes(
        common_dir.join("config"),
        gix::config::Source::Local,
    ) {
        if let Some(value) = config.string("nostr.repo")
            && let Some((key, id)) = coordinate_from_naddr(&value.to_str_lossy())
        {
            signals.nostr_repo_config = true;
            owner = Some(key);
            identifier = Some(id);
        }

        for key in ["nostr.repo-relay-only", "nostr.nostate", "nostr.private"] {
            if config.string(key).is_some() {
                signals.nostr_aux_config = true;
            }
        }

        if let Some(sections) = config.sections_by_name("remote") {
            for section in sections {
                let Some(name) = section.header().subsection_name() else {
                    continue;
                };
                let nak_grasp_remote = name.to_str_lossy().starts_with("nip34/grasp/");

                for url in section.values("url") {
                    let url = url.to_str_lossy();

                    if url.starts_with("nostr://") {
                        signals.nostr_remote = true;
                        // Strong markers win; only fill an empty binding.
                        if owner.is_none()
                            && identifier.is_none()
                            && let Some((key, id)) = parse_nostr_url(&url)
                        {
                            owner = Some(key);
                            identifier = Some(id);
                        }
                    }

                    if is_grasp_url(&url) {
                        signals.grasp_remote = true;
                        signals.nip34_grasp_remote |= nak_grasp_remote;
                        grasp_urls.push(url.to_string());

                        if owner.is_none()
                            && identifier.is_none()
                            && let Some((key, id)) = grasp_parts(&url)
                        {
                            owner = Some(key);
                            identifier = Some(id);
                        }
                    }
                }
            }
        }
    }

    // `nak` materializes a kind-30618 state as `refs/heads/nip34/state/*`.
    if let Ok(platform) = repo.references()
        && let Ok(mut refs) = platform.prefixed(b"refs/heads/nip34/state/")
        && refs.next().is_some()
    {
        signals.nip34_state_refs = true;
    }

    if !signals.any() {
        return None;
    }

    let kind = if signals.nip34_json
        || signals.nostr_repo_config
        || signals.nip34_grasp_remote
        || signals.nip34_state_refs
    {
        Nip34Kind::Initialized
    } else if signals.nostr_remote {
        Nip34Kind::Cloned
    } else {
        Nip34Kind::ToolingOnly
    };

    Some(Nip34Binding {
        kind,
        signals,
        owner,
        identifier,
        grasp_urls,
    })
}

/// Record a repository's NIP-34 coordinate in its local `nostr.repo` config.
pub fn set_nostr_repo(repo_path: &Path, naddr: &str) -> Result<()> {
    let repo = gix::open(repo_path)?;

    crate::remote::edit_local_config(&repo, |config| {
        config.set_raw_value("nostr.repo", naddr)?;
        Ok(())
    })
}

/// Mirrors `nak`'s `IsGraspURL`: two path segments, a path of at least 65 bytes,
/// and a first segment that decodes as an `npub`.
pub fn is_grasp_url(url: &str) -> bool {
    let Ok(parsed) = Url::parse(url) else {
        return false;
    };

    if !matches!(parsed.scheme(), "http" | "https" | "grasp") {
        return false;
    }

    let path = parsed.path();
    if path.matches('/').count() != 2 || path.len() < 65 {
        return false;
    }

    grasp_parts(url).is_some()
}

fn grasp_parts(url: &str) -> Option<(PublicKey, String)> {
    let parsed = Url::parse(url).ok()?;
    let mut segments = parsed.path_segments()?.filter(|part| !part.is_empty());

    let owner = PublicKey::parse(segments.next()?).ok()?;
    let identifier = non_empty(segments.next()?.trim_end_matches(".git"))?;

    Some((owner, identifier))
}

fn coordinate_from_naddr(value: &str) -> Option<(PublicKey, String)> {
    let coordinate = Nip19Coordinate::from_bech32(value).ok()?;
    if coordinate.kind != Kind::GitRepoAnnouncement {
        return None;
    }

    let identifier = non_empty(coordinate.identifier.clone())?;
    Some((coordinate.public_key, identifier))
}

/// Handles a bare `naddr`, an `npub`, and the optional `[ssh-key-file@]`,
/// `[protocol/]` and `[relay/]` components. An `nip05` owner yields no binding.
fn parse_nostr_url(url: &str) -> Option<(PublicKey, String)> {
    let rest = url.strip_prefix("nostr://")?;

    if rest.starts_with("naddr1") {
        return coordinate_from_naddr(rest);
    }

    let rest = rest.rsplit_once('@').map_or(rest, |(_, after)| after);
    let mut parts: Vec<&str> = rest.split('/').filter(|part| !part.is_empty()).collect();

    if parts
        .first()
        .is_some_and(|first| matches!(*first, "ssh" | "https" | "http"))
    {
        parts.remove(0);
    }

    // `[owner, (relay), identifier]`.
    if parts.len() < 2 {
        return None;
    }

    let owner = PublicKey::parse(parts[0]).ok()?;
    let identifier = non_empty(parts.last()?.trim_end_matches(".git"))?;

    Some((owner, identifier))
}

fn non_empty(value: impl Into<String>) -> Option<String> {
    let value = value.into();
    (!value.is_empty()).then_some(value)
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::*;

    fn init_repo() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("repo");
        std::fs::create_dir_all(&path).expect("mkdir");
        git(&path, &["init", "-q"]);
        (dir, path)
    }

    fn git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "Test Author")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test Author")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .env("GIT_EDITOR", "true")
            .args(args)
            .status()
            .expect("spawn git");
        assert!(status.success(), "git {args:?} failed");
    }

    fn key() -> PublicKey {
        Keys::generate().public_key()
    }

    fn naddr(kind: Kind, owner: PublicKey, identifier: &str) -> String {
        let coordinate = Coordinate::new(kind, owner).identifier(identifier);
        Nip19Coordinate::new(coordinate, Vec::<RelayUrl>::new())
            .to_bech32()
            .expect("naddr")
    }

    #[test]
    fn plain_repository_has_no_binding() {
        let (_dir, path) = init_repo();
        assert!(detect_nip34(&path).is_none());
    }

    #[test]
    fn nip34_json_marks_a_repository_initialized() {
        let (_dir, path) = init_repo();
        let owner = key();
        let npub = owner.to_bech32().expect("npub");
        std::fs::write(
            path.join("nip34.json"),
            format!(r#"{{"identifier":"my-repo","owner":"{npub}"}}"#),
        )
        .expect("write");

        let binding = detect_nip34(&path).expect("binding");
        assert_eq!(binding.kind, Nip34Kind::Initialized);
        assert!(binding.signals.nip34_json);
        assert_eq!(binding.owner, Some(owner));
        assert_eq!(binding.identifier.as_deref(), Some("my-repo"));
    }

    #[test]
    fn malformed_nip34_json_is_ignored() {
        let (_dir, path) = init_repo();
        std::fs::write(path.join("nip34.json"), b"not json").expect("write");

        assert!(detect_nip34(&path).is_none());
    }

    #[test]
    fn nak_exclude_and_state_refs_are_detected() {
        let (_dir, path) = init_repo();

        std::fs::create_dir_all(path.join(".git/info")).expect("mkdir");
        std::fs::write(path.join(".git/info/exclude"), "nip34.json\n").expect("write");

        git(&path, &["commit", "-q", "--allow-empty", "-m", "initial"]);
        git(
            &path,
            &["update-ref", "refs/heads/nip34/state/HEAD", "HEAD"],
        );

        let binding = detect_nip34(&path).expect("binding");
        assert_eq!(binding.kind, Nip34Kind::Initialized);
        assert!(binding.signals.nip34_excluded);
        assert!(binding.signals.nip34_state_refs);
    }

    #[test]
    fn nostr_repo_config_marks_a_repository_initialized() {
        let (_dir, path) = init_repo();
        let owner = key();
        let naddr = naddr(Kind::GitRepoAnnouncement, owner, "my-repo");
        git(&path, &["config", "nostr.repo", &naddr]);

        let binding = detect_nip34(&path).expect("binding");
        assert_eq!(binding.kind, Nip34Kind::Initialized);
        assert!(binding.signals.nostr_repo_config);
        assert_eq!(binding.owner, Some(owner));
        assert_eq!(binding.identifier.as_deref(), Some("my-repo"));
    }

    #[test]
    fn the_written_nostr_repo_marker_is_detected() {
        let (_dir, path) = init_repo();
        let owner = key();
        let naddr = naddr(Kind::GitRepoAnnouncement, owner, "my-repo");

        set_nostr_repo(&path, &naddr).expect("write marker");

        let binding = detect_nip34(&path).expect("binding");
        assert_eq!(binding.kind, Nip34Kind::Initialized);
        assert!(binding.signals.nostr_repo_config);
        assert_eq!(binding.owner, Some(owner));
        assert_eq!(binding.identifier.as_deref(), Some("my-repo"));
    }

    #[test]
    fn nostr_remote_is_a_nip34_clone() {
        let (_dir, path) = init_repo();
        let owner = key();
        let npub = owner.to_bech32().expect("npub");
        let url = format!("nostr://{npub}/relay.ngit.dev/my-repo");
        git(&path, &["remote", "add", "origin", &url]);

        let binding = detect_nip34(&path).expect("binding");
        assert_eq!(binding.kind, Nip34Kind::Cloned);
        assert!(binding.signals.nostr_remote);
        assert_eq!(binding.owner, Some(owner));
        assert_eq!(binding.identifier.as_deref(), Some("my-repo"));
    }

    #[test]
    fn nak_grasp_remote_marks_a_repository_initialized() {
        let (_dir, path) = init_repo();
        let owner = key();
        let npub = owner.to_bech32().expect("npub");
        let url = format!("https://gitnostr.com/{npub}/my-repo.git");
        git(
            &path,
            &["config", "remote.nip34/grasp/gitnostr.com.url", &url],
        );

        let binding = detect_nip34(&path).expect("binding");
        assert_eq!(binding.kind, Nip34Kind::Initialized);
        assert!(binding.signals.nip34_grasp_remote);
        assert!(binding.signals.grasp_remote);
        assert_eq!(binding.grasp_urls, vec![url]);
        assert_eq!(binding.owner, Some(owner));
        assert_eq!(binding.identifier.as_deref(), Some("my-repo"));
    }

    #[test]
    fn nostr_cache_alone_is_tooling_only() {
        let (_dir, path) = init_repo();
        std::fs::write(path.join(".git/nostr-cache.lmdb"), b"cache").expect("write");

        let binding = detect_nip34(&path).expect("binding");
        assert_eq!(binding.kind, Nip34Kind::ToolingOnly);
        assert!(binding.signals.nostr_cache);
    }

    #[test]
    fn grasp_urls_are_recognised_by_shape() {
        let owner = key();
        let npub = owner.to_bech32().expect("npub");

        assert!(is_grasp_url(&format!(
            "https://gitnostr.com/{npub}/my-repo.git"
        )));
        assert!(is_grasp_url(&format!(
            "grasp://gitnostr.com/{npub}/my-repo.git"
        )));

        assert!(!is_grasp_url("https://gitnostr.com/my-repo.git"));
        assert!(!is_grasp_url(
            "https://gitnostr.com/not-a-pubkey/my-repo.git"
        ));
        assert!(!is_grasp_url(&format!(
            "ssh://gitnostr.com/{npub}/my-repo.git"
        )));
    }
}

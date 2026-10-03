use nostr::prelude::*;

/// Compare git URLs ignoring scheme, trailing `.git` and path slashes, so a
/// grasp announce URL matches its https origin. Unparseable values compare literally.
pub fn same_repo_url(a: &str, b: &str) -> bool {
    match (url_identity(a), url_identity(b)) {
        (Some(a), Some(b)) => a == b,
        _ => a == b,
    }
}

fn url_identity(url: &str) -> Option<(String, Option<u16>, String)> {
    let parsed = Url::parse(url).ok()?;
    let host = parsed.host_str()?.to_ascii_lowercase();
    let mut path = parsed.path().trim_matches('/').to_owned();
    if let Some(stripped) = path.strip_suffix(".git") {
        path = stripped.to_owned();
    }
    Some((host, parsed.port(), path))
}

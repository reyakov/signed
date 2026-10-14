use nostr::prelude::Url;
use nostr_sdk::prelude::*;

/// `ws://` grasp servers, like ngit, use `http(s)://<host>`.
pub(crate) fn grasp_base_url(relay: &RelayUrl) -> Option<String> {
    let parsed = Url::parse(relay.as_str()).ok()?;
    let host = parsed.host_str()?;
    let port = parsed
        .port()
        .map(|port| format!(":{port}"))
        .unwrap_or_default();
    let scheme = if relay.scheme().is_secure() {
        "https"
    } else {
        "http"
    };
    Some(format!("{scheme}://{host}{port}"))
}

pub(crate) fn grasp_clone_url(relay: &RelayUrl, owner: &str, repo_id: &str) -> Option<Url> {
    let base = grasp_base_url(relay)?;
    Url::parse(&format!("{base}/{owner}/{repo_id}.git")).ok()
}

/// GRASP-06 contributor namespace URL of a pull request tip.
pub(crate) fn grasp06_prs_url(base_url: &str, npub: &str, repo_id: &str) -> String {
    format!("{base_url}/prs/{npub}/{repo_id}.git")
}

/// The author's GRASP-06 `/prs/` URLs come first.
pub(crate) fn pr_clone_urls(prs_urls: Vec<Url>, base_clone_urls: Vec<Url>) -> Vec<Url> {
    let mut seen = std::collections::HashSet::new();
    let mut urls = Vec::new();
    for url in prs_urls.into_iter().chain(base_clone_urls) {
        if seen.insert(url.to_string()) {
            urls.push(url);
        }
    }
    urls
}

#[derive(Debug, Clone)]
pub struct GraspServer {
    relay: RelayUrl,
    reason: Option<String>,
}

impl GraspServer {
    pub(crate) fn ok(relay: RelayUrl) -> Self {
        Self {
            relay,
            reason: None,
        }
    }

    pub(crate) fn failed(relay: &RelayUrl, reason: impl Into<String>) -> Self {
        Self {
            relay: relay.to_owned(),
            reason: Some(reason.into()),
        }
    }

    pub fn relay(&self) -> &RelayUrl {
        &self.relay
    }

    pub fn reason(&self) -> Option<&str> {
        self.reason.as_deref()
    }
}

#[derive(Debug, Clone, Default)]
pub struct PushOutcome {
    pub servers: Vec<GraspServer>,
    /// The newest state event a grasp relay accepted for this push.
    pub state_event: Option<Event>,
}

impl PushOutcome {
    pub fn accepted(&self) -> usize {
        self.servers
            .iter()
            .filter(|server| server.reason.is_none())
            .count()
    }

    fn failing(&self) -> impl Iterator<Item = &GraspServer> {
        self.servers.iter().filter(|server| server.reason.is_some())
    }

    pub fn failure_summary(&self) -> String {
        self.failing()
            .map(|server| {
                let reason =
                    utils::flatten_whitespace(server.reason.as_deref().unwrap_or("unknown error"));
                format!("{}: {reason}", server.relay)
            })
            .collect::<Vec<_>>()
            .join("; ")
    }

    /// `None` when every server accepted the push or nothing was pushed.
    pub fn partial_warning(&self) -> Option<String> {
        let accepted = self.accepted();
        if self.servers.is_empty() || accepted == self.servers.len() {
            return None;
        }
        Some(format!(
            "Pushed to {accepted} of {} grasp servers: {}. Republish to sync.",
            self.servers.len(),
            self.failure_summary()
        ))
    }
}

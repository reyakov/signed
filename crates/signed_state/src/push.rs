use std::path::Path;

use anyhow::Error;
use nostr::prelude::Url;
use nostr_sdk::prelude::*;
use signed_core::RepoState;

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
    fn ok(relay: RelayUrl) -> Self {
        Self {
            relay,
            reason: None,
        }
    }

    fn failed(relay: RelayUrl, reason: impl Into<String>) -> Self {
        Self {
            relay,
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

    /// All staged events carry the same refs; the newest timestamp wins on the relays.
    fn keep_newest(&mut self, event: Event) {
        if self
            .state_event
            .as_ref()
            .is_none_or(|current| event.created_at > current.created_at)
        {
            self.state_event = Some(event);
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn push_staged_to_grasps<S>(
    client: &Client,
    signer: &S,
    repo_id: &str,
    refs: &[(String, String)],
    head: Option<&str>,
    path: &Path,
    owner: &str,
    servers: &[RelayUrl],
    push: impl Fn(&Path, &str, &str, &str) -> Result<(), Error>,
) -> PushOutcome
where
    S: AsyncGetPublicKey + AsyncSignEvent + ?Sized,
{
    let mut outcome = PushOutcome::default();

    for relay in servers {
        let Some(base) = grasp_base_url(relay) else {
            outcome
                .servers
                .push(GraspServer::failed(relay.clone(), "no domain"));
            continue;
        };

        let event = match RepoState::build(repo_id, refs, head)
            .finalize_async(signer)
            .await
        {
            Ok(event) => event,
            Err(e) => {
                let reason = format!("could not sign the state event: {e}");
                outcome
                    .servers
                    .push(GraspServer::failed(relay.clone(), reason));
                continue;
            }
        };

        // The relay parks the accepted event in purgatory, authorizing the paired git push.
        let staging = match client.add_relay(relay).and_connect().await {
            Ok(_) => match client.send_event(&event).to([relay.clone()]).await {
                Ok(output) if output.success.contains_key(relay) => Ok(()),
                Ok(output) => Err(output
                    .failed
                    .get(relay)
                    .cloned()
                    .unwrap_or_else(|| "relay did not confirm the event".to_owned())),
                Err(e) => Err(format!("could not send the state event to {relay}: {e}")),
            },
            Err(e) => Err(format!("could not add relay {relay}: {e}")),
        };

        if let Err(reason) = staging {
            log::warn!("grasp push failed: {relay}: {reason}");
            outcome
                .servers
                .push(GraspServer::failed(relay.clone(), reason));
            continue;
        }

        match push(path, &base, owner, repo_id) {
            Ok(()) => {
                outcome.keep_newest(event);
                outcome.servers.push(GraspServer::ok(relay.clone()));
            }
            Err(e) => {
                log::warn!("grasp push failed: {relay}: {e}");
                outcome
                    .servers
                    .push(GraspServer::failed(relay.clone(), e.to_string()));
            }
        }
    }

    outcome
}

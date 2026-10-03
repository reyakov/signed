use std::path::Path;
use std::time::Duration;

use anyhow::{Error, bail};
use gpui::BackgroundExecutor;
use nostr::event::IntoEventBuilder;
use nostr::prelude::Url;
use nostr_sdk::prelude::*;
use signed_core::RepoState;
use signed_git::Repo;
use signed_nostr::UniversalSigner;

pub(crate) const GRASP_PUSH_ATTEMPTS: usize = 3;

const GRASP_RETRY_DELAY: Duration = Duration::from_secs(1);

// `ws://` grasp servers, like ngit, use `http://<host>`; secure relays map to
// `https://<host>`.
pub(crate) fn grasp_base_url(relay: &RelayUrl) -> Option<String> {
    let parsed = Url::parse(relay.as_str()).ok()?;
    let host = parsed.host_str()?;
    let port = parsed.port().map(|p| format!(":{p}")).unwrap_or_default();
    // `ws://` grasp servers, e.g. local dev relays, speak plain HTTP.
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

// GRASP-06 contributor namespace URL of a pull request tip.
pub(crate) fn grasp06_prs_url(base_url: &str, npub: &str, repo_id: &str) -> String {
    format!("{base_url}/prs/{npub}/{repo_id}.git")
}

// The author's GRASP-06 `/prs/` URLs come first.
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
    // The newest state event a grasp relay accepted for this push; broadcast
    // to the other relays once a git server holds the data.
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

    // `None` when every server accepted the push or nothing was pushed.
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

// Reasons a push attempt should be retried with a freshly staged state event
// and a fresh git advertisement. Two families are retried:
//
// - **Purgatory denials**: the grasp server sends these when the state event
//   for the push has not reached its purgatory yet. Re-staging a fresh event
//   resolves them.
// - **Stale advertisement races**: `git receive-pack` compares each ref
//   update against the value it advertised when the push started. The grasp
//   server's own background sync can move a ref in between - typically by
//   aligning the repository to a parked state event once the objects of an
//   earlier attempt land - so the compare-and-swap fails with `cannot lock
//   ref` / `incorrect old value provided`. A retry against the fresh
//   advertisement converges, and when the race is lost the pushed data is
//   usually already on the server (see `is_stale_advertisement_race` and
//   the convergence probe in `push_staged_to_grasps`).
//
// Other rejections are not retried.
fn is_transient_grasp_denial(stderr: &str) -> bool {
    let error = stderr.to_lowercase();
    [
        "no state events in purgatory",
        "no matching state event",
        "doesn't match push",
        "none from authorized publishers",
        "no repository announcement found",
        "cannot lock ref",
        "incorrect old value provided",
    ]
    .iter()
    .any(|marker| error.contains(marker))
}

// A push rejected because `git receive-pack`'s compare-and-swap lost to the
// grasp server's own background ref alignment: the ref moved between this
// push's advertisement and its ref transaction (`cannot lock ref ... is at
// ... but expected ...` / `incorrect old value provided`). The pushed data
// is usually already on the server by then.
fn is_stale_advertisement_race(stderr: &str) -> bool {
    let error = stderr.to_lowercase();
    error.contains("cannot lock ref") || error.contains("incorrect old value provided")
}

// All staged events carry the same refs; the newest timestamp wins on the relays.
fn keep_newest(state_event: &mut Option<Event>, event: Event) {
    if state_event
        .as_ref()
        .is_none_or(|current| event.created_at > current.created_at)
    {
        *state_event = Some(event);
    }
}

// The grasp push pipeline: stage a signed state event on each server's
// relay, then push the git data, retrying transient denials.
#[derive(Clone)]
pub(crate) struct GraspPush {
    client: Client,
    signer: UniversalSigner,
}

impl GraspPush {
    pub(crate) fn new(client: Client, signer: UniversalSigner) -> Self {
        Self { client, signer }
    }

    pub(crate) fn require_relay_accepted(
        output: SendEventOutput,
        event: Event,
    ) -> Result<Event, Error> {
        if output.success.is_empty() && !output.failed.is_empty() {
            let reasons = output
                .failed
                .values()
                .cloned()
                .collect::<Vec<String>>()
                .join(", ");
            bail!("event not accepted by any relay: {reasons}");
        }

        Ok(event)
    }

    // A relay hiccup should not block sign-up, so failures are only logged.
    pub(crate) async fn publish_best_effort(&self, builder: EventBuilder) {
        let result: Result<(), Error> = async {
            let event = builder.finalize_async(&self.signer).await?;
            let output = self.client.send_event(&event).broadcast().await?;
            Self::require_relay_accepted(output, event)?;
            Ok(())
        }
        .await;

        if let Err(e) = result {
            log::warn!("failed to publish identity bootstrap event: {e}");
        }
    }

    pub(crate) async fn publish_one(&self, builder: EventBuilder) -> Result<Event, Error> {
        let event = builder.finalize_async(&self.signer).await?;
        self.send_accepted(event).await
    }

    pub(crate) async fn send_accepted(&self, event: Event) -> Result<Event, Error> {
        let output = self.client.send_event(&event).broadcast().await?;
        Self::require_relay_accepted(output, event)
    }

    pub(crate) async fn retract_event(&self, event: &Event) -> Result<(), Error> {
        let builder = EventDeletionRequest::new()
            .id(event.id)
            .into_event_builder();

        let deletion = builder.finalize_async(&self.signer).await?;
        self.client.send_event(&deletion).broadcast().await?;

        Ok(())
    }

    // Retries within the same second get the next second: a grasp relay
    // treats a same-id resend as a duplicate and does not re-run its ingest,
    // so an identical resend cannot re-park a state event lost from its purgatory.
    async fn sign_state_event(
        &self,
        repo_id: &str,
        refs: &[(String, String)],
        head: Option<&str>,
        last_created_at: u64,
    ) -> Result<(Event, u64), String> {
        let now = Timestamp::now().as_secs();
        let created_at = if now > last_created_at {
            now
        } else {
            last_created_at + 1
        };

        let event = RepoState::build(repo_id, refs, head)
            .custom_created_at(Timestamp::from_secs(created_at))
            .finalize_async(&self.signer)
            .await
            .map_err(|e| format!("could not sign the state event: {e}"))?;

        Ok((event, created_at))
    }

    // `Ok` only when the relay confirmed the event. On a grasp relay the
    // accept parks the event in purgatory, which authorizes the paired git push.
    async fn stage_event_on_relay(&self, relay: &RelayUrl, event: &Event) -> Result<(), String> {
        self.client
            .add_relay(relay)
            .and_connect()
            .await
            .map_err(|e| format!("could not add relay {relay}: {e}"))?;

        let output = self
            .client
            .send_event(event)
            .to([relay.clone()])
            .await
            .map_err(|e| format!("could not send the state event to {relay}: {e}"))?;

        if output.success.contains_key(relay) {
            Ok(())
        } else {
            let reason = output
                .failed
                .get(relay)
                .cloned()
                .unwrap_or_else(|| "relay did not confirm the event".to_owned());
            Err(reason)
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn push_staged_to_grasps(
        &self,
        repo_id: &str,
        refs: &[(String, String)],
        head: Option<&str>,
        path: &Path,
        owner: &str,
        servers: &[RelayUrl],
        executor: &BackgroundExecutor,
        push: impl Fn(&Path, &str, &str, &str) -> Result<(), Error>,
    ) -> PushOutcome {
        let mut outcome = PushOutcome::default();

        for relay in servers {
            let Some(base) = grasp_base_url(relay) else {
                outcome
                    .servers
                    .push(GraspServer::failed(relay.clone(), "no domain"));
                continue;
            };
            let git_url = format!("{base}/{owner}/{repo_id}.git");

            let mut reason = None;
            let mut last_created_at = 0;
            // For the convergence probe when every push attempt lost the
            // stale-ref race.
            let mut staged_event = None;

            'server: for attempt in 1..=GRASP_PUSH_ATTEMPTS {
                if attempt > 1 {
                    executor.timer(GRASP_RETRY_DELAY).await;
                }

                let (event, created_at) = match self
                    .sign_state_event(repo_id, refs, head, last_created_at)
                    .await
                {
                    Ok(signed) => signed,
                    Err(e) => {
                        reason = Some(e);
                        break 'server;
                    }
                };

                last_created_at = created_at;

                // A failed stage means the grasp never parked the state, so
                // the git push would be denied anyway: skip it.
                if let Err(e) = self.stage_event_on_relay(relay, &event).await {
                    // One retry absorbs a relay connect blip, on the first
                    // attempt only.
                    if attempt == 1 && self.stage_event_on_relay(relay, &event).await.is_ok() {
                    } else {
                        reason = Some(e);
                        break 'server;
                    }
                }
                staged_event = Some(event.clone());

                match push(path, &base, owner, repo_id) {
                    Ok(()) => {
                        keep_newest(&mut outcome.state_event, event);
                        break 'server;
                    }
                    Err(e) => {
                        let text = e.to_string();
                        if attempt < GRASP_PUSH_ATTEMPTS && is_transient_grasp_denial(&text) {
                            reason = Some(text);
                            continue 'server;
                        }
                        reason = Some(text);
                        break 'server;
                    }
                }
            }

            // The grasp's own background sync aligns refs to staged state
            // events as soon as the objects land, which can beat every push
            // attempt's compare-and-swap. When the last denial was that race
            // the sync has usually finished by now: verify the advertised refs
            // and accept the server when the pushed data is already there.
            if let Some(last_reason) = &reason
                && is_stale_advertisement_race(last_reason)
                && Repo::open(path)
                    .and_then(|repo| repo.remote_has_refs(&git_url, refs))
                    .unwrap_or(false)
            {
                if let Some(event) = staged_event {
                    keep_newest(&mut outcome.state_event, event);
                }
                reason = None;
            }

            match reason {
                Some(reason) => {
                    log::warn!("grasp push failed: {relay}: {reason}");
                    outcome
                        .servers
                        .push(GraspServer::failed(relay.clone(), reason));
                }
                None => outcome.servers.push(GraspServer::ok(relay.clone())),
            }
        }

        outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grasp_base_url_maps_schemes_like_ngit() {
        let wss = RelayUrl::parse("wss://relay.ngit.dev").expect("url");
        assert_eq!(
            grasp_base_url(&wss).as_deref(),
            Some("https://relay.ngit.dev")
        );

        let ws = RelayUrl::parse("ws://localhost:8080").expect("url");
        assert_eq!(
            grasp_base_url(&ws).as_deref(),
            Some("http://localhost:8080")
        );
    }

    #[test]
    fn grasp_clone_url_matches_ngit_format() {
        let relay = RelayUrl::parse("wss://gitnostr.com").expect("url");
        let url = grasp_clone_url(&relay, "npub1test", "my-repo").expect("url");
        assert_eq!(
            url.to_string(),
            "https://gitnostr.com/npub1test/my-repo.git"
        );
    }

    #[test]
    fn grasp06_prs_url_matches_ngit_format() {
        assert_eq!(
            grasp06_prs_url("https://relay.ngit.dev", "npub1author", "my-repo"),
            "https://relay.ngit.dev/prs/npub1author/my-repo.git"
        );
        assert_eq!(
            grasp06_prs_url("http://localhost:8080", "npub1author", "my-repo"),
            "http://localhost:8080/prs/npub1author/my-repo.git"
        );
    }

    #[test]
    fn pr_clone_urls_orders_author_first_and_deduplicates() {
        let prs = vec![
            Url::parse("https://a.example/prs/npub1me/repo.git").expect("url"),
            Url::parse("https://a.example/prs/npub1me/repo.git").expect("url"),
        ];
        let base = vec![
            Url::parse("https://a.example/npub1owner/repo.git").expect("url"),
            Url::parse("https://b.example/npub1owner/repo.git").expect("url"),
            Url::parse("https://b.example/npub1owner/repo.git").expect("url"),
        ];

        let urls = pr_clone_urls(prs, base);
        assert_eq!(
            urls.iter().map(ToString::to_string).collect::<Vec<_>>(),
            vec![
                "https://a.example/prs/npub1me/repo.git",
                "https://a.example/npub1owner/repo.git",
                "https://b.example/npub1owner/repo.git",
            ]
        );
    }

    #[test]
    fn transient_grasp_denials_are_classified() {
        let reported = "remote: ERR authorisation failed: No state events in purgatory\n\
            fatal: the remote end hung up unexpectedly\n\
            error: failed to push some refs to 'https://relay.ngit.dev/...git'";
        assert!(is_transient_grasp_denial(reported));

        assert!(is_transient_grasp_denial(
            "remote: ERR authorisation failed: No matching state event found in purgatory"
        ));
        assert!(is_transient_grasp_denial(
            "remote: ERR authorisation failed: 1 state event in purgatory from authorized \
             publisher but doesn't match push"
        ));
        assert!(is_transient_grasp_denial(
            "remote: ERR authorisation failed: 2 state events in purgatory but none from \
             authorized publishers"
        ));
        assert!(is_transient_grasp_denial(
            "remote: ERR authorisation failed: No repository announcement found"
        ));

        assert!(!is_transient_grasp_denial(
            "remote: ERR authorisation failed: not a maintainer of this repository"
        ));
        assert!(!is_transient_grasp_denial(
            "fatal: unable to access 'https://relay.ngit.dev/...': The requested URL returned \
             error: 403"
        ));
        assert!(!is_transient_grasp_denial(
            "fatal: unable to access 'https://relay.ngit.dev/...': Could not resolve host"
        ));
    }

    #[test]
    fn stale_ref_races_are_retried() {
        let reported = "remote: error: cannot lock ref 'refs/heads/main': is at \
            cac2ac91b6f5fb8dfcb6962785babc6e65350cb3 but expected \
            bc5e892aa84dc6240a5fbcd59367a4857d26f49b\n\
            To https://relay.ngit.dev/npub1owner/signed-test.git\n\
             ! [remote rejected] main -> main (incorrect old value provided)\n\
            error: failed to push some refs to 'https://relay.ngit.dev/npub1owner/signed-test.git'";
        assert!(is_transient_grasp_denial(reported));
        assert!(is_stale_advertisement_race(reported));

        assert!(is_stale_advertisement_race(
            "cannot lock ref 'refs/heads/main'"
        ));
        assert!(is_stale_advertisement_race(
            "! [remote rejected] main -> main (incorrect old value provided)"
        ));

        assert!(!is_stale_advertisement_race("No state events in purgatory"));

        assert!(!is_transient_grasp_denial(
            " ! [rejected]        main -> main (non-fast-forward)"
        ));
    }
}

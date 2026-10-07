use nostr::prelude::*;

#[derive(serde::Deserialize)]
pub(crate) struct Nip34Json {
    pub(crate) identifier: Option<String>,
    pub(crate) owner: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Nip34Kind {
    Initialized,
    Cloned,
    ToolingOnly,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GraspSignals {
    pub nip34_json: bool,
    pub nip34_excluded: bool,
    pub nostr_repo_config: bool,
    pub nostr_remote: bool,
    pub grasp_remote: bool,
    pub nip34_grasp_remote: bool,
    pub nip34_state_refs: bool,
    pub nostr_cache: bool,
    pub nostr_aux_config: bool,
    pub maintainers_yaml: bool,
}

impl GraspSignals {
    /// Returns whether any Nostr tooling marker was found.
    pub fn any(&self) -> bool {
        *self != Self::default()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Nip34Binding {
    pub kind: Nip34Kind,
    pub signals: GraspSignals,
    pub owner: Option<PublicKey>,
    pub identifier: Option<String>,
    pub grasp_urls: Vec<String>,
}

impl Nip34Binding {
    /// Creates a binding from its kind, detected signals, and recovered coordinates.
    pub fn new(
        kind: Nip34Kind,
        signals: GraspSignals,
        owner: Option<PublicKey>,
        identifier: Option<String>,
        grasp_urls: Vec<String>,
    ) -> Self {
        Self {
            kind,
            signals,
            owner,
            identifier,
            grasp_urls,
        }
    }
}

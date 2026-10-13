use bitcoin_hashes::sha1::Hash as Sha1Hash;
use signed_git::PatchParser;

/// NIP-34 suggests patches when each event is under 60kb.
pub(crate) const MAX_PATCH_EVENT_BYTES: usize = 60 * 1024;

pub(crate) struct PatchSeries {
    pub(crate) parts: Vec<String>,
}

impl PatchSeries {
    pub(crate) fn parse(patch: &str) -> Self {
        Self {
            parts: PatchParser::split_patch_series(patch)
                .into_iter()
                .map(str::to_owned)
                .collect(),
        }
    }

    /// Returns the length of the first part that exceeds the NIP-34 recommended size.
    pub(crate) fn oversized_length(&self) -> Option<usize> {
        self.parts
            .iter()
            .map(String::len)
            .find(|length| *length > MAX_PATCH_EVENT_BYTES)
    }

    /// The last commit of the series, which is the tip of the branch.
    pub(crate) fn tip_commit(&self) -> Option<Sha1Hash> {
        self.parts
            .last()
            .and_then(|part| patch_current_commit(part))
            .and_then(|hex| hex.parse::<Sha1Hash>().ok())
    }

    pub(crate) fn commit_of(&self, index: usize) -> Option<&str> {
        self.parts
            .get(index)
            .and_then(|part| patch_current_commit(part))
            .filter(|hex| hex.len() == 40)
    }
}

/// The `From <commit>` header on the first line.
fn patch_current_commit(patch: &str) -> Option<&str> {
    let line = patch.lines().next()?;
    let hex = line.strip_prefix("From ")?;
    hex.split_whitespace().next().filter(|hex| hex.len() == 40)
}

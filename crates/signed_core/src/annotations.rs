use nostr::prelude::*;

/// GitWorkshop and `ngit` cover-note extension, kind 1624.
///
/// A markdown note attached to an issue, patch or PR by its author or a maintainer,
/// not part of the NIP-34 draft, read support for interop.
pub const COVER_NOTE_KIND: Kind = Kind::Custom(1624);

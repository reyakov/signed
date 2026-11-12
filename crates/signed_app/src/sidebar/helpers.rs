use std::time::{SystemTime, UNIX_EPOCH};

use gpui::{AnyElement, App, IntoElement, SharedString};
use gpui_native_kit::avatar::Avatar;
use gpui_native_kit::common::Sizable as _;
use metadata::ProfileDirectory;
use signed_state::{Nip34Binding, ResolvedLocalRepo};

pub(super) fn pick_banner() -> SharedString {
    let num = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.subsec_nanos())
        .unwrap_or(1)
        % 3
        + 1;
    format!("backgrounds/banner{num}.jpg").into()
}

/// The platform that bound a repository, `"nak"` or `"ngit"`.
pub(super) fn local_platform(binding: &Nip34Binding) -> Option<&'static str> {
    let signals = &binding.signals;

    if signals.nip34_json || signals.nip34_grasp_remote || signals.nip34_state_refs {
        Some("nak")
    } else if signals.nostr_repo_config {
        Some("ngit")
    } else {
        None
    }
}

/// The bound owner's avatar, falling back to the path's pixel avatar when unowned.
pub(super) fn local_avatar(
    entry: &ResolvedLocalRepo,
    directory: &gpui::Entity<ProfileDirectory>,
    cx: &App,
) -> AnyElement {
    let Some(owner) = entry.nip34.as_ref().and_then(|binding| binding.owner) else {
        return Avatar::new(entry.path.to_string_lossy())
            .small()
            .into_any_element();
    };

    let profile = directory.read(cx).profile(&owner, cx);

    Avatar::new(owner)
        .name(profile.name())
        .picture(profile.picture())
        .small()
        .into_any_element()
}

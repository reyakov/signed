use gpui::prelude::*;
use gpui::{AnyElement, App, SharedString, Window, div, px};
use gpui_component::clipboard::Clipboard;
use gpui_component::{ActiveTheme, StyledExt, WindowExt, h_flex, v_flex};
use nostr::prelude::PublicKey;
use signed_core::Announcement;
use signed_state::ProfileStore;
use signed_ui::{UserAvatar, middle_truncate};

/// Open the "About" dialog: every field of the repository's announcement
/// event (NIP-34, kind 30617), as parsed into [`Announcement`].
pub(super) fn open_about_dialog(announcement: Announcement, window: &mut Window, cx: &mut App) {
    window.open_dialog(cx, move |dialog, _window, cx| {
        let announcement = announcement.clone();
        dialog
            .w(px(500.))
            .h(px(600.))
            .keyboard(true)
            .close_button(true)
            .title("About")
            .child(announcement_rows(&announcement, cx))
    });
}

/// The announcement's fields as labeled rows; hex identifiers carry a copy
/// button, multi-value tags one line per value.
fn announcement_rows(announcement: &Announcement, cx: &App) -> AnyElement {
    let mut rows: Vec<AnyElement> = Vec::new();

    rows.push(row(
        "Name",
        text(
            announcement
                .name
                .clone()
                .unwrap_or_else(|| SharedString::from("—")),
        ),
        cx,
    ));

    rows.push(row(
        "Description",
        text(
            announcement
                .description
                .clone()
                .unwrap_or_else(|| SharedString::from("—")),
        ),
        cx,
    ));

    if !announcement.web.is_empty() {
        rows.push(row(
            "Web",
            list(
                "about-web",
                announcement.web.iter().map(|url| url.to_string()),
                cx,
            ),
            cx,
        ));
    }

    if let Some(euc) = &announcement.euc {
        rows.push(row(
            "Earliest Commit",
            copy_value("about-euc", euc.clone(), cx),
            cx,
        ));
    }

    if let Some(upstream) = &announcement.upstream {
        rows.push(row("Upstream", text(upstream.display()), cx));
    }

    if !announcement.hashtags.is_empty() {
        rows.push(row(
            "Hashtags",
            text(SharedString::from(announcement.hashtags.join(", "))),
            cx,
        ));
    }

    if !announcement.clone.is_empty() {
        rows.push(row(
            "Clone URLs",
            list(
                "about-clone",
                announcement.clone.iter().map(|url| url.to_string()),
                cx,
            ),
            cx,
        ));
    }

    if !announcement.relays.is_empty() {
        rows.push(row(
            "Grasp Relays",
            list(
                "about-relays",
                announcement.relays.iter().map(|url| url.to_string()),
                cx,
            ),
            cx,
        ));
    }

    if !announcement.maintainers.is_empty() {
        rows.push(row(
            "Maintainers",
            maintainers(&announcement.maintainers, cx),
            cx,
        ));
    }

    v_flex().gap_3().w_full().children(rows).into_any_element()
}

/// One info row: a small muted label above the value.
fn row(label: &'static str, value: AnyElement, cx: &App) -> AnyElement {
    v_flex()
        .gap_1()
        .min_w_0()
        .child(
            div()
                .text_xs()
                .font_semibold()
                .text_color(cx.theme().muted_foreground)
                .child(label),
        )
        .child(value)
        .into_any_element()
}

/// Plain text value, wrapping within the dialog.
fn text(value: SharedString) -> AnyElement {
    div()
        .text_sm()
        .w_full()
        .min_w_0()
        .child(value)
        .into_any_element()
}

/// A mono-spaced value with a copy button, for hex identifiers.
fn copy_value(id: &'static str, value: String, cx: &App) -> AnyElement {
    h_flex()
        .gap_2()
        .items_center()
        .min_w_0()
        .child(
            div()
                .font_family(cx.theme().mono_font_family.clone())
                .text_xs()
                .min_w_0()
                .child(SharedString::from(value.clone())),
        )
        .child(Clipboard::new(id).tooltip("Copy").value(value))
        .into_any_element()
}

/// One row per maintainer: avatar and display name (falling back to a
/// shortened npub), with a copy button for the full pubkey.
fn maintainers(maintainers: &[PublicKey], cx: &App) -> AnyElement {
    let profile_store = ProfileStore::global(cx);
    v_flex()
        .gap_2()
        .min_w_0()
        .children(maintainers.iter().map(|pubkey| {
            let profile = profile_store.read(cx).get(pubkey);
            let name = profile.name();
            let picture = profile.picture();

            h_flex()
                .gap_2()
                .items_center()
                .min_w_0()
                .child(UserAvatar::new(name.clone()).picture(picture))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_sm()
                        .child(name),
                )
        }))
        .into_any_element()
}

/// One row per item of a multi-value tag: the value is truncated to a single
/// line, with a copy button that copies the full value.
fn list(id: &'static str, items: impl IntoIterator<Item = String>, cx: &App) -> AnyElement {
    v_flex()
        .gap_2()
        .min_w_0()
        .children(items.into_iter().enumerate().map(|(ix, item)| {
            h_flex()
                .id(ix)
                .h_8()
                .px_2()
                .gap_2()
                .min_w_0()
                .bg(cx.theme().secondary)
                .hover(|this| this.bg(cx.theme().secondary_hover))
                .rounded(cx.theme().radius)
                .text_color(cx.theme().secondary_foreground)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_sm()
                        .child(SharedString::from(middle_truncate(&item, 28, 16))),
                )
                .child(
                    Clipboard::new(format!("{id}-{ix}"))
                        .tooltip("Copy")
                        .value(item),
                )
                .into_any_element()
        }))
        .into_any_element()
}

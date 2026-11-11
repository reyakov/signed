use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Window, div, px,
};
use gpui_native_kit::avatar::Avatar;
use gpui_native_kit::clipboard::ClipboardButton;
use gpui_native_kit::common::{Sizable as _, h_flex, v_flex};
use gpui_native_kit::dialog::Dialog;
use gpui_native_kit::theme::ActiveTheme as _;
use metadata::ProfileDirectory;
use nostr::prelude::PublicKey;
use signed_core::Announcement;
use utils::middle_truncate;

use super::{close_global, open_global};

pub struct AboutDialog {
    announcement: Announcement,
    directory: Entity<ProfileDirectory>,
}

impl AboutDialog {
    pub fn new(
        announcement: Announcement,
        directory: Entity<ProfileDirectory>,
        cx: &mut App,
    ) -> Entity<Self> {
        cx.new(|_| Self {
            announcement,
            directory,
        })
    }
}

impl Render for AboutDialog {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let announcement = self.announcement.clone();
        let directory = self.directory.clone();

        Dialog::new("about-dialog")
            .open(true)
            .min_width(px(500.))
            .on_change(|open, _, cx| {
                if !open {
                    close_global(cx);
                }
            })
            .title("About")
            .content(move |_, cx| announcement_rows(&announcement, &directory, cx))
            .into_any_element()
    }
}

fn announcement_rows(
    announcement: &Announcement,
    directory: &Entity<ProfileDirectory>,
    cx: &App,
) -> AnyElement {
    let mut rows: Vec<AnyElement> = Vec::new();

    rows.push(row(
        "Name",
        text(
            announcement
                .name
                .as_deref()
                .map(SharedString::from)
                .unwrap_or_else(|| SharedString::from("-")),
        ),
        cx,
    ));
    rows.push(row(
        "Description",
        text(
            announcement
                .description
                .as_deref()
                .map(SharedString::from)
                .unwrap_or_else(|| SharedString::from("-")),
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
            maintainers(&announcement.maintainers, directory, cx),
            cx,
        ));
    }

    v_flex().gap_3().w_full().children(rows).into_any_element()
}

fn row(label: &'static str, value: AnyElement, cx: &App) -> AnyElement {
    v_flex()
        .gap_1()
        .min_w_0()
        .child(
            div()
                .text_xs()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(cx.theme().colors.muted_foreground)
                .child(label),
        )
        .child(value)
        .into_any_element()
}

fn text<T>(value: T) -> AnyElement
where
    T: Into<SharedString>,
{
    div()
        .text_sm()
        .w_full()
        .min_w_0()
        .child(value.into())
        .into_any_element()
}

fn copy_value(id: &'static str, value: String, cx: &App) -> AnyElement {
    h_flex()
        .gap_2()
        .items_center()
        .min_w_0()
        .child(
            div()
                .font_family(cx.theme().mono_font)
                .text_xs()
                .min_w_0()
                .child(SharedString::from(value.clone())),
        )
        .child(ClipboardButton::new(id, value))
        .into_any_element()
}

fn maintainers(
    maintainers: &[PublicKey],
    directory: &Entity<ProfileDirectory>,
    cx: &App,
) -> AnyElement {
    v_flex()
        .gap_2()
        .min_w_0()
        .children(maintainers.iter().map(|pubkey| {
            let profile = directory.read(cx).profile(pubkey, cx);
            let name = profile.name();

            h_flex()
                .gap_2()
                .items_center()
                .min_w_0()
                .child(
                    Avatar::new(*pubkey)
                        .name(name.clone())
                        .picture(profile.picture())
                        .small(),
                )
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
                .into_any_element()
        }))
        .into_any_element()
}

fn list(id: &'static str, items: impl IntoIterator<Item = String>, cx: &App) -> AnyElement {
    v_flex()
        .gap_2()
        .min_w_0()
        .children(items.into_iter().enumerate().map(|(ix, item)| {
            h_flex()
                .h_8()
                .px_2()
                .gap_2()
                .min_w_0()
                .bg(cx.theme().colors.secondary)
                .rounded(cx.theme().radius.control)
                .text_color(cx.theme().colors.secondary_foreground)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_sm()
                        .child(SharedString::from(middle_truncate(&item, 28, 16))),
                )
                .child(ClipboardButton::new(
                    SharedString::from(format!("{id}-{ix}")),
                    item,
                ))
                .into_any_element()
        }))
        .into_any_element()
}

/// Opens the read-only repository metadata dialog.
pub fn open(announcement: Announcement, directory: Entity<ProfileDirectory>, cx: &mut App) {
    open_global(AboutDialog::new(announcement, directory, cx), cx);
}

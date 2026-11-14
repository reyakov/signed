use std::collections::HashSet;

use assets::CustomIconName;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, AppContext as _, ClipboardItem, Context, IntoElement, ParentElement as _,
    SharedString, Styled as _, Window, div,
};
use gpui_native_kit::avatar::Avatar;
use gpui_native_kit::button::{Button, ButtonVariant};
use gpui_native_kit::common::{Sizable as _, h_flex, v_flex};
use gpui_native_kit::data::tabs::{Tab, Tabs};
use gpui_native_kit::dropdown_menu::DropdownMenu;
use gpui_native_kit::icons::{Icon, IconRole};
use gpui_native_kit::menu::MenuItem;
use gpui_native_kit::theme::{ActiveTheme as _, TextRole, Typeset as _};
use gpui_native_kit::tooltip::Tooltip;
use signed_state::{Backend, Nip34Kind};

use super::RepoDetailView;
use super::actions;
use super::dialogs;
use super::helpers::{ShareTargets, bound_repo_label, fork_row, repo_display_name};
use crate::issues::IssuesView;
use crate::pull_requests::{NewPullRequestView, PullRequestsView, SendPatchView};

impl RepoDetailView {
    pub(super) fn open_upstream(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(addr) = self
            .announcement(cx)
            .and_then(|announcement| announcement.upstream.as_ref())
            .and_then(|upstream| upstream.addr.clone())
        else {
            return;
        };

        actions::open_repo_panel(&self.dock, &addr, None, window, cx);
    }

    /// Open the dialog that publishes the local repository to NIP-34.
    fn open_init_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(local_path) = self.store.read(cx).path.clone() else {
            return;
        };
        dialogs::init::open(local_path, cx.entity().downgrade(), window, cx);
    }

    pub(super) fn open_pulls(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(addr) = self.store.read(cx).addr().cloned() else {
            return;
        };

        let id = SharedString::from(format!("pulls:{addr}"));
        let dock = self.dock.clone();
        let dock_for_build = dock.clone();
        let store = self.store.clone();

        crate::shared::panels::open(
            &dock,
            id,
            move |_, cx| {
                cx.new(|cx| PullRequestsView::new(dock_for_build.clone(), store.clone(), cx))
            },
            window,
            cx,
        );
    }

    pub(super) fn open_new_pull_request(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(addr) = self.store.read(cx).addr().cloned() else {
            return;
        };

        let id = SharedString::from(format!("new-pr:{addr}"));
        let dock = self.dock.clone();
        let dock_for_build = dock.clone();
        let store = self.store.clone();

        crate::shared::panels::open(
            &dock,
            id.clone(),
            move |window, cx| {
                cx.new(|cx| {
                    NewPullRequestView::new(
                        dock_for_build.clone(),
                        id.clone(),
                        store.clone(),
                        window,
                        cx,
                    )
                })
            },
            window,
            cx,
        );
    }

    pub(super) fn open_send_patch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(addr) = self.store.read(cx).addr().cloned() else {
            return;
        };

        let id = SharedString::from(format!("send-patch:{addr}"));
        let dock = self.dock.clone();
        let dock_for_build = dock.clone();
        let store = self.store.clone();

        crate::shared::panels::open(
            &dock,
            id.clone(),
            move |window, cx| {
                cx.new(|cx| {
                    SendPatchView::new(
                        dock_for_build.clone(),
                        id.clone(),
                        store.clone(),
                        window,
                        cx,
                    )
                })
            },
            window,
            cx,
        );
    }

    pub(super) fn open_issues(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(addr) = self.store.read(cx).addr().cloned() else {
            return;
        };
        let id = SharedString::from(format!("issues:{addr}"));
        let dock = self.dock.clone();
        let dock_for_build = dock.clone();
        let store = self.store.clone();

        crate::shared::panels::open(
            &dock,
            id,
            move |_, cx| cx.new(|cx| IssuesView::new(dock_for_build.clone(), store.clone(), cx)),
            window,
            cx,
        );
    }

    pub(super) fn render_header(&mut self, cx: &mut Context<Self>) -> AnyElement {
        if self.store.read(cx).addr().is_none() {
            return self.render_local_header(cx);
        }

        let store = self.store.read(cx);
        let pushing = store.pushing;

        let Some(announcement) = store.announcement.clone() else {
            return div().into_any_element();
        };

        let share = ShareTargets::from_announcement(&announcement);
        let name = repo_display_name(self.store.read(cx));
        let description = announcement.description();
        let avatar = Avatar::new(format!("{}:{}", announcement.owner, announcement.id));

        let share_naddr = share.naddr.clone();
        let share_for_menu = share.clone();
        let announcement_for_menu = announcement.clone();
        let weak_share = cx.entity().downgrade();
        let weak_menu_open = cx.entity().downgrade();
        let weak_menu_items = cx.entity().downgrade();

        v_flex()
            .flex_none()
            .p_4()
            .w_full()
            .gap_6()
            .border_b_1()
            .border_color(cx.theme().colors.border)
            .child(
                h_flex()
                    .w_full()
                    .gap_4()
                    .items_start()
                    .justify_between()
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_1()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .typeset(TextRole::Headline, cx)
                                    .child(avatar)
                                    .child(name),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .typeset(TextRole::Body, cx)
                                    .text_color(cx.theme().colors.muted_foreground)
                                    .line_clamp(2)
                                    .child(description),
                            )
                            .when_some(fork_row(&announcement, cx), |this, row| this.child(row))
                            .child(
                                h_flex()
                                    .mt_2()
                                    .w_full()
                                    .gap_1()
                                    .child(
                                        div()
                                            .typeset(TextRole::Caption2, cx)
                                            .text_color(cx.theme().colors.muted_foreground)
                                            .child("Maintainers:"),
                                    )
                                    .child(self.render_maintainers(cx)),
                            ),
                    )
                    .child(
                        h_flex()
                            .flex_none()
                            .gap_2()
                            .justify_end()
                            .child(
                                DropdownMenu::new("share")
                                    .icon(Icon::named(IconRole::Copy))
                                    .variant(ButtonVariant::Secondary)
                                    .open(self.share_open)
                                    .on_click(move |_, _, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            share_naddr.clone(),
                                        ));
                                    })
                                    .on_open_change(move |open, _, cx| {
                                        if let Err(error) = weak_share.update(cx, |this, cx| {
                                            this.share_open = open;
                                            cx.notify();
                                        }) {
                                            log::warn!("failed to update share menu: {error}");
                                        }
                                    })
                                    .items(move |_, _| share_for_menu.menu()),
                            )
                            .child(
                                DropdownMenu::new("repo-menu")
                                    .icon(Icon::named(CustomIconName::EllipsisVertical))
                                    .variant(ButtonVariant::Secondary)
                                    .loading(pushing)
                                    .disabled(pushing)
                                    .open(self.repo_menu_open)
                                    .on_open_change(move |open, _, cx| {
                                        if let Err(error) = weak_menu_open.update(cx, |this, cx| {
                                            this.repo_menu_open = open;
                                            cx.notify();
                                        }) {
                                            log::warn!("failed to update repo menu: {error}");
                                        }
                                    })
                                    .items(move |_, cx| {
                                        let owner = Backend::global(cx)
                                            .read(cx)
                                            .current_user()
                                            == Some(announcement_for_menu.owner);

                                        let mut items = vec![MenuItem::new("About").handler({
                                            let weak = weak_menu_items.clone();
                                            let announcement = announcement_for_menu.clone();
                                            move |_, cx| {
                                                if let Err(error) = weak.update(cx, |this, cx| {
                                                    dialogs::about::open(
                                                        announcement.clone(),
                                                        this.directory.clone(),
                                                        cx,
                                                    );
                                                }) {
                                                    log::warn!("failed to open about: {error}");
                                                }
                                            }
                                        })];

                                        if owner {
                                            items.push(MenuItem::new("Republish").handler({
                                                let weak = weak_menu_items.clone();
                                                move |window, cx| {
                                                    if let Err(error) = weak.update(cx, |this, cx| {
                                                        this.push_repository(window, cx);
                                                    }) {
                                                        log::warn!("failed to republish: {error}");
                                                    }
                                                }
                                            }));
                                            items.push(MenuItem::new("Reset remote to local").handler({
                                                let weak = weak_menu_items.clone();
                                                move |window, cx| {
                                                    if let Err(error) = weak.update(cx, |this, cx| {
                                                        this.reset_remote(window, cx);
                                                    }) {
                                                        log::warn!("failed to reset remote: {error}");
                                                    }
                                                }
                                            }));
                                            items.push(MenuItem::separator());
                                            items.push(
                                                MenuItem::new("Delete")
                                                    .destructive(true)
                                                    .handler({
                                                        let weak = weak_menu_items.clone();
                                                        move |window, cx| {
                                                            if let Err(error) =
                                                                weak.update(cx, |this, cx| {
                                                                    this.delete_repository(window, cx);
                                                                })
                                                            {
                                                                log::warn!(
                                                                    "failed to delete repository: {error}"
                                                                );
                                                            }
                                                        }
                                                    }),
                                            );
                                        }

                                        items
                                    }),
                            )
                    ),
            )
            .child(self.render_tabs(cx))
            .into_any_element()
    }

    fn render_local_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let name = repo_display_name(self.store.read(cx));
        let path = self
            .store
            .read(cx)
            .path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_default();
        let avatar = Avatar::new(path.clone());

        let bound = self.store.read(cx).nip34.clone();
        let action = match bound
            .as_ref()
            .filter(|binding| binding.kind == Nip34Kind::Initialized)
        {
            Some(binding) => bound_repo_label(binding, &self.directory, cx),
            None => Button::new("init")
                .icon(Icon::named(CustomIconName::Init))
                .label("Initialize on Nostr")
                .tooltip(Tooltip::text("Publish this repository to Nostr"))
                .on_click(cx.listener(|this, _, window, cx| {
                    this.open_init_dialog(window, cx);
                }))
                .into_any_element(),
        };

        v_flex()
            .flex_none()
            .px_4()
            .pb_4()
            .w_full()
            .gap_6()
            .border_b_1()
            .border_color(cx.theme().colors.border)
            .child(
                h_flex()
                    .w_full()
                    .gap_4()
                    .items_start()
                    .justify_between()
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_1()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .typeset(TextRole::Headline, cx)
                                    .child(avatar)
                                    .child(name),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .typeset(TextRole::Body, cx)
                                    .text_color(cx.theme().colors.muted_foreground)
                                    .child(path),
                            ),
                    )
                    .child(action),
            )
            .child(self.render_tabs(cx))
            .into_any_element()
    }

    fn render_tabs(&self, cx: &mut Context<Self>) -> AnyElement {
        let count = self.history.read(cx).commit_count();
        let muted = cx.theme().colors.muted_foreground;
        let weak = cx.entity().downgrade();
        let commits_label = match count {
            Some(count) => SharedString::from(format!("Commits ({count})")),
            None => SharedString::from("Commits"),
        };

        Tabs::new("repo-tabs")
            .selected(self.active_tab)
            .tabs(move |_, _| {
                vec![
                    Tab::new("files", "Files")
                        .lead(Icon::named(CustomIconName::GitFile).color(muted)),
                    Tab::new("commits", commits_label.clone())
                        .lead(Icon::named(CustomIconName::GitCommit).color(muted)),
                ]
            })
            .on_change(move |index, _, cx| {
                if let Err(error) = weak.update(cx, |this, cx| {
                    this.active_tab = index;
                    cx.notify();
                }) {
                    log::warn!("failed to switch repo tab: {error}");
                }
            })
            .into_any_element()
    }

    fn render_maintainers(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(announcement) = self.announcement(cx) else {
            return div().into_any_element();
        };

        let mut seen = HashSet::new();
        let rest: Vec<_> = announcement
            .maintainers
            .iter()
            .copied()
            .filter(|key| key != &announcement.owner && seen.insert(*key))
            .collect();

        let owner = self.directory.read(cx).profile(&announcement.owner, cx);
        let owner_name = owner.name();

        h_flex()
            .w_full()
            .gap_3()
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        h_flex()
                            .gap_1()
                            .child(
                                Avatar::new(announcement.owner)
                                    .name(owner_name.clone())
                                    .picture(owner.picture())
                                    .small(),
                            )
                            .child(
                                div()
                                    .typeset(TextRole::Caption, cx)
                                    .whitespace_nowrap()
                                    .child(owner_name),
                            ),
                    )
                    .when(!rest.is_empty(), |this| {
                        this.child(
                            div()
                                .typeset(TextRole::Caption, cx)
                                .text_color(cx.theme().colors.muted_foreground)
                                .child(SharedString::from(format!("+{}", rest.len()))),
                        )
                    }),
            )
            .into_any_element()
    }
}

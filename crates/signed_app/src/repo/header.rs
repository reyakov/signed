use std::collections::HashSet;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use signed_state::{Backend, Nip34Kind};
use ui::avatar::Avatar;
use ui::button::{Button, ButtonVariant};
use ui::common::{Sizable, StyledExt as _, h_flex, v_flex};
use ui::dropdown_menu::DropdownMenu;
use ui::icons::{Icon, IconRole};
use ui::menu::MenuItem;
use ui::popover::Popover;
use ui::theme::ActiveTheme as _;
use ui::tooltip::Tooltip;
use utils::relative_time_secs;

use super::helpers::{
    bound_repo_label, clone_group, clone_row, fork_row, nostr_clone_url, repo_display_name,
};
use super::{RepoDetailView, actions, dialogs};

impl RepoDetailView {
    pub(super) fn open_upstream(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(addr) = self
            .announcement(cx)
            .and_then(|announcement| announcement.upstream.as_ref())
            .and_then(|upstream| upstream.addr.clone())
        else {
            return;
        };

        actions::open_repo_panel(&self.workspace, &addr, None, window, cx);
    }

    /// Open the dialog that publishes the local repository to NIP-34.
    fn open_init_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(local_path) = self.store.read(cx).path.clone() else {
            return;
        };
        self.init = Some(dialogs::init::InitRepoDialog::new(
            local_path,
            cx.entity().downgrade(),
            window,
            cx,
        ));
        cx.notify();
    }

    pub(super) fn render_about(&mut self, cx: &mut Context<Self>) -> AnyElement {
        if self.store.read(cx).addr().is_none() {
            return self.render_init_banner(cx);
        }

        let backend = Backend::global(cx);
        let store = self.store.read(cx);
        let pushing = store.pushing;
        let cloning = store.cloning;

        let Some(announcement) = store.announcement.clone() else {
            return div().into_any_element();
        };

        let nip05 = self
            .directory
            .read(cx)
            .profile(&announcement.owner, cx)
            .metadata()
            .nip05
            .clone()
            .filter(|nip05| !nip05.trim().is_empty());

        let nostr_url = nostr_clone_url(&announcement, nip05.as_deref());
        let ngit_command = SharedString::from(format!("git clone {nostr_url}"));
        let nak_command = SharedString::from(format!("nak git clone {nostr_url}"));
        let git_commands: Vec<String> = announcement.clone_urls();

        let name = repo_display_name(self.store.read(cx));
        let description = announcement.description();
        let avatar = Avatar::new(format!("{}:{}", announcement.owner, announcement.id));

        let announcement_for_menu = announcement.clone();
        let weak_menu_open = cx.entity().downgrade();
        let weak_menu_items = cx.entity().downgrade();
        let weak_clone = cx.entity().downgrade();
        let weak_download = cx.entity().downgrade();
        let ngit_for_popover = ngit_command.clone();
        let nak_for_popover = nak_command.clone();
        let git_for_popover = git_commands.clone();

        h_flex()
            .p_6()
            .w_full()
            .gap_6()
            .items_start()
            .justify_between()
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_2()
                    .child(
                        h_flex()
                            .gap_2()
                            .text_sm()
                            .font_semibold()
                            .child(avatar)
                            .child(name),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .text_sm()
                            .text_color(cx.theme().colors.muted_foreground)
                            .line_clamp(3)
                            .child(description),
                    )
                    .when_some(fork_row(&announcement, cx), |this, row| this.child(row)),
            )
            .child(
                h_flex()
                    .flex_none()
                    .gap_2()
                    .justify_end()
                    .child(
                        Popover::new("code-popover")
                            .open(self.clone_open)
                            .on_change(move |open, _, cx| {
                                if let Err(error) = weak_clone.update(cx, |this, cx| {
                                    this.clone_open = open;
                                    cx.notify();
                                }) {
                                    log::warn!("failed to update clone popover: {error}");
                                }
                            })
                            .trigger(move |_, _| {
                                Button::new("code")
                                    .label("Code")
                                    .icon(Icon::named(IconRole::GitClone))
                                    .tooltip(Tooltip::text("Clone repository"))
                                    .loading(cloning)
                                    .disabled(cloning)
                                    .into_any_element()
                            })
                            .content(move |_, cx| {
                                v_flex()
                                    .w(px(440.))
                                    .p_3()
                                    .gap_4()
                                    .child(clone_group(
                                        "Clone with ngit",
                                        "copy-ngit".into(),
                                        &ngit_for_popover,
                                        cx,
                                    ))
                                    .child(clone_group(
                                        "Clone with nak",
                                        "copy-nak".into(),
                                        &nak_for_popover,
                                        cx,
                                    ))
                                    .child(
                                        v_flex()
                                            .gap_1()
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .font_semibold()
                                                    .child("Grasp Servers"),
                                            )
                                            .when(git_for_popover.is_empty(), |this| {
                                                this.child(
                                                    div()
                                                        .text_xs()
                                                        .text_center()
                                                        .text_color(
                                                            cx.theme().colors.muted_foreground,
                                                        )
                                                        .child("No git clone urls."),
                                                )
                                            })
                                            .children(git_for_popover.iter().enumerate().map(
                                                |(ix, command)| {
                                                    clone_row(
                                                        SharedString::from(format!(
                                                            "copy-git-{ix}"
                                                        )),
                                                        command.clone().into(),
                                                        cx,
                                                    )
                                                },
                                            )),
                                    )
                                    .child(
                                        h_flex().justify_end().child(
                                            Button::new("download")
                                                .icon(Icon::named(IconRole::GitClone))
                                                .label("Download")
                                                .on_click({
                                                    let weak = weak_download.clone();
                                                    move |_, window, cx| {
                                                        if let Err(error) =
                                                            weak.update(cx, |this, cx| {
                                                                this.clone_to_folder(window, cx);
                                                            })
                                                        {
                                                            log::warn!("failed to clone: {error}");
                                                        }
                                                    }
                                                }),
                                        ),
                                    )
                                    .into_any_element()
                            }),
                    )
                    .child(
                        DropdownMenu::new("repo-menu")
                            .icon(IconRole::EllipsisVertical)
                            .variant(ButtonVariant::Ghost)
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
                                let owner = backend.read(cx).current_user()
                                    == Some(announcement_for_menu.owner);

                                let mut items = vec![MenuItem::new("About").handler({
                                    let weak = weak_menu_items.clone();
                                    let announcement = announcement_for_menu.clone();
                                    move |_, cx| {
                                        if let Err(error) = weak.update(cx, |this, cx| {
                                            this.about = Some(dialogs::about::AboutDialog::new(
                                                announcement.clone(),
                                                this.directory.clone(),
                                                cx,
                                            ));
                                            cx.notify();
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
                                    items.push(MenuItem::new("Delete").destructive(true).handler(
                                        {
                                            let weak = weak_menu_items.clone();
                                            move |window, cx| {
                                                if let Err(error) = weak.update(cx, |this, cx| {
                                                    this.delete_repository(window, cx);
                                                }) {
                                                    log::warn!("failed to delete: {error}");
                                                }
                                            }
                                        },
                                    ));
                                }

                                items
                            }),
                    ),
            )
            .into_any_element()
    }

    fn render_init_banner(&self, cx: &mut Context<Self>) -> AnyElement {
        let binding = self
            .store
            .read(cx)
            .nip34
            .as_ref()
            .filter(|binding| binding.kind == Nip34Kind::Initialized);

        let path = self
            .store
            .read(cx)
            .path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_default();

        v_flex()
            .flex_none()
            .p_6()
            .h_32()
            .w_full()
            .items_center()
            .justify_center()
            .gap_2()
            .map(|this| match binding {
                Some(binding) => this.child(bound_repo_label(binding, &self.directory, cx)),
                None => this.child(
                    Button::new("init")
                        .icon(Icon::named(IconRole::ArrowUp))
                        .label("Initialize on Nostr")
                        .tooltip(Tooltip::text("Publish this repository to Nostr"))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_init_dialog(window, cx);
                        })),
                ),
            })
            .child(
                div()
                    .min_w_0()
                    .text_xs()
                    .text_color(cx.theme().colors.muted_foreground)
                    .child(path),
            )
            .into_any_element()
    }

    pub(super) fn render_metadata(&self, cx: &mut Context<Self>) -> AnyElement {
        h_flex()
            .px_6()
            .h_12()
            .w_full()
            .gap_4()
            .items_center()
            .justify_between()
            .child(self.render_maintainers(cx))
            .child(self.render_ref_controls(cx))
            .into_any_element()
    }

    fn render_ref_controls(&self, cx: &mut Context<Self>) -> AnyElement {
        h_flex()
            .justify_end()
            .flex_none()
            .gap_2()
            .when_some(self.render_last_commit(cx), |this, commit| {
                this.child(commit)
            })
            .child(self.refs.branch_select.clone())
            .child(self.refs.tag_select.clone())
            .into_any_element()
    }

    fn render_last_commit(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let commit = self.head_commit.as_ref()?;
        let short = commit.id.get(..8).unwrap_or(&commit.id).to_owned();
        let history = self.history.clone();
        let commit_id = commit.id.clone();

        Some(
            h_flex()
                .id("last-commit")
                .h_8()
                .px_2()
                .gap_2()
                .min_w_0()
                .rounded(cx.theme().radius.control)
                .hover(|this| this.bg(cx.theme().colors.muted_hover))
                .tooltip(Tooltip::text("View latest commit"))
                .on_click(move |_, window, cx| {
                    history.update(cx, |history, cx| {
                        history.open_commit_diff(&commit_id, window, cx);
                    });
                })
                .child(Icon::named(IconRole::GitCommit).color(cx.theme().colors.muted_foreground))
                .child(
                    div()
                        .font_family(cx.theme().mono_font)
                        .text_sm()
                        .text_color(cx.theme().colors.muted_foreground)
                        .child(SharedString::from(short)),
                )
                .child(
                    div()
                        .max_w(px(220.))
                        .text_sm()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .child(commit.summary.clone()),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().colors.muted_foreground)
                        .child(relative_time_secs(commit.time)),
                )
                .into_any_element(),
        )
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
            .min_w_0()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Avatar::new(announcement.owner)
                            .name(owner_name.clone())
                            .picture(owner.picture())
                            .small(),
                    )
                    .child(div().text_xs().whitespace_nowrap().child(owner_name)),
            )
            .when(!rest.is_empty(), |this| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().colors.muted_foreground)
                        .child(SharedString::from(format!("+{}", rest.len()))),
                )
            })
            .into_any_element()
    }
}

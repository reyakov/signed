use std::rc::Rc;

use assets::CustomIconName;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, Context, Entity, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Subscription, WeakEntity, Window, div, px,
};
use gpui_native_kit::button::{Button, ButtonSide, ButtonVariant};
use gpui_native_kit::clipboard::ClipboardButton;
use gpui_native_kit::common::{Sizable as _, h_flex, v_flex};
use gpui_native_kit::dropdown_menu::DropdownMenu;
use gpui_native_kit::icons::{Icon, IconRole};
use gpui_native_kit::menu::MenuItem;
use gpui_native_kit::popover::Popover;
use gpui_native_kit::theme::{ActiveTheme as _, TextRole, Typeset as _};
use gpui_native_kit::tooltip::Tooltip;
use metadata::ProfileDirectory;
use signed_state::{Backend, RepoStore, SyncStatusDirectory};

use super::RepoDetailView;
use super::files::{ActivateHandler, RepoFilesView};
use super::helpers::{clone_group, nostr_clone_url};
use crate::shared::widgets::empty_state;

pub(super) struct RepoRightSidebar {
    repo: WeakEntity<RepoDetailView>,
    store: Entity<RepoStore>,
    files: Entity<RepoFilesView>,
    directory: Entity<ProfileDirectory>,
    sync_directory: Entity<SyncStatusDirectory>,
    issues_menu_open: bool,
    pulls_menu_open: bool,
    clone_open: bool,
    _subscriptions: Vec<Subscription>,
}

impl RepoRightSidebar {
    pub(super) fn new(
        repo: WeakEntity<RepoDetailView>,
        store: Entity<RepoStore>,
        files: Entity<RepoFilesView>,
        directory: Entity<ProfileDirectory>,
        sync_directory: Entity<SyncStatusDirectory>,
        cx: &mut Context<Self>,
    ) -> Self {
        // The dock caches the right view, so observe its sources instead of relying on render reads.
        let subscriptions = vec![
            cx.observe(&files, |_, _, cx| cx.notify()),
            cx.observe(&store, |_, _, cx| cx.notify()),
            cx.observe(&sync_directory, |_, _, cx| cx.notify()),
            cx.observe(&directory, |_, _, cx| cx.notify()),
        ];

        Self {
            repo,
            store,
            files,
            directory,
            sync_directory,
            issues_menu_open: false,
            pulls_menu_open: false,
            clone_open: false,
            _subscriptions: subscriptions,
        }
    }

    fn render_issues(&self, count: usize, cx: &mut Context<Self>) -> AnyElement {
        let repo = self.repo.clone();
        let sidebar = cx.entity().downgrade();

        DropdownMenu::new("sidebar-issues")
            .label(format!("Issues {count}"))
            .icon(Icon::named(CustomIconName::GitIssueDone))
            .variant(ButtonVariant::Secondary)
            .small()
            .open(self.issues_menu_open)
            .on_click(move |_, window, cx| {
                if let Err(error) = repo.update(cx, |repo, cx| repo.open_issues(window, cx)) {
                    log::warn!("failed to open issues: {error}");
                }
            })
            .on_open_change(move |open, _, cx| {
                if let Err(error) = sidebar.update(cx, |sidebar, cx| {
                    sidebar.issues_menu_open = open;
                    cx.notify();
                }) {
                    log::warn!("failed to update issues menu: {error}");
                }
            })
            .items({
                let store = self.store.clone();
                move |_, _| {
                    let store = store.clone();
                    vec![MenuItem::new("New issue").handler(move |_, cx| {
                        crate::issues::new_issue::open(store.clone(), cx);
                    })]
                }
            })
            .into_any_element()
    }

    fn render_pulls(&self, count: usize, cx: &mut Context<Self>) -> AnyElement {
        let repo = self.repo.clone();
        let sidebar = cx.entity().downgrade();

        DropdownMenu::new("sidebar-pulls")
            .label(format!("Pull requests {count}"))
            .icon(Icon::named(CustomIconName::GitPullRequest))
            .variant(ButtonVariant::Secondary)
            .small()
            .open(self.pulls_menu_open)
            .on_click(move |_, window, cx| {
                if let Err(error) = repo.update(cx, |repo, cx| repo.open_pulls(window, cx)) {
                    log::warn!("failed to open pull requests: {error}");
                }
            })
            .on_open_change(move |open, _, cx| {
                if let Err(error) = sidebar.update(cx, |sidebar, cx| {
                    sidebar.pulls_menu_open = open;
                    cx.notify();
                }) {
                    log::warn!("failed to update pulls menu: {error}");
                }
            })
            .items({
                let repo = self.repo.clone();
                move |_, _| {
                    let new_pr = repo.clone();
                    let send_patch = repo.clone();
                    vec![
                        MenuItem::new("New pull request").handler(move |window, cx| {
                            if let Err(error) = new_pr.update(cx, |repo, cx| {
                                repo.open_new_pull_request(window, cx);
                            }) {
                                log::warn!("failed to open new pull request: {error}");
                            }
                        }),
                        MenuItem::new("Send patch").handler(move |window, cx| {
                            if let Err(error) =
                                send_patch.update(cx, |repo, cx| repo.open_send_patch(window, cx))
                            {
                                log::warn!("failed to open send patch: {error}");
                            }
                        }),
                    ]
                }
            })
            .into_any_element()
    }

    fn render_sync(&self, available: bool, pushing: bool, _cx: &mut Context<Self>) -> AnyElement {
        let repo = self.repo.clone();

        Button::new("sidebar-sync")
            .icon(IconRole::Refresh)
            .label("Sync")
            .side(ButtonSide::Left)
            .variant(ButtonVariant::Secondary)
            .small()
            .loading(pushing)
            .disabled(pushing || !available)
            .tooltip(if available {
                Tooltip::text("Push local commits to Nostr")
            } else {
                Tooltip::text("No local commits to sync")
            })
            .on_click(move |_, window, cx| {
                if let Err(error) = repo.update(cx, |repo, cx| repo.sync_now(window, cx)) {
                    log::warn!("failed to sync repository: {error}");
                }
            })
            .into_any_element()
    }

    fn render_clone(&self, cloning: bool, cx: &mut Context<Self>) -> AnyElement {
        let Some(announcement) = self.store.read(cx).announcement.clone() else {
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
        let sidebar = cx.entity().downgrade();
        let repo = self.repo.clone();

        Popover::new("sidebar-clone")
            .open(self.clone_open)
            .on_change(move |open, _, cx| {
                if let Err(error) = sidebar.update(cx, |sidebar, cx| {
                    sidebar.clone_open = open;
                    cx.notify();
                }) {
                    log::warn!("failed to update clone popover: {error}");
                }
            })
            .trigger(move |_, _| {
                Button::new("sidebar-clone-trigger")
                    .icon(Icon::named(CustomIconName::GitClone))
                    .label("Clone")
                    .side(ButtonSide::Left)
                    .variant(ButtonVariant::Secondary)
                    .small()
                    .loading(cloning)
                    .disabled(cloning)
                    .into_any_element()
            })
            .content(move |_, cx| {
                let repo = repo.clone();
                v_flex()
                    .w(px(440.))
                    .p_3()
                    .gap_4()
                    .child(clone_group(
                        "Clone with ngit",
                        "copy-ngit",
                        &ngit_command,
                        cx,
                    ))
                    .child(clone_group("Clone with nak", "copy-nak", &nak_command, cx))
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .typeset(TextRole::Caption, cx)
                                    .text_color(cx.theme().colors.muted_foreground)
                                    .child("Grasp Servers"),
                            )
                            .when(git_commands.is_empty(), |this| {
                                this.child(
                                    div()
                                        .typeset(TextRole::Caption, cx)
                                        .child("No git clone urls."),
                                )
                            })
                            .children(git_commands.iter().enumerate().map(|(ix, command)| {
                                ClipboardButton::new(
                                    SharedString::from(format!("copy-sidebar-git-{ix}")),
                                    command.clone(),
                                )
                            })),
                    )
                    .child(
                        h_flex().justify_end().child(
                            Button::new("sidebar-clone-download")
                                .icon(Icon::named(CustomIconName::GitClone))
                                .label("Download")
                                .on_click(move |_, window, cx| {
                                    if let Err(error) =
                                        repo.update(cx, |repo, cx| repo.clone_to_folder(window, cx))
                                    {
                                        log::warn!("failed to clone: {error}");
                                    }
                                }),
                        ),
                    )
                    .into_any_element()
            })
            .into_any_element()
    }
}

impl Render for RepoRightSidebar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let store = self.store.read(cx);
        let addr = store.addr().cloned();
        let pushing = store.pushing;
        let cloning = store.cloning;
        let issues_count = store.issue_count();
        let pulls_count = store.pull_request_count();
        let rows = self.files.read(cx).tree_rows().to_vec();
        let selected = self.files.read(cx).selected_index();
        let files = self.files.downgrade();
        let repo = self.repo.clone();
        let on_activate: ActivateHandler = Rc::new(move |_, cx| {
            if let Err(error) = repo.update(cx, |repo, cx| {
                if repo.active_tab != 0 {
                    repo.active_tab = 0;
                    cx.notify();
                }
            }) {
                log::warn!("failed to switch to files: {error}");
            }
        });

        let sync_available = addr.as_ref().is_some_and(|addr| {
            let user = Backend::global(cx).read(cx).current_user();
            user.is_some_and(|user| store.is_author(&user))
                && self
                    .sync_directory
                    .read(cx)
                    .statuses_of(addr, cx)
                    .iter()
                    .any(|status| status.status.ahead_total > 0)
        });

        let actions = addr.as_ref().map(|_| {
            v_flex()
                .w_full()
                .gap_1()
                .child(self.render_issues(issues_count, cx))
                .child(self.render_pulls(pulls_count, cx))
                .child(self.render_sync(sync_available, pushing, cx))
                .child(self.render_clone(cloning, cx))
                .into_any_element()
        });

        v_flex()
            .size_full()
            .p_2()
            .gap_2()
            .when_some(actions, |this, actions| this.child(actions))
            .child(
                div()
                    .w_full()
                    .px_1()
                    .typeset(TextRole::Headline, cx)
                    .text_color(cx.theme().colors.muted_foreground)
                    .child("Files"),
            )
            .child(div().w_full().h(px(1.)).bg(cx.theme().colors.border))
            .child(div().flex_1().min_h_0().child(if rows.is_empty() {
                empty_state("No files")
            } else {
                RepoFilesView::tree_element(rows, selected, files, Some(on_activate))
            }))
    }
}

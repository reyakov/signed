use std::rc::Rc;
use std::sync::Arc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _, Render, Role,
    SharedString, StatefulInteractiveElement as _, Styled as _, UniformListScrollHandle, Window,
    div, px, rems, uniform_list,
};
use metadata::ProfileDirectory;
use nostr::prelude::PublicKey;
use ui::avatar::Avatar;
use ui::button::{Button, ButtonSide, ButtonVariant};
use ui::common::{Sizable as _, h_flex, v_flex};
use ui::icons::{Icon, IconRole};
use ui::input::{Input, InputState};
use ui::kbd::Kbd;
use ui::popover::Popover;
use ui::separator::Separator;
use ui::theme::ActiveTheme as _;
use ui::tooltip::Tooltip;

use super::{CreateRepository, REPOSITORY_KEY_CONTEXT, SwitchRepository};
use crate::shared::log_dropped;

type OpenHandler = Rc<dyn Fn(usize, &mut Window, &mut App)>;
type CreateHandler = Rc<dyn Fn(&mut Window, &mut App)>;
type RescanHandler = Rc<dyn Fn(&mut App)>;

/// One row of the repository switcher.
#[derive(Clone)]
pub(super) struct RepositoryEntry {
    pub(super) index: usize,
    pub(super) key: SharedString,
    pub(super) label: SharedString,
    pub(super) owner: Option<PublicKey>,
}

impl RepositoryEntry {
    pub(super) fn avatar(&self, directory: &Entity<ProfileDirectory>, cx: &App) -> Avatar {
        let mut avatar = Avatar::new(self.key.clone());
        if let Some(owner) = self.owner {
            let profile = directory.read(cx).profile(&owner, cx);
            avatar = avatar.name(profile.name()).picture(profile.picture());
        }
        avatar.small()
    }
}

/// The titlebar repository launcher: a static trigger and a searchable list.
pub(crate) struct RepositorySwitcher {
    search: Entity<InputState>,
    directory: Entity<ProfileDirectory>,
    entries: Arc<Vec<RepositoryEntry>>,
    open: bool,
    on_open: OpenHandler,
    on_create: CreateHandler,
    on_rescan: RescanHandler,
    scroll_handle: UniformListScrollHandle,
}

impl RepositorySwitcher {
    pub(super) fn new(
        directory: Entity<ProfileDirectory>,
        on_open: impl Fn(usize, &mut Window, &mut App) + 'static,
        on_create: impl Fn(&mut Window, &mut App) + 'static,
        on_rescan: impl Fn(&mut App) + 'static,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = InputState::new(cx);

        Self {
            search,
            directory,
            entries: Arc::new(Vec::new()),
            open: false,
            on_open: Rc::new(on_open),
            on_create: Rc::new(on_create),
            on_rescan: Rc::new(on_rescan),
            scroll_handle: UniformListScrollHandle::new(),
        }
    }

    pub(super) fn set_entries(&mut self, entries: Vec<RepositoryEntry>, cx: &mut Context<Self>) {
        self.entries = Arc::new(entries);
        cx.notify();
    }

    pub(super) fn close(&mut self, cx: &mut Context<Self>) {
        if self.open {
            self.open = false;
            cx.notify();
        }
    }
}

impl Render for RepositorySwitcher {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.theme();
        let weak = cx.entity().downgrade();
        let entries = Arc::clone(&self.entries);
        let search = self.search.clone();
        let directory = self.directory.clone();

        let on_open = self.on_open.clone();
        let on_create = self.on_create.clone();
        let on_rescan = self.on_rescan.clone();

        let row_weak = weak.clone();
        let create_weak = weak.clone();
        let rescan_weak = weak.clone();
        let content_search = search.clone();
        let scroll_handle = self.scroll_handle.clone();

        let change_search = search.clone();
        let change_weak = weak.clone();

        Popover::new("repo-switcher-popover")
            .open(self.open)
            .on_change(move |open, window, cx| {
                let search = change_search.clone();
                log_dropped(change_weak.update(cx, |this, cx| {
                    this.open = open;
                    if open {
                        search.update(cx, |state, cx| {
                            state.set_value_silent("", window, cx);
                        });
                    }
                    cx.notify();
                }));
            })
            .trigger(move |_window: &mut Window, _cx: &mut App| {
                Button::new("repo-switcher")
                    .icon(Icon::named(IconRole::Stack))
                    .variant(ButtonVariant::Ghost)
                    .fully_rounded(true)
                    .tooltip(Tooltip::text("Repositories"))
                    .accessibility_label("Repositories")
                    .into_any_element()
            })
            .content(move |window: &mut Window, cx: &mut App| {
                let focus = content_search.read(cx).focus_handle();
                let query = content_search.read(cx).value().to_lowercase();

                window.defer(cx, move |window, cx| {
                    if !focus.is_focused(window) {
                        window.focus(&focus, cx);
                    }
                });

                let visible: Arc<Vec<RepositoryEntry>> = Arc::new(
                    entries
                        .iter()
                        .filter(|entry| {
                            query.is_empty() || entry.label.to_lowercase().contains(&query)
                        })
                        .cloned()
                        .collect(),
                );

                let row_height = rems(2.0).to_pixels(window.rem_size()) + px(2.);
                let list_height = (row_height * visible.len() as f32).min(px(280.));
                let list_entries = Arc::clone(&visible);
                let list_directory = directory.clone();
                let list_weak = row_weak.clone();
                let list_on_open = on_open.clone();
                let list_scroll_handle = scroll_handle.clone();
                v_flex()
                    .w(px(380.))
                    .p_2()
                    .gap_2()
                    .child(
                        Input::new("repo-switcher-search", content_search.clone())
                            .placeholder("Filter repositories...")
                            .fully_rounded(true)
                            .prefix(
                                Icon::named(IconRole::Search)
                                    .small()
                                    .color(theme.colors.muted_foreground),
                            )
                            .bg(theme.colors.muted)
                            .border_color(theme.colors.muted),
                    )
                    .map(|this| {
                        if visible.is_empty() {
                            this.child(
                                div()
                                    .p_3()
                                    .text_color(theme.colors.muted_foreground)
                                    .text_center()
                                    .text_sm()
                                    .child("No repositories found"),
                            )
                        } else {
                            this.child(
                                uniform_list(
                                    "repo-switcher-list",
                                    visible.len(),
                                    move |range, window, cx| {
                                        range
                                            .filter_map(|row| {
                                                let entry = list_entries.get(row)?;
                                                let index = entry.index;
                                                let shortcut = Kbd::binding_for_action(
                                                    &SwitchRepository { index },
                                                    Some(REPOSITORY_KEY_CONTEXT),
                                                    window,
                                                );
                                                let weak = list_weak.clone();
                                                let on_open = list_on_open.clone();

                                                Some(
                                                    h_flex()
                                                        .id(("repo-switcher-entry", index))
                                                        .role(Role::MenuItem)
                                                        .w_full()
                                                        .h(row_height)
                                                        .px_3()
                                                        .gap_2()
                                                        .rounded(theme.radius.full)
                                                        .text_sm()
                                                        .hover(|row| row.bg(theme.colors.muted))
                                                        .child(entry.avatar(&list_directory, cx))
                                                        .child(
                                                            div()
                                                                .flex_1()
                                                                .min_w_0()
                                                                .child(entry.label.clone()),
                                                        )
                                                        .when_some(shortcut, |row, shortcut| {
                                                            row.child(shortcut)
                                                        })
                                                        .cursor_pointer()
                                                        .on_click(move |_, window, cx| {
                                                            log_dropped(weak.update(
                                                                cx,
                                                                |this, cx| {
                                                                    this.close(cx);
                                                                },
                                                            ));
                                                            on_open(index, window, cx);
                                                        }),
                                                )
                                            })
                                            .collect()
                                    },
                                )
                                .track_scroll(&list_scroll_handle)
                                .h(list_height)
                                .w_full(),
                            )
                        }
                    })
                    .child(Separator::horizontal())
                    .child(
                        Button::new("repo-switcher-create")
                            .variant(ButtonVariant::Ghost)
                            .side(ButtonSide::Left)
                            .full_width(true)
                            .icon(Icon::named(IconRole::CirclePlus))
                            .child(div().flex_1().child("Create Repository"))
                            .when_some(
                                Kbd::binding_for_action(
                                    &CreateRepository,
                                    Some(REPOSITORY_KEY_CONTEXT),
                                    window,
                                ),
                                |button, hint| button.child(hint),
                            )
                            .on_click(move |_, window, cx| {
                                log_dropped(create_weak.update(cx, |this, cx| {
                                    this.close(cx);
                                }));
                                on_create(window, cx);
                            }),
                    )
                    .child(
                        Button::new("repo-switcher-rescan")
                            .variant(ButtonVariant::Ghost)
                            .side(ButtonSide::Left)
                            .full_width(true)
                            .icon(Icon::named(IconRole::Refresh))
                            .label("Rescan Repositories")
                            .on_click(move |_, _, cx| {
                                log_dropped(rescan_weak.update(cx, |this, cx| {
                                    this.close(cx);
                                }));
                                on_rescan(cx);
                            }),
                    )
                    .into_any_element()
            })
    }
}

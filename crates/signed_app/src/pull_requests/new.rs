use std::path::PathBuf;

use assets::CustomIconName;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Subscription, UniformListScrollHandle,
    WeakEntity, Window, div, px, uniform_list,
};
use gpui_native_kit::button::Button;
use gpui_native_kit::combobox::{Combobox, ComboboxState};
use gpui_native_kit::common::{h_flex, v_flex};
use gpui_native_kit::data::tabs::{Tab, Tabs};
use gpui_native_kit::icons::{Icon, IconRole};
use gpui_native_kit::input::{Input, InputState};
use gpui_native_kit::layout::dock::{DockState, Panel};
use gpui_native_kit::menu::Menu;
use gpui_native_kit::scrollbar::Scrollbar;
use gpui_native_kit::spinner::Spinner;
use gpui_native_kit::theme::{ActiveTheme as _, TextRole, Typeset as _};
use metadata::ProfileDirectory;
use signed_state::{CheckoutsStore, RepoStore};

use super::helpers::{ForkCompare, fork_display_name, truncate_label};
use crate::repo::commit_diff::{CommitDiffView, DiffPane};
use crate::repo::helpers::commit_row;
use crate::shared::widgets::empty_state;

mod source;

pub struct NewPullRequestView {
    dock: WeakEntity<DockState>,
    panel_id: SharedString,
    pub(super) store: Entity<RepoStore>,
    repo_name: SharedString,
    pub(super) repo_path: Option<PathBuf>,
    pub(super) branches: Vec<SharedString>,
    pub(super) fork: Option<ForkCompare>,
    pub(super) base: SharedString,
    pub(super) compare: SharedString,
    pub(super) base_select: Entity<ComboboxState<SharedString>>,
    pub(super) compare_select: Entity<ComboboxState<SharedString>>,
    subject: Entity<InputState>,
    description: Entity<InputState>,
    pub(super) merge_base: Option<String>,
    pub(super) commits: Option<Vec<signed_git::FileCommit>>,
    pub(super) loading: bool,
    pub(super) error: Option<SharedString>,
    submitting: bool,
    pub(super) compare_generation: u64,
    pub(super) fork_generation: u64,
    active_tab: usize,
    pub(super) pane: Entity<DiffPane>,
    scroll_handle: UniformListScrollHandle,
    pub(super) tasks: Vec<gpui::Task<Result<(), anyhow::Error>>>,
    directory: Entity<ProfileDirectory>,
    source_open: bool,
    _subscriptions: Vec<Subscription>,
}

impl NewPullRequestView {
    pub fn new(
        dock: WeakEntity<DockState>,
        panel_id: SharedString,
        store: Entity<RepoStore>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let directory = ProfileDirectory::new(cx);
        let repo_name = store.read(cx).name();
        let pane = cx.new(DiffPane::new);

        let weak = cx.entity().downgrade();
        let subject = InputState::new(cx);
        let description = InputState::builder().multiline().build(cx);

        let base_select = cx.new(|cx| {
            ComboboxState::new(
                Combobox::new("pr-base", "Base", Vec::new()).on_change({
                    let weak = weak.clone();
                    move |value, _window, cx| {
                        if let Some(base) = value {
                            crate::shared::log_dropped(weak.update(cx, |this, cx| {
                                if this.base == base {
                                    return;
                                }
                                this.base = base;
                                this.reload_compare(cx);
                            }));
                        }
                    }
                }),
                window,
                cx,
            )
        });

        let compare_select = cx.new(|cx| {
            ComboboxState::new(
                Combobox::new("pr-compare", "Compare", Vec::new()).on_change({
                    let weak = weak.clone();
                    move |value, _window, cx| {
                        if let Some(compare) = value {
                            crate::shared::log_dropped(weak.update(cx, |this, cx| {
                                if this.compare == compare {
                                    return;
                                }
                                this.compare = compare;
                                this.reload_compare(cx);
                            }));
                        }
                    }
                }),
                window,
                cx,
            )
        });

        if let Some(owner) = store.read(cx).announcement.as_ref().map(|a| a.owner) {
            directory.update(cx, |directory, cx| directory.track([owner], cx));
        }

        window.defer(cx, move |window, cx| {
            crate::shared::log_dropped(weak.update(cx, |this, cx| {
                let Some(addr) = this.store.read(cx).addr().cloned() else {
                    return;
                };

                let Some(path) = CheckoutsStore::global(cx)
                    .read(cx)
                    .associations_of(&addr)
                    .into_iter()
                    .next()
                else {
                    return;
                };

                this.apply_folder_path(path, window, cx);
            }));
        });

        Self {
            dock,
            panel_id,
            store,
            repo_name,
            repo_path: None,
            branches: Vec::new(),
            fork: None,
            base: SharedString::default(),
            compare: SharedString::default(),
            base_select,
            compare_select,
            subject,
            description,
            merge_base: None,
            commits: None,
            loading: false,
            error: None,
            submitting: false,
            compare_generation: 0,
            fork_generation: 0,
            active_tab: 0,
            pane,
            scroll_handle: UniformListScrollHandle::new(),
            tasks: Vec::new(),
            directory,
            source_open: false,
            _subscriptions: Vec::new(),
        }
    }

    fn submit(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.submitting || self.loading {
            return;
        }
        let Some(merge_base) = self.merge_base.clone() else {
            return;
        };
        let Some(repo_path) = self.work_path() else {
            return;
        };
        let subject = self.subject.read(cx).value().to_string();
        let description = self.description.read(cx).value().to_string();
        let branch_name = self.compare.to_string();
        let compare_ref = self.compare_ref();
        let store = self.store.clone();
        let dock = self.dock.clone();
        let panel_id = self.panel_id.clone();

        self.submitting = true;
        self.error = None;
        cx.notify();

        let task: gpui::Task<Result<(), anyhow::Error>> = cx.spawn(async move |this, cx| {
            let publish = store.update(cx, |store, cx| {
                store.open_pull_request_from_refs(
                    repo_path,
                    merge_base,
                    compare_ref,
                    (!subject.is_empty()).then_some(subject),
                    description,
                    Some(branch_name),
                    false,
                    cx,
                )
            });
            if let Err(error) = publish.await {
                this.update(cx, |this, cx| {
                    this.submitting = false;
                    this.error = Some(error.to_string().into());
                    cx.notify();
                })?;
                return Ok(());
            }
            this.update(cx, |this, _cx| {
                this.submitting = false;
            })?;
            crate::shared::log_dropped(dock.update(cx, |dock, cx| dock.close_panel(&panel_id, cx)));
            Ok(())
        });
        self.tasks.push(task);
    }

    fn open_commit_diff(&mut self, commit_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repo_path) = self.work_path() else {
            return;
        };
        let id = match self.store.read(cx).addr() {
            Some(addr) => SharedString::from(format!("commit:{addr}:{commit_id}")),
            None => SharedString::from(format!("commit:{}:{commit_id}", repo_path.display())),
        };
        let dock = self.dock.clone();
        let store = self.store.clone();
        let repo_name = self.repo_name.clone();
        let commit_id = commit_id.to_owned();
        crate::shared::panels::open(
            &dock,
            id,
            move |window, cx| {
                cx.new(|cx| {
                    CommitDiffView::new(
                        store.clone(),
                        repo_path.clone(),
                        repo_name.clone(),
                        commit_id.clone(),
                        window,
                        cx,
                    )
                })
            },
            window,
            cx,
        );
    }

    fn submit_enabled(&self, cx: &App) -> bool {
        self.has_source()
            && !self.loading
            && !self.submitting
            && self.merge_base.is_some()
            && self
                .commits
                .as_ref()
                .is_some_and(|commits| !commits.is_empty())
            && !self.subject.read(cx).value().is_empty()
    }

    fn render_compare_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let weak = cx.entity().downgrade();
        let weak_menu = cx.entity().downgrade();
        let source_view = cx.entity().downgrade();
        let associated = self
            .store
            .read(cx)
            .addr()
            .map(|addr| CheckoutsStore::global(cx).read(cx).associations_of(addr))
            .unwrap_or_default();
        let active_path = (self.fork.is_none())
            .then(|| self.repo_path.clone())
            .flatten();
        let candidates = self.fork_candidates(cx);
        let user = signed_state::Backend::global(cx).read(cx).current_user();
        let active_fork = self.fork.as_ref().map(|fork| fork.announcement.addr());
        let can_submit = self.submit_enabled(cx);
        let source_label = match &self.fork {
            Some(fork) => truncate_label(&fork_display_name(&fork.announcement)),
            None => self.repo_path.as_ref().map_or_else(
                || SharedString::from("No source"),
                |path| truncate_label(&path.display().to_string()),
            ),
        };
        let refresh_fork = self.fork.as_ref().map(|fork| fork.announcement.clone());

        h_flex()
            .px_4()
            .py_2()
            .w_full()
            .gap_3()
            .items_end()
            .child(
                v_flex()
                    .gap_1()
                    .child(label("Merge Into", cx))
                    .child(div().w(px(160.)).child(self.base_select.clone())),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(label("Pull From", cx))
                    .child(div().w(px(160.)).child(self.compare_select.clone())),
            )
            .child(
                v_flex().gap_1().child(label("Source", cx)).child(
                    Menu::new("pr-source")
                        .open(self.source_open)
                        .on_change(move |open, _, cx| {
                            crate::shared::log_dropped(weak_menu.update(cx, |this, cx| {
                                this.source_open = open;
                                cx.notify();
                            }));
                        })
                        .trigger(move |_, _| {
                            Button::new("compare-source")
                                .label(source_label.clone())
                                .variant(gpui_native_kit::button::ButtonVariant::Secondary)
                                .into_any_element()
                        })
                        .items(move |_, _| {
                            super::helpers::source_menu_items(
                                source_view.clone(),
                                associated.clone(),
                                active_path.clone(),
                                candidates.clone(),
                                user,
                                active_fork.clone(),
                            )
                        }),
                ),
            )
            .when_some(refresh_fork, |this, fork| {
                this.child(
                    Button::new("refresh-fork")
                        .icon(Icon::named(CustomIconName::Refresh))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.choose_fork(fork.clone(), window, cx);
                        })),
                )
            })
            .child(div().flex_1())
            .child(
                Button::new("create-pr")
                    .label("Create pull request")
                    .icon(Icon::named(IconRole::Plus))
                    .loading(self.submitting)
                    .disabled(!can_submit)
                    .on_click(move |_, window, cx| {
                        crate::shared::log_dropped(
                            weak.update(cx, |this, cx| this.submit(window, cx)),
                        );
                    }),
            )
            .into_any_element()
    }

    fn render_tabs(&self, cx: &mut Context<Self>) -> AnyElement {
        let files = self.pane.read(cx).diff().map_or(0, |diff| diff.files.len());
        let commits = self.commits.as_ref().map_or(0, |commits| commits.len());
        let weak = cx.entity().downgrade();

        Tabs::new("new-pr-tabs")
            .selected(self.active_tab)
            .tabs(move |_, _| {
                vec![
                    Tab::new("files", format!("Files ({files})"))
                        .lead(Icon::named(CustomIconName::GitFile)),
                    Tab::new("commits", format!("Commits ({commits})"))
                        .lead(Icon::named(CustomIconName::GitCommit)),
                ]
            })
            .on_change(move |index, _, cx| {
                crate::shared::log_dropped(weak.update(cx, |this, cx| {
                    this.active_tab = index;
                    cx.notify();
                }));
            })
            .into_any_element()
    }

    fn render_content(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.loading {
            return v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .child(Spinner::new("new-pr-loading"))
                .into_any_element();
        }
        if !self.has_source() {
            return empty_state("Choose a local checkout or an announced fork to compare");
        }
        if self.commits.is_none() && self.error.is_some() {
            return empty_state("Nothing to compare");
        }
        match self.active_tab {
            0 => self.pane.clone().into_any_element(),
            _ => self.render_commits(cx),
        }
    }

    fn render_commits(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(commits) = self.commits.as_ref() else {
            return empty_state("No commits");
        };
        if commits.is_empty() {
            return empty_state("No commits between the branches");
        }
        let commits = commits.clone();
        let view = cx.entity().downgrade();

        v_flex()
            .relative()
            .flex_1()
            .min_h_0()
            .child(
                uniform_list("new-pr-commits", commits.len(), move |range, _, cx| {
                    range
                        .map(|ix| {
                            let commit = commits[ix].clone();
                            let id = commit.id.clone();
                            let view = view.clone();
                            commit_row(
                                ix,
                                &commit,
                                move |window, cx| {
                                    crate::shared::log_dropped(view.update(cx, |this, cx| {
                                        this.open_commit_diff(&id, window, cx);
                                    }));
                                },
                                cx,
                            )
                        })
                        .collect()
                })
                .track_scroll(&self.scroll_handle)
                .size_full(),
            )
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .child(Scrollbar::vertical(
                        "new-pr-commits-scroll",
                        self.scroll_handle.clone(),
                    )),
            )
            .into_any_element()
    }
}

impl Panel for NewPullRequestView {
    fn title(&self, _cx: &gpui::App) -> Option<SharedString> {
        Some(SharedString::from(format!(
            "{}/new-pull-request",
            self.repo_name
        )))
    }

    fn lead(&self, cx: &gpui::App) -> Option<AnyElement> {
        Some(crate::shared::widgets::repo_tab_avatar(
            self.store.read(cx),
            &self.directory,
            cx,
        ))
    }
}

impl Render for NewPullRequestView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .id("new-pr")
            .size_full()
            .child(
                v_flex()
                    .gap_3()
                    .child(self.render_compare_bar(cx))
                    .child(
                        v_flex()
                            .px_4()
                            .gap_2()
                            .child(
                                Input::new("pr-title", self.subject.clone()).placeholder("Title"),
                            )
                            .child(
                                Input::new("pr-description", self.description.clone())
                                    .rows(3)
                                    .placeholder("Describe..."),
                            )
                            .when_some(self.error.clone(), |this, error| {
                                this.child(
                                    div()
                                        .typeset(TextRole::Caption, cx)
                                        .text_color(cx.theme().ramps.destructive.base)
                                        .child(error),
                                )
                            }),
                    )
                    .child(self.render_tabs(cx)),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .child(self.render_content(cx)),
            )
    }
}

fn label(text: &'static str, cx: &App) -> AnyElement {
    div()
        .typeset(TextRole::Caption2, cx)
        .child(text)
        .into_any_element()
}

//! The shared issue and pull-request lists: toolbar, uniform rows, scrollbar
//! overlay, and empty state, plus the scrollable commit list.

use std::rc::Rc;
use std::sync::Arc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, ClickEvent, ElementId, InteractiveElement as _, IntoElement,
    ParentElement as _, Pixels, SharedString, StatefulInteractiveElement as _, Styled as _,
    UniformListScrollHandle, Window, div, px, uniform_list,
};
use nostr::prelude::PublicKey;
use signed_core::RepoStatus;
use signed_git::FileCommit;
use ui::avatar::Avatar;
use ui::common::{Sizable as _, h_flex, v_flex};
use ui::empty_state::EmptyState;
use ui::scrollbar::Scrollbar;
use ui::theme::ActiveTheme as _;
use utils::relative_time_secs;

use crate::repo::helpers::short_id;
use crate::shared::widgets::status_badge;

/// The height of one issue or pull-request row.
pub const ROW_HEIGHT: f32 = 73.;
/// The height of one commit row in the commit list.
pub const COMMIT_ROW_HEIGHT: f32 = 56.;

/// A row's activation handler.
pub type EventHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// One issue or pull-request row's display data and click handler.
pub struct EventRow {
    pub id: ElementId,
    pub status: RepoStatus,
    pub title: SharedString,
    pub author: SharedString,
    pub picture: Option<SharedString>,
    pub pubkey: PublicKey,
    pub short_id: SharedString,
    pub age: SharedString,
    pub on_click: EventHandler,
}

impl EventRow {
    /// Renders one issue or pull-request row.
    fn render(&self, row_height: Pixels, cx: &App) -> AnyElement {
        h_flex()
            .id(self.id.clone())
            .w_full()
            .h(row_height)
            .gap_4()
            .px_4()
            .border_b_1()
            .border_color(cx.theme().colors.border)
            .hover(|this| this.bg(cx.theme().colors.muted_hover))
            .on_click({
                let on_click = self.on_click.clone();
                move |event, window, cx| on_click(event, window, cx)
            })
            .child(status_badge(self.status))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .child(
                        div()
                            .text_base()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(self.title.clone()),
                    )
                    .child(self.meta_row(cx)),
            )
            .into_any_element()
    }

    /// The author, age, and id line under a row's title.
    fn meta_row(&self, cx: &App) -> AnyElement {
        h_flex()
            .gap_2()
            .text_xs()
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        Avatar::new(self.pubkey)
                            .name(self.author.clone())
                            .picture(self.picture.clone())
                            .small(),
                    )
                    .child(div().child(self.author.clone())),
            )
            .child(SharedString::from("opened"))
            .child(
                div()
                    .text_color(cx.theme().colors.muted_foreground)
                    .child(self.short_id.clone()),
            )
            .child(self.age.clone())
            .into_any_element()
    }
}

/// The shared issue and pull-request list,
pub struct EventList {
    id: ElementId,
    toolbar: Option<AnyElement>,
    rows: Vec<EventRow>,
    scroll_handle: UniformListScrollHandle,
    empty_message: SharedString,
}

impl EventList {
    /// Builds a list rendering `rows` under `toolbar`.
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            toolbar: None,
            rows: Vec::new(),
            scroll_handle: UniformListScrollHandle::new(),
            empty_message: "Nothing here".into(),
        }
    }

    /// Replaces the list's scroll handle with the caller's.
    pub fn scroll_handle(mut self, handle: UniformListScrollHandle) -> Self {
        self.scroll_handle = handle;
        self
    }

    /// Sets the toolbar above the rows.
    pub fn toolbar(mut self, toolbar: AnyElement) -> Self {
        self.toolbar = Some(toolbar);
        self
    }

    /// Sets the rendered rows.
    pub fn rows(mut self, rows: Vec<EventRow>) -> Self {
        self.rows = rows;
        self
    }

    /// Sets the empty-state message shown when no rows match.
    pub fn empty_message(mut self, message: impl Into<SharedString>) -> Self {
        self.empty_message = message.into();
        self
    }

    /// Renders the list.
    pub fn render(self, _cx: &App) -> AnyElement {
        let count = self.rows.len();
        let rows = self.rows;
        let row_height = px(ROW_HEIGHT);

        v_flex()
            .relative()
            .flex_1()
            .min_h_0()
            .w_full()
            .children(self.toolbar)
            .when(count > 0, |this| {
                this.child(
                    uniform_list((self.id.clone(), "rows"), count, move |range, _, cx| {
                        range
                            .map(|ix| {
                                let Some(row) = rows.get(ix) else {
                                    return div().into_any_element();
                                };
                                row.render(row_height, cx)
                            })
                            .collect()
                    })
                    .track_scroll(&self.scroll_handle)
                    .size_full(),
                )
                .child(Scrollbar::vertical(
                    (self.id.clone(), "scroll"),
                    self.scroll_handle.clone(),
                ))
            })
            .when(count == 0, |this| {
                this.child(EmptyState::centered(self.empty_message))
            })
            .into_any_element()
    }
}

/// The toolbar above the issue and pull-request lists: a filter and its actions.
pub fn filter_toolbar(cx: &App, children: impl IntoIterator<Item = AnyElement>) -> AnyElement {
    h_flex()
        .px_4()
        .w_full()
        .gap_3()
        .border_b_1()
        .border_color(cx.theme().colors.border)
        .bg(cx.theme().colors.muted)
        .children(children)
        .into_any_element()
}

/// A scrollable commit list: uniform `commit_row`s under a scrollbar overlay.
pub fn commit_list(
    id: &'static str,
    scroll_id: &'static str,
    commits: Arc<Vec<FileCommit>>,
    scroll_handle: UniformListScrollHandle,
    action: impl Fn(usize, &FileCommit, &App) -> Option<AnyElement> + 'static,
    on_click: impl Fn(&FileCommit, &mut Window, &mut App) + Clone + 'static,
) -> AnyElement {
    let count = commits.len();

    v_flex()
        .relative()
        .flex_1()
        .min_h_0()
        .child(
            uniform_list(id, count, move |range, _, cx| {
                range
                    .map(|ix| {
                        let commit = commits[ix].clone();
                        let action = action(ix, &commit, cx);
                        let on_click = on_click.clone();
                        let row_commit = commit.clone();
                        commit_row(
                            ix,
                            &commit,
                            action,
                            move |window, cx| on_click(&row_commit, window, cx),
                            cx,
                        )
                    })
                    .collect()
            })
            .track_scroll(&scroll_handle)
            .size_full(),
        )
        .child(Scrollbar::vertical(scroll_id, scroll_handle))
        .into_any_element()
}

/// One copyable commit summary row: id, subject, author, and age.
pub(crate) fn commit_row(
    ix: usize,
    commit: &FileCommit,
    action: Option<AnyElement>,
    on_click: impl Fn(&mut Window, &mut gpui::App) + 'static,
    cx: &gpui::App,
) -> gpui::AnyElement {
    h_flex()
        .id(ix)
        .px_4()
        .h(px(COMMIT_ROW_HEIGHT))
        .w_full()
        .gap_3()
        .border_b_1()
        .border_color(cx.theme().colors.border)
        .hover(|this| this.bg(cx.theme().colors.muted_hover))
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_1()
                .justify_center()
                .child(
                    h_flex()
                        .gap_2()
                        .overflow_hidden()
                        .child(
                            div()
                                .font_family(cx.theme().mono_font)
                                .text_sm()
                                .text_color(cx.theme().colors.muted_foreground)
                                .child(short_id(&commit.id)),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_sm()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .child(commit.summary.clone()),
                        ),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .text_xs()
                        .text_color(cx.theme().colors.muted_foreground)
                        .child(commit.author.clone())
                        .child(relative_time_secs(commit.time)),
                ),
        )
        .when_some(action, |row, action| row.child(action))
        .on_click(move |_, window, cx| on_click(window, cx))
        .into_any_element()
}

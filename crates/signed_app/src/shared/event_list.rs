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
use ui::theme::ActiveTheme as _;

use crate::repo::helpers::commit_row;
use crate::shared::widgets::{empty_state, scroll_overlay, status_badge};

/// The height of one issue or pull-request row.
pub const ROW_HEIGHT: f32 = 73.;

/// A row's activation handler.
pub type EventHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

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

/// The shared issue and pull-request list: toolbar, uniform rows, scrollbar
/// overlay, and empty state.
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
                                event_row(row, row_height, cx)
                            })
                            .collect()
                    })
                    .track_scroll(&self.scroll_handle)
                    .size_full(),
                )
                .child(scroll_overlay(
                    (self.id.clone(), "scroll"),
                    self.scroll_handle.clone(),
                ))
            })
            .when(count == 0, |this| {
                this.child(empty_state(self.empty_message))
            })
            .into_any_element()
    }
}

/// Renders one issue or pull-request row.
pub fn event_row(row: &EventRow, row_height: Pixels, cx: &App) -> AnyElement {
    h_flex()
        .id(row.id.clone())
        .w_full()
        .h(row_height)
        .gap_4()
        .px_4()
        .border_b_1()
        .border_color(cx.theme().colors.border)
        .hover(|this| this.bg(cx.theme().colors.muted_hover))
        .on_click({
            let on_click = row.on_click.clone();
            move |event, window, cx| on_click(event, window, cx)
        })
        .child(status_badge(row.status))
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
                        .child(row.title.clone()),
                )
                .child(event_meta_row(row, cx)),
        )
        .into_any_element()
}

/// The author, age, and id line under a row's title.
pub fn event_meta_row(row: &EventRow, cx: &App) -> AnyElement {
    h_flex()
        .gap_2()
        .text_xs()
        .child(
            h_flex()
                .gap_1()
                .child(
                    Avatar::new(row.pubkey)
                        .name(row.author.clone())
                        .picture(row.picture.clone())
                        .small(),
                )
                .child(div().child(row.author.clone())),
        )
        .child(SharedString::from("opened"))
        .child(
            div()
                .text_color(cx.theme().colors.muted_foreground)
                .child(row.short_id.clone()),
        )
        .child(row.age.clone())
        .into_any_element()
}

/// A scrollable commit list: uniform `commit_row`s under a scrollbar overlay.
#[allow(clippy::too_many_arguments)]
pub fn commit_list(
    id: &'static str,
    scroll_id: &'static str,
    commits: Arc<Vec<FileCommit>>,
    scroll_handle: UniformListScrollHandle,
    action: impl Fn(usize, &FileCommit, &App) -> Option<AnyElement> + 'static,
    on_click: impl Fn(&FileCommit, &mut Window, &mut App) + Clone + 'static,
    _cx: &App,
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
        .child(scroll_overlay(scroll_id, scroll_handle))
        .into_any_element()
}

use gpui::prelude::*;
use gpui::{AnyElement, App, Context, WeakEntity, div, px};
use gpui_component::scroll::Scrollbar;
use gpui_component::spinner::Spinner;
use gpui_component::{ActiveTheme, Sizable, h_flex, v_flex, v_virtual_list};
use signed_git::FileCommit;
use signed_ui::placeholder;
use utils::relative_time_secs;

use super::RepoDetailView;

/// Height of one commit row in the virtual list.
pub(super) const COMMIT_ROW_HEIGHT: f32 = 56.;

/// One row of the commit list: id, summary, author and relative time.
/// Clicking a row opens the diff of that commit in a new panel.
fn commit_row(
    ix: usize,
    commit: &FileCommit,
    view: &WeakEntity<RepoDetailView>,
    cx: &App,
) -> AnyElement {
    let view = view.clone();
    let id = commit.id.clone();

    h_flex()
        .id(ix)
        .px_4()
        .h(px(COMMIT_ROW_HEIGHT))
        .w_full()
        .gap_3()
        .items_center()
        .border_b(px(1.))
        .border_color(cx.theme().border)
        .hover(|this| this.bg(cx.theme().list_hover))
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_0p5()
                .justify_center()
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .overflow_hidden()
                        .child(
                            div()
                                .font_family(cx.theme().mono_font_family.clone())
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(commit.id.clone()),
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
                        .text_color(cx.theme().muted_foreground)
                        .child(commit.author.clone())
                        .child(relative_time_secs(commit.time)),
                ),
        )
        .on_click(move |_event, window, cx| {
            if let Some(view) = view.upgrade() {
                view.update(cx, |this, cx| this.open_commit_diff(&id, window, cx));
            }
        })
        .into_any_element()
}

impl RepoDetailView {
    /// Full-height body of the Commits tab: all commits in a virtual
    /// list, or a status message while loading / when there are none.
    pub(super) fn render_commits_tab(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(list) = self.all_commits.as_ref() else {
            return if self.loading_all_commits {
                v_flex()
                    .size_full()
                    .items_center()
                    .justify_center()
                    .child(Spinner::new().small())
                    .into_any_element()
            } else {
                placeholder("Failed to load commits", cx)
            };
        };

        if list.commits.is_empty() {
            return placeholder("No commits found", cx);
        }

        // Copy only the values the element tree needs; the list itself is
        // borrowed inside the renderer below instead of being cloned per
        // frame (a full history can be tens of thousands of commits).
        let view = cx.entity().clone();
        let sizes = self.item_sizes.clone();
        let scroll_handle = self.scroll_handle.clone();
        let shown = list.commits.len();
        let total = list.total;

        v_flex()
            .relative()
            .flex_1()
            .w_full()
            .min_h_0()
            .child(
                v_virtual_list(
                    view,
                    "repo-commits",
                    sizes,
                    move |this, range, _window, cx| {
                        let commits = this
                            .all_commits
                            .as_ref()
                            .map(|list| list.commits.as_slice())
                            .unwrap_or(&[]);
                        let view = cx.entity().downgrade();
                        range
                            .map(|ix| commit_row(ix, &commits[ix], &view, cx))
                            .collect()
                    },
                )
                .track_scroll(&scroll_handle)
                .size_full(),
            )
            .when(shown < total, |this| {
                // The history is capped; tell the user the list is truncated.
                this.child(
                    div()
                        .py_2()
                        .w_full()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(format!("Showing {shown} of {total} commits")),
                )
            })
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .child(Scrollbar::vertical(&self.scroll_handle)),
            )
            .into_any_element()
    }
}

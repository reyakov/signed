//! Issues panel: a bottom panel listing every issue of the repository with
//! its title, event id, author, age and status, filterable by status via
//! the header's All/Open/Closed filter.

use std::rc::Rc;

use assets::CustomIconName;
use dock::{BasePanel, DockArea, DockPlacement, Panel, PanelEvent, panel_handle};
use gpui::prelude::*;
use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Focusable, Pixels, Render,
    SharedString, Size, WeakEntity, Window, div, px, relative, size,
};
use gpui_base::Button as BaseButton;
use gpui_component::avatar::Avatar;
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::dialog::{DialogDescription, DialogFooter, DialogHeader, DialogTitle};
use gpui_component::form::{field, v_form};
use gpui_component::input::{Input, InputState, Textarea, TextareaState};
use gpui_component::scroll::Scrollbar;
use gpui_component::{
    ActiveTheme, Icon, Sizable, VirtualListScrollHandle, WindowExt, h_flex, v_flex, v_virtual_list,
};
use nostr::prelude::{Event, EventId};
use signed_core::{RepoStatus, activity_subject};
use signed_state::{ProfileStore, RepoStore};
use utils::relative_time;

use super::helpers::{placeholder, status_badge};
use super::issue_detail::IssueDetailView;
use crate::image_cache::{MAX_IMAGES, image_cache};

/// Height of one issue row in the virtual list: 8px vertical padding
/// (`py_2`) on top and bottom, a 32px title line (`h_8`) and a 24px meta
/// line (`h_6`), plus the 1px bottom border; the row totals 73px. The
/// status chip (`size_7`, 28px) is shorter than the content.
const ISSUE_ROW_HEIGHT: f32 = 73.;

/// Status filter of the issues list, chosen via the header's filter buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IssueFilter {
    /// Every issue, regardless of status.
    All,
    /// Issues whose resolved status is [`RepoStatus::Open`].
    Open,
    /// Issues whose resolved status is [`RepoStatus::Closed`] or
    /// [`RepoStatus::Applied`] (both are "done" states).
    Closed,
}

impl IssueFilter {
    /// Whether `issue` (of `store`) is included by this filter.
    fn matches(self, store: &RepoStore, issue: &Event) -> bool {
        match self {
            Self::All => true,
            Self::Open => store.status_of(issue) == RepoStatus::Open,
            Self::Closed => matches!(
                store.status_of(issue),
                RepoStatus::Closed | RepoStatus::Applied
            ),
        }
    }
}

pub struct IssuesView {
    focus_handle: FocusHandle,
    /// Dock area the issue detail panel is opened in.
    dock_area: WeakEntity<DockArea>,
    /// Repo store holding the issues and their statuses.
    store: Entity<RepoStore>,
    /// Display name of the repository, for the panel title.
    repo_name: SharedString,
    /// Filter selected in the header filter buttons.
    filter: IssueFilter,
    /// Per-row heights of the virtual list.
    item_sizes: Rc<Vec<Size<Pixels>>>,
    /// Number of rows [`Self::item_sizes`] was built for (the filtered
    /// issue count); rebuilt on change.
    issue_len: usize,
    /// Indices into the store's `issues` matching [`Self::filter`], rebuilt
    /// every render; the virtual list renders this slice.
    visible_issues: Vec<usize>,
    /// Virtual list state of the issues list.
    scroll_handle: VirtualListScrollHandle,
}

impl IssuesView {
    pub fn new(
        dock_area: WeakEntity<DockArea>,
        store: Entity<RepoStore>,
        repo_name: SharedString,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            dock_area,
            store,
            repo_name,
            filter: IssueFilter::Open,
            item_sizes: Rc::new(Vec::new()),
            issue_len: 0,
            visible_issues: Vec::new(),
            scroll_handle: VirtualListScrollHandle::new(),
        }
    }

    /// Open the detail panel of `issue_id` at the bottom of the dock area.
    fn open_issue_detail(
        &mut self,
        issue_id: EventId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(dock_area) = self.dock_area.upgrade() else {
            return;
        };

        let panel = cx.new(|cx| IssueDetailView::new(self.store.clone(), issue_id, window, cx));

        dock_area.update(cx, |dock_area, cx| {
            dock_area.add_panel_view(panel_handle(panel), DockPlacement::Center, None, window, cx);
        });
    }

    /// Render one row of the issue list; `ix` is the row index and
    /// `issue_ix` the index of the issue in the store's `issues`.
    fn render_row(&self, ix: usize, issue_ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let issue = &self.store.read(cx).issues[issue_ix];
        let title = activity_subject(issue);
        let id_hex = issue.id.to_hex();
        let profile = ProfileStore::global(cx).read(cx).get(&issue.pubkey);
        let author = profile.name();
        let picture = profile.picture();
        let age = relative_time(issue.created_at);
        let status = self.store.read(cx).status_of(issue);
        let issue_id = issue.id;

        h_flex()
            .id(ix)
            .w_full()
            .gap_4()
            .px_4()
            .py_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .items_start()
            .on_click(cx.listener(move |this, _event, window, cx| {
                this.open_issue_detail(issue_id, window, cx);
            }))
            .child(status_badge(status, cx))
            .child(
                v_flex()
                    .flex_1()
                    .child(
                        div()
                            .h_8()
                            .min_w_0()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .line_clamp(1)
                            .text_sm()
                            .child(title),
                    )
                    .child(
                        h_flex()
                            .h_6()
                            .gap_2()
                            .text_xs()
                            .child(
                                h_flex()
                                    .gap_1()
                                    .child(
                                        Avatar::new()
                                            .name(author.clone())
                                            .when_some(picture, |this, url| this.src(url))
                                            .rounded(cx.theme().radius)
                                            .small(),
                                    )
                                    .child(div().child(author)),
                            )
                            .child(SharedString::from("opened"))
                            .child(
                                div()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(SharedString::from(&id_hex[..8])),
                            )
                            .child(SharedString::from(age)),
                    ),
            )
            .hover(|this| this.bg(cx.theme().list_hover))
            .into_any_element()
    }

    fn render_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let store = self.store.read(cx);
        let (total, open, closed) =
            store
                .issues
                .iter()
                .fold(
                    (0usize, 0usize, 0usize),
                    |(total, open, closed), issue| match store.status_of(issue) {
                        RepoStatus::Open => (total + 1, open + 1, closed),
                        RepoStatus::Closed => (total + 1, open, closed + 1),
                        RepoStatus::Draft | RepoStatus::Applied => (total + 1, open, closed),
                    },
                );

        h_flex()
            .px_4()
            .w_full()
            .gap_3()
            .border_b_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().muted.opacity(0.5))
            .child(
                h_flex()
                    .h_12()
                    .gap_2()
                    .child(
                        BaseButton::new("all")
                            .flex()
                            .items_center()
                            .h_7()
                            .px_2()
                            .gap_1()
                            .child(Icon::new(CustomIconName::GitIssueDone))
                            .child(div().text_sm().child("All"))
                            .child(
                                h_flex()
                                    .justify_center()
                                    .ml_2()
                                    .px_1()
                                    .py_0p5()
                                    .min_w_4()
                                    .text_size(px(8.))
                                    .bg(cx.theme().muted)
                                    .text_color(cx.theme().muted_foreground)
                                    .rounded(cx.theme().radius)
                                    .line_height(relative(1.))
                                    .child(SharedString::from(total.to_string())),
                            )
                            .text_color(cx.theme().button_foreground)
                            .rounded(cx.theme().radius)
                            .hover(|this| this.bg(cx.theme().button_hover))
                            .active(|this| this.bg(cx.theme().button_active))
                            .selected(self.filter == IssueFilter::All)
                            .when(self.filter == IssueFilter::All, |this| {
                                this.bg(cx.theme().button_active)
                            })
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.filter = IssueFilter::All;
                                cx.notify();
                            })),
                    )
                    .child(
                        BaseButton::new("open")
                            .flex()
                            .items_center()
                            .h_7()
                            .px_2()
                            .gap_1()
                            .child(Icon::new(CustomIconName::GitIssueOpen))
                            .child(div().text_sm().child("Open"))
                            .child(
                                h_flex()
                                    .justify_center()
                                    .ml_2()
                                    .px_1()
                                    .py_0p5()
                                    .min_w_4()
                                    .text_size(px(8.))
                                    .bg(cx.theme().muted)
                                    .text_color(cx.theme().muted_foreground)
                                    .rounded(cx.theme().radius)
                                    .line_height(relative(1.))
                                    .child(SharedString::from(open.to_string())),
                            )
                            .text_color(cx.theme().button_foreground)
                            .rounded(cx.theme().radius)
                            .hover(|this| this.bg(cx.theme().button_hover))
                            .selected(self.filter == IssueFilter::Open)
                            .when(self.filter == IssueFilter::Open, |this| {
                                this.bg(cx.theme().button_active)
                            })
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.filter = IssueFilter::Open;
                                cx.notify();
                            })),
                    )
                    .child(
                        BaseButton::new("closed")
                            .flex()
                            .items_center()
                            .h_7()
                            .px_2()
                            .gap_1()
                            .child(Icon::new(CustomIconName::GitIssueClosed))
                            .child(div().text_sm().child("Closed"))
                            .child(
                                h_flex()
                                    .justify_center()
                                    .ml_2()
                                    .px_1()
                                    .py_0p5()
                                    .min_w_4()
                                    .text_size(px(8.))
                                    .bg(cx.theme().muted)
                                    .text_color(cx.theme().muted_foreground)
                                    .rounded(cx.theme().radius)
                                    .line_height(relative(1.))
                                    .child(SharedString::from(closed.to_string())),
                            )
                            .text_color(cx.theme().button_foreground)
                            .rounded(cx.theme().radius)
                            .hover(|this| this.bg(cx.theme().button_hover))
                            .selected(self.filter == IssueFilter::Closed)
                            .when(self.filter == IssueFilter::Closed, |this| {
                                this.bg(cx.theme().button_active)
                            })
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.filter = IssueFilter::Closed;
                                cx.notify();
                            })),
                    ),
            )
            .child(div().flex_1())
            .child(
                BaseButton::new("new")
                    .flex()
                    .items_center()
                    .h_7()
                    .px_2()
                    .gap_1()
                    .child(Icon::new(CustomIconName::CirclePlus))
                    .child(div().text_sm().child("New issue"))
                    .text_color(cx.theme().button_primary_foreground)
                    .rounded(cx.theme().radius)
                    .bg(cx.theme().button_primary)
                    .hover(|this| this.bg(cx.theme().button_primary_hover))
                    .active(|this| this.bg(cx.theme().button_primary_active))
                    .on_click(cx.listener(|this, _event, window, cx| {
                        open_new_issue_dialog(this.store.clone(), window, cx);
                    })),
            )
            .into_any_element()
    }
}

/// Open the "new issue" dialog: a title and a content input that submit
/// through [`RepoStore::open_issue`] when confirmed.
fn open_new_issue_dialog(store: Entity<RepoStore>, window: &mut Window, cx: &mut App) {
    let subject = cx.new(|cx| InputState::new(window, cx).placeholder("Issue title"));
    let content = cx.new(|cx| TextareaState::new(window, cx).placeholder("Describe the issue…"));

    window.open_dialog(cx, move |dialog, _window, _cx| {
        let subject = subject.clone();
        let content = content.clone();
        let store = store.clone();

        dialog
            .keyboard(true)
            .close_button(true)
            .content(move |body, _window, _cx| {
                body.child(
                    DialogHeader::new()
                        .child(DialogTitle::new().child("New issue"))
                        .child(
                            DialogDescription::new()
                                .child("Report a bug, ask a question, or propose a change."),
                        ),
                )
                .child(
                    v_form()
                        .child(
                            field()
                                .label("Title")
                                .required(true)
                                .child(Input::new(&subject)),
                        )
                        .child(
                            field()
                                .label("Content")
                                .child(Textarea::new(&content).h(px(160.))),
                        ),
                )
                .child(
                    DialogFooter::new().justify_end().child(
                        Button::new("submit")
                            .primary()
                            .label("Create issue")
                            .tooltip("Create issue")
                            .on_click({
                                let subject = subject.clone();
                                let content = content.clone();
                                let store = store.clone();

                                move |_event, window, cx| {
                                    let subject = subject.read(cx).value().to_string();
                                    let content = content.read(cx).value().to_string();
                                    let subject = (!subject.is_empty()).then_some(subject);

                                    store.update(cx, |store, cx| {
                                        store.open_issue(subject, content, cx);
                                    });

                                    window.close_dialog(cx);
                                }
                            }),
                    ),
                )
            })
    });
}

impl BasePanel for IssuesView {
    fn panel_name(&self) -> &'static str {
        "issues"
    }
}

impl Panel for IssuesView {
    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().child(SharedString::from(format!("{}/issues", self.repo_name)))
    }
}

impl EventEmitter<PanelEvent> for IssuesView {}

impl Focusable for IssuesView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for IssuesView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let filter = self.filter;

        // Indices of the issues matching the active filter; the virtual
        // list renders this filtered slice.
        self.visible_issues = {
            let store = self.store.read(cx);
            store
                .issues
                .iter()
                .enumerate()
                .filter(|(_, issue)| filter.matches(store, issue))
                .map(|(ix, _)| ix)
                .collect()
        };

        let count = self.visible_issues.len();

        // The virtual list's item count comes from `item_sizes`; rebuild it
        // whenever the filtered issue count changes.
        if count != self.issue_len {
            self.issue_len = count;
            self.item_sizes = Rc::new(vec![size(px(0.), px(ISSUE_ROW_HEIGHT)); count]);
        }

        let sizes = self.item_sizes.clone();
        let scroll_handle = self.scroll_handle.clone();

        v_flex()
            .size_full()
            .image_cache(image_cache("issues", MAX_IMAGES))
            .child(self.render_header(cx))
            .child(
                v_flex()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .when(count > 0, |this| {
                        this.child(
                            v_virtual_list(
                                cx.entity().clone(),
                                "issues",
                                sizes,
                                move |this, range, _window, cx| {
                                    range
                                        .map(|ix| {
                                            let issue = this.visible_issues[ix];
                                            this.render_row(ix, issue, cx)
                                        })
                                        .collect()
                                },
                            )
                            .track_scroll(&scroll_handle)
                            .size_full(),
                        )
                        .child(
                            div()
                                .absolute()
                                .top_0()
                                .left_0()
                                .right_0()
                                .bottom_0()
                                .child(Scrollbar::vertical(&scroll_handle)),
                        )
                    })
                    .when(count == 0, |this| {
                        let message = match filter {
                            IssueFilter::All => "No issues",
                            IssueFilter::Open => "No open issues",
                            IssueFilter::Closed => "No closed issues",
                        };
                        this.child(placeholder(message, cx))
                    }),
            )
            .into_any_element()
    }
}

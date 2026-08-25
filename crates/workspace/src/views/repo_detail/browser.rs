//! File explorer of the repository detail view: the file tree column and the
//! content column (README / file preview), backed by persistent
//! [`TextViewState`]s for markdown documents and persistent [`InputState`]s
//! for code files.

use gpui::prelude::*;
use gpui::{AnyElement, Context, Entity, SharedString, WeakEntity, Window, div, px};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::{Editor, EditorState};
use gpui_component::list::ListItem;
use gpui_component::spinner::Spinner;
use gpui_component::text::{TextView, TextViewState};
use gpui_component::tree::{TreeEntry, TreeState, tree};
use gpui_component::{ActiveTheme, Sizable, StyledExt, h_flex, v_flex};

use super::RepoDetailView;
use super::helpers::{code_language, is_markdown_path, placeholder, tree_row};

/// Width of the file explorer column.
const TREE_WIDTH: f32 = 240.;
/// Files larger than this are not previewed.
pub(super) const MAX_PREVIEW_BYTES: usize = 1024 * 1024;
/// Preview cache caps: at most this many files (or this many text bytes)
/// are kept in memory at once; the oldest previews are evicted beyond that.
pub(super) const MAX_PREVIEWED_FILES: usize = 32;
pub(super) const MAX_PREVIEW_CACHE_BYTES: usize = 8 * 1024 * 1024;

/// Preview state of a browsed file.
pub(super) enum FileContent {
    /// Decodable text content.
    Text(String),
    /// Not valid UTF-8.
    Binary,
    /// Bigger than [`MAX_PREVIEW_BYTES`].
    TooLarge,
    /// Reading failed.
    Failed(String),
}

/// A markdown document loaded into a persistent [`TextViewState`].
///
/// The state is owned by the view rather than created per render (as the
/// stateless `text::markdown` helper does), so it survives branch switches
/// in the content pane. GPUI's keyed element state is dropped as soon as the
/// element is absent for a single frame, which would otherwise re-parse the
/// whole document on the main thread every time the pane switches between
/// the README, a file preview, and the loading spinner.
pub(super) struct MarkdownView {
    /// Source path; `None` means the repository README.
    pub(super) path: Option<SharedString>,
    pub(super) state: Entity<TextViewState>,
}

/// A code file loaded into a persistent [`InputState`], rendered as a
/// disabled (read-only) code editor with syntax highlighting, line numbers
/// and search.
///
/// Same persistence rationale as [`MarkdownView`]: the state lives as long
/// as this view, so re-viewing the same file does not re-parse it, and
/// parsing happens on a background task inside the editor.
pub(super) struct CodeView {
    /// Source path, relative to the worktree root.
    pub(super) path: SharedString,
    pub(super) state: Entity<EditorState>,
}

/// Spinner shown while a document is being loaded/parsed.
fn preview_spinner() -> AnyElement {
    v_flex()
        .size_full()
        .items_center()
        .justify_center()
        .child(Spinner::new().small())
        .into_any_element()
}

impl RepoDetailView {
    /// One row of the file tree: icon + name, indented by depth.
    fn render_tree_item(
        ix: usize,
        entry: &TreeEntry,
        selected: bool,
        view: &WeakEntity<Self>,
    ) -> ListItem {
        let view = view.clone();
        let id = entry.item().id.clone();

        tree_row(ix, entry, selected, move |window, cx| {
            if let Some(view) = view.upgrade() {
                view.update(cx, |this, cx| this.open_file(&id, window, cx));
            }
        })
    }

    /// Left column: the file tree.
    pub(super) fn render_tree_column(
        tree_state: Entity<TreeState>,
        view: WeakEntity<Self>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        v_flex()
            .h_full()
            .w(px(TREE_WIDTH))
            .p_2()
            .flex_none()
            .border_r_1()
            .border_color(cx.theme().border)
            .child(div().flex_1().min_h_0().child(tree(
                &tree_state,
                move |ix, entry, selected, _window, _cx| {
                    Self::render_tree_item(ix, entry, selected, &view)
                },
            )))
    }

    /// Right column: README, selected file preview, or status text.
    pub(super) fn render_content_column(
        &self,
        pane_title: SharedString,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let loading = self.loading;
        let error = self.error.clone();
        let selected_file = self.selected_file.clone();

        let body: AnyElement = if loading {
            v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap_2()
                .child(Spinner::new().small())
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("Cloning repository…"),
                )
                .into_any_element()
        } else if let Some(error) = error {
            v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .p_4()
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(error),
                )
                .into_any_element()
        } else if let Some(path) = selected_file {
            match self.files.get(path.as_ref()) {
                Some(FileContent::Text(_)) => {
                    if is_markdown_path(path.as_ref()) {
                        self.markdown_element(Some(path.as_ref()), cx)
                    } else {
                        self.code_element(path.as_ref(), cx)
                    }
                }
                Some(FileContent::Binary) => placeholder("Binary file — preview not supported", cx),
                Some(FileContent::TooLarge) => placeholder("File is too large to preview", cx),
                Some(FileContent::Failed(message)) => placeholder(message, cx),
                None => preview_spinner(),
            }
        } else if self.readme_name.is_some() {
            self.markdown_element(None, cx)
        } else {
            placeholder("No README found", cx)
        };

        // Latest commit for the current pane: the selected file, or the README
        // while nothing is selected. Computed after the body above, which
        // needs `&mut self`.
        let commit = match &self.selected_file {
            Some(path) => self.commits.get(path.as_ref()),
            None => self
                .readme_name
                .as_ref()
                .and_then(|name| self.commits.get(name.as_ref())),
        };

        v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(
                h_flex()
                    .px_3()
                    .h_9()
                    .gap_2()
                    .bg(cx.theme().muted)
                    .border_b(px(1.))
                    .border_color(cx.theme().border)
                    .text_color(cx.theme().muted_foreground)
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(pane_title),
                    )
                    .when_some(commit, |this, commit| {
                        this.child(
                            h_flex()
                                .flex_1()
                                .gap_1()
                                .child(
                                    Button::new("commit")
                                        .xsmall()
                                        .text()
                                        .label(commit.id.clone()),
                                )
                                .child(
                                    div()
                                        .max_w(px(250.))
                                        .text_xs()
                                        .text_ellipsis()
                                        .whitespace_nowrap()
                                        .child(commit.summary.clone()),
                                ),
                        )
                    }),
            )
            .child(div().id("repo-content").flex_1().min_h_0().child(body))
    }

    /// Load `text` into the persistent markdown TextView state.
    ///
    /// The state is created empty and fed via `push_str`, which parses on a
    /// background task: switching files never blocks the main thread, and
    /// the state lives as long as this view, so re-viewing the same document
    /// does not re-parse it.
    pub(super) fn set_markdown(
        &mut self,
        path: Option<SharedString>,
        text: &str,
        cx: &mut Context<Self>,
    ) {
        let state = cx.new(|cx| TextViewState::markdown("", cx));
        state.update(cx, |state, cx| state.push_str(text, cx));
        self.md = Some(MarkdownView { path, state });
    }

    /// The persistent markdown TextView for `path` (`None` = README), or a
    /// spinner while the document is being loaded/parsed.
    fn markdown_element(&self, path: Option<&str>, _cx: &mut Context<Self>) -> AnyElement {
        let Some(md) = &self.md else {
            return preview_spinner();
        };
        let ready = match path {
            Some(path) => md.path.as_deref() == Some(path),
            None => md.path.is_none(),
        };
        if !ready {
            return preview_spinner();
        }

        TextView::new(&md.state)
            .selectable(true)
            .scrollable(true)
            .p_4()
            .text_sm()
            .into_any_element()
    }

    /// Load `text` into the persistent code editor state for `path`.
    ///
    /// The state is created in code editor mode so the Input renders it as
    /// a syntax-highlighted, read-only editor. Like [`set_markdown`], the
    /// state lives as long as this view, so re-viewing the same file does
    /// not re-parse it; the tree-sitter parse runs on a background task
    /// inside the editor instead of blocking the main thread.
    pub(super) fn set_code(
        &mut self,
        path: SharedString,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let language = code_language(path.as_ref()).unwrap_or("text");
        let state = cx.new(|cx| {
            EditorState::new(window, cx)
                .language(language)
                .default_value(text)
                .line_number(true)
                .folding(true)
        });
        self.code = Some(CodeView { path, state });
    }

    /// The persistent code editor for `path`, or a spinner while the file is
    /// being loaded/parsed.
    fn code_element(&self, path: &str, _cx: &mut Context<Self>) -> AnyElement {
        let Some(code) = &self.code else {
            return preview_spinner();
        };
        if code.path.as_ref() != path {
            return preview_spinner();
        }

        Editor::new(&code.state)
            .readonly(true)
            .bordered(false)
            .rounded_none()
            .h_full()
            .text_sm()
            .into_any_element()
    }
}

//! Renders a read-only, virtualized view of highlighted source with a
//! line-number gutter, an active-line wash on hover, and drag selection
//! with copy. Rows render only when visible, and the overlay scrollbar is
//! the core one.
//!
//! The view is the state: build a [`CodeViewState`] in a [`gpui::Entity`]
//! and render the entity, then mutate the state to update the view.
//! [`bind_code_view_keys`] installs the selection copy chord once at
//! startup.
//!
//! # Examples
//!
//! ```no_run
//! use gpui::{App, AppContext as _};
//! use ui::code::{CodeViewState, bind_code_view_keys};
//! use ui::highlight::Language;
//!
//! fn source_pane(cx: &mut App) -> gpui::Entity<CodeViewState> {
//!     bind_code_view_keys(cx);
//!     let view = cx.new(|cx| CodeViewState::new("source", "fn main() {}\n", Language::Rust, cx));
//!     view.update(cx, |state, cx| state.set_source("fn hello() {}\n", Language::Rust, cx));
//!     view
//! }
//! ```

use std::rc::Rc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, Bounds, ClipboardItem, Context, ElementId, HighlightStyle, Hsla, InteractiveElement as _,
    IntoElement, KeyBinding, KeyBindingContextPredicate, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ParentElement as _, Pixels, Point, Render, ScrollStrategy,
    SharedString, StatefulInteractiveElement as _, Styled as _, TextStyle, UniformListScrollHandle,
    Window, actions, canvas, div, rems, uniform_list,
};

use crate::components::highlight::{HighlightedCode, Language, Role, highlight};
use crate::components::scrollbar::{Scrollbar, ScrollbarHandle, ScrollbarVisibility};
use crate::components::utils::gutter_cell;
use crate::theme::{
    ActiveTheme as _, CODE_GUTTER_WIDTH, CODE_LINE_HEIGHT, ColorTokens, MONO_ADVANCE_RATIO, Ramps,
    WASH_ALPHA_SOFT,
};

actions!(code_view, [Copy]);

/// The key context the code view declares while it holds focus.
pub const CODE_VIEW_CONTEXT: &str = "CodeView";

/// Binds the code view's copy chord: cmd-c on macOS, ctrl-c elsewhere.
///
/// Call once at startup. Without it, selection copy has no key.
pub fn bind_code_view_keys(cx: &mut App) {
    let context = match KeyBindingContextPredicate::parse(CODE_VIEW_CONTEXT) {
        Ok(context) => Some(Rc::new(context)),
        Err(error) => {
            log::error!("failed to parse code view key context: {error}");
            return;
        }
    };
    let binding = match KeyBinding::load("secondary-c", Box::new(Copy), context, false, None, cx) {
        Ok(binding) => binding,
        Err(error) => {
            log::error!("failed to parse code view copy keystroke: {error}");
            return;
        }
    };
    cx.bind_keys([binding]);
}

/// A point in the source, in lines and characters (not bytes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SelectionPoint {
    /// The zero-based line index.
    pub row: usize,
    /// The zero-based character offset within the line.
    pub col: usize,
}

impl SelectionPoint {
    /// The earlier of two points, in document order.
    fn earlier_than(&self, other: &Self) -> bool {
        (self.row, self.col) <= (other.row, other.col)
    }
}

/// Retained state for a code view.
///
/// Build one in a [`gpui::Entity`], render the entity, and call its setters
/// to update the rendered view.
pub struct CodeViewState {
    id: ElementId,
    source: SharedString,
    language: Language,
    code: HighlightedCode,
    handle: UniformListScrollHandle,
    /// The hovered line, driving the active-line wash and number emphasis.
    active_line: Option<usize>,
    /// `(anchor, head)` in the order they were set.
    selection: Option<(SelectionPoint, SelectionPoint)>,
    /// Whether a drag that started on a row is still extending the selection.
    selecting: bool,
    focus_handle: gpui::FocusHandle,
    a11y_label: Option<SharedString>,
}

impl CodeViewState {
    /// Builds the state, highlighting `source` with `language`.
    pub fn new<I, S>(id: I, source: S, language: Language, cx: &mut App) -> Self
    where
        I: Into<ElementId>,
        S: Into<SharedString>,
    {
        let source = source.into();
        let code = highlight(&source, language);

        Self {
            id: id.into(),
            source,
            language,
            code,
            handle: UniformListScrollHandle::new(),
            active_line: None,
            selection: None,
            selecting: false,
            focus_handle: cx.focus_handle().tab_stop(true),
            a11y_label: None,
        }
    }

    /// Replaces the source and re-highlights it, clearing the selection and
    /// scrolling back to the top.
    pub fn set_source<S>(&mut self, source: S, language: Language, cx: &mut Context<Self>)
    where
        S: Into<SharedString>,
    {
        let source = source.into();

        self.code = highlight(&source, language);
        self.source = source;
        self.language = language;
        self.selection = None;
        self.handle.scroll_to_item(0, ScrollStrategy::Top);

        cx.notify();
    }

    /// Replaces one line's text and re-highlights the file. `line_ix` past
    /// the end clamps to the last line, and the replacement may span
    /// multiple lines.
    ///
    /// # Panics
    ///
    /// Panics if the source is empty, which leaves no line to replace.
    pub fn update_line<T>(&mut self, line_ix: usize, text: T, cx: &mut Context<Self>)
    where
        T: AsRef<str>,
    {
        let trailing_newline = self.source.ends_with('\n');
        let mut lines: Vec<String> = self.source.lines().map(str::to_string).collect();
        let line_ix = line_ix.min(lines.len().saturating_sub(1));

        let replacement: Vec<String> = if text.as_ref().is_empty() {
            vec![String::new()]
        } else {
            text.as_ref().lines().map(str::to_string).collect()
        };

        lines.splice(line_ix..line_ix + 1, replacement);
        self.source = (lines.join("\n") + if trailing_newline { "\n" } else { "" }).into();

        self.code = highlight(&self.source, self.language);
        self.selection = None;
        cx.notify();
    }

    /// The source text the view renders.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// The highlighted lines.
    pub fn code(&self) -> &HighlightedCode {
        &self.code
    }

    /// Scrolls `line` to the top of the view; past-the-end lines clamp to
    /// the last line.
    pub fn scroll_to_line(&self, line: usize) {
        self.handle.scroll_to_item(
            line.min(self.code.lines.len().saturating_sub(1)),
            ScrollStrategy::Top,
        );
    }

    /// The line the pointer last hovered, if any.
    pub fn active_line(&self) -> Option<usize> {
        self.active_line
    }

    /// Selects a range, endpoints in lines and characters.
    pub fn select_range(
        &mut self,
        start: SelectionPoint,
        end: SelectionPoint,
        cx: &mut Context<Self>,
    ) {
        self.selection = if start.earlier_than(&end) {
            Some((start, end))
        } else {
            Some((end, start))
        };
        self.selecting = false;
        cx.notify();
    }

    /// The selected range, endpoints in document order, when there is one.
    pub fn selection(&self) -> Option<(SelectionPoint, SelectionPoint)> {
        self.selection.map(|(anchor, head)| {
            if anchor.earlier_than(&head) {
                (anchor, head)
            } else {
                (head, anchor)
            }
        })
    }

    /// The selected text, or `None` when nothing is selected.
    pub fn selected_text(&self) -> Option<String> {
        let (start, end) = self.selection()?;

        let lines: Vec<&str> = self.source.split('\n').collect();
        let mut selected = Vec::new();

        for row in start.row..=end.row.min(lines.len().saturating_sub(1)) {
            let line_chars: Vec<char> = lines.get(row)?.chars().collect();
            let start_col = if row == start.row {
                start.col.min(line_chars.len())
            } else {
                0
            };

            let end_col = if row == end.row {
                end.col.min(line_chars.len())
            } else {
                line_chars.len()
            };

            selected.push(line_chars[start_col..end_col].iter().collect::<String>());
        }

        (!selected.is_empty()).then(|| selected.join("\n"))
    }

    /// Overrides the accessibility label of the view.
    pub fn accessibility_label<L>(mut self, label: L) -> Self
    where
        L: Into<SharedString>,
    {
        self.a11y_label = Some(label.into());
        self
    }

    /// Maps a window position to a point in the source.
    fn position_at(
        &self,
        bounds: Bounds<Pixels>,
        position: Point<Pixels>,
        scroll_y: Pixels,
        line_height: Pixels,
        char_width: Pixels,
        text_x: Pixels,
    ) -> SelectionPoint {
        // Saturating so an empty document cannot invert the clamp.
        let last_row = self.code.lines.len().saturating_sub(1);
        let row = (((position.y - bounds.origin.y - scroll_y) / line_height).floor() as isize)
            .clamp(0, last_row as isize) as usize;

        let chars = self.code.lines.get(row).map_or(0, |line| {
            line.spans
                .iter()
                .map(|span| span.text.chars().count())
                .sum::<usize>()
        });

        let col = (((position.x - text_x) / char_width).round() as isize).clamp(0, chars as isize)
            as usize;

        SelectionPoint { row, col }
    }
}

impl gpui::Focusable for CodeViewState {
    fn focus_handle(&self, _: &App) -> gpui::FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for CodeViewState {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let colors = theme.colors;
        let mono_font = theme.mono_font;
        let text_size = rems(0.75).to_pixels(window.rem_size());
        let line_height = text_size * CODE_LINE_HEIGHT;
        let char_width = text_size * MONO_ADVANCE_RATIO;
        let text_offset = rems(1.0).to_pixels(window.rem_size());

        let id = self.id.clone();
        let state = cx.entity();
        let line_count = self.code.lines.len();
        let selection = self.selection();
        let gutter_width = CODE_GUTTER_WIDTH;

        let bounds_cell: std::rc::Rc<std::cell::RefCell<Option<Bounds<Pixels>>>> =
            std::rc::Rc::new(std::cell::RefCell::new(None));

        let rows = uniform_list((id.clone(), "rows"), line_count, {
            let id = id.clone();
            let state = state.clone();
            move |range, _window, cx| {
                let code = state.read(cx);
                let active_line = code.active_line;
                let theme = cx.theme();

                range
                    .map(|line_ix| {
                        render_row(
                            code,
                            &id,
                            line_ix,
                            active_line == Some(line_ix),
                            selection,
                            theme.mono_font,
                            theme.colors,
                            theme.ramps,
                            text_size,
                            line_height,
                            cx,
                        )
                    })
                    .collect()
            }
        })
        .track_scroll(&self.handle)
        .h_full()
        .w_full();

        div()
            .id((id.clone(), "view"))
            .relative()
            .size_full()
            .text_xs()
            .font_family(mono_font)
            .text_color(colors.foreground)
            .key_context(CODE_VIEW_CONTEXT)
            .track_focus(&self.focus_handle)
            .on_mouse_down(MouseButton::Left, {
                let state = state.clone();
                let bounds_cell = bounds_cell.clone();
                move |event: &MouseDownEvent, window: &mut Window, cx: &mut App| {
                    window.focus(&state.read(cx).focus_handle.clone(), cx);

                    let bounds = bounds_cell.borrow();
                    let Some(bounds) = bounds.as_ref() else {
                        return;
                    };

                    let text_x = bounds.origin.x + gutter_width + text_offset;
                    let point = {
                        let code = state.read(cx);
                        code.position_at(
                            *bounds,
                            event.position,
                            ScrollbarHandle::offset(&code.handle).y,
                            line_height,
                            char_width,
                            text_x,
                        )
                    };

                    state.update(cx, |state, cx| {
                        if event.modifiers.shift {
                            if let Some((anchor, _)) = state.selection {
                                state.selection = Some((anchor, point));
                            } else {
                                state.selection = Some((point, point));
                            }
                        } else {
                            state.selection = Some((point, point));
                        }
                        state.selecting = true;
                        cx.notify();
                    });
                }
            })
            .on_mouse_move({
                let state = state.clone();
                let bounds_cell = bounds_cell.clone();
                move |event: &MouseMoveEvent, _window: &mut Window, cx: &mut App| {
                    let bounds = bounds_cell.borrow();
                    let Some(bounds) = bounds.as_ref() else {
                        return;
                    };

                    let text_x = bounds.origin.x + gutter_width + text_offset;
                    let point = {
                        let code = state.read(cx);
                        code.position_at(
                            *bounds,
                            event.position,
                            ScrollbarHandle::offset(&code.handle).y,
                            line_height,
                            char_width,
                            text_x,
                        )
                    };

                    let selecting = state.read(cx).selecting;
                    if selecting {
                        let changed = state
                            .read(cx)
                            .selection
                            .map(|(_, head)| head != point)
                            .unwrap_or(true);
                        if changed {
                            state.update(cx, |state, cx| {
                                if let Some((anchor, _)) = state.selection {
                                    state.selection = Some((anchor, point));
                                }
                                cx.notify();
                            });
                        }
                        return;
                    }

                    let changed = state.read(cx).active_line != Some(point.row);
                    if changed {
                        state.update(cx, |state, cx| {
                            state.active_line = Some(point.row);
                            cx.notify();
                        });
                    }
                }
            })
            .on_mouse_up(MouseButton::Left, {
                let state = state.clone();
                move |_: &MouseUpEvent, _window: &mut Window, cx: &mut App| {
                    if state.read(cx).selecting {
                        state.update(cx, |state, cx| {
                            state.selecting = false;
                            cx.notify();
                        });
                    }
                }
            })
            .on_action({
                let state = state.clone();
                move |_: &Copy, _window, cx| {
                    if let Some(text) = state.read(cx).selected_text() {
                        cx.write_to_clipboard(ClipboardItem::new_string(text));
                    }
                }
            })
            .child(
                canvas(
                    {
                        let bounds_cell = bounds_cell.clone();
                        move |bounds, _window, _cx| {
                            *bounds_cell.borrow_mut() = Some(bounds);
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            )
            .child(rows)
            .child(
                Scrollbar::vertical((self.id.clone(), "v-scrollbar"), self.handle.clone())
                    .visibility(ScrollbarVisibility::Platform),
            )
            .when_some(self.a11y_label.clone(), |view, label| {
                view.aria_label(label)
            })
    }
}

#[allow(clippy::too_many_arguments)]
fn render_row(
    code: &CodeViewState,
    id: &ElementId,
    line_ix: usize,
    active: bool,
    selection: Option<(SelectionPoint, SelectionPoint)>,
    mono_font: &'static str,
    colors: ColorTokens,
    ramps: Ramps,
    text_size: Pixels,
    line_height: Pixels,
    cx: &App,
) -> gpui::AnyElement {
    let line = code.code().lines.get(line_ix);

    let mut text = String::new();
    for span in line.map_or(&[][..], |line| line.spans.as_slice()) {
        text.push_str(&span.text);
    }

    let char_count = text.chars().count();
    let mut roles = vec![Role::Text; char_count];
    let mut offset = 0usize;

    for span in line.map_or(&[][..], |line| line.spans.as_slice()) {
        let chars = span.text.chars().count();
        for slot in &mut roles[offset..offset + chars] {
            *slot = span.role;
        }
        offset += chars;
    }

    let mut selected = vec![false; char_count];

    if let Some((start, end)) = selection
        && line_ix >= start.row
        && line_ix <= end.row
    {
        let start_col = if line_ix == start.row {
            start.col.min(char_count)
        } else {
            0
        };

        let end_col = if line_ix == end.row {
            end.col.min(char_count)
        } else {
            char_count
        };

        for slot in &mut selected[start_col..end_col] {
            *slot = true;
        }
    }

    let mut highlights: Vec<(std::ops::Range<usize>, HighlightStyle)> = Vec::new();
    let mut run_start = 0usize;
    let mut run_end = 0usize;
    let mut run_role = Role::Text;
    let mut run_selected = false;

    for (char_ix, (byte_ix, character)) in text.char_indices().enumerate() {
        let fresh = char_ix == 0 || roles[char_ix] != run_role || selected[char_ix] != run_selected;
        if fresh {
            if char_ix > 0 {
                highlights.push((
                    run_start..run_end,
                    run_role.run_style(run_selected, colors, ramps),
                ));
            }
            run_start = byte_ix;
            run_role = roles[char_ix];
            run_selected = selected[char_ix];
        }
        run_end = byte_ix + character.len_utf8();
    }

    if run_end > run_start || !text.is_empty() {
        highlights.push((
            run_start..run_end,
            run_role.run_style(run_selected, colors, ramps),
        ));
    }

    let style = TextStyle {
        color: colors.foreground,
        font_family: SharedString::from(mono_font),
        font_size: text_size.into(),
        line_height: line_height.into(),
        white_space: gpui::WhiteSpace::Nowrap,
        ..Default::default()
    };

    div()
        .debug_selector(move || format!("{id}-row-{line_ix}"))
        .id(("code-row", line_ix))
        .w_full()
        .text_xs()
        .h(line_height)
        .flex()
        .items_center()
        .overflow_hidden()
        .when(active, |row| {
            row.bg(Hsla {
                a: colors.muted.a * WASH_ALPHA_SOFT,
                ..colors.muted
            })
        })
        .child(
            gutter_cell(
                cx,
                if active {
                    colors.foreground
                } else {
                    colors.muted_foreground
                },
                format!("{}", line_ix + 1),
            )
            .justify_end()
            .pr_2()
            .whitespace_nowrap(),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .ml_4()
                .overflow_hidden()
                .whitespace_nowrap()
                .child(gpui::StyledText::new(text).with_default_highlights(&style, highlights)),
        )
        .into_any_element()
}

//! Renders a text field bound to an application-owned [`InputState`].
//!
//! The state is a [`gpui::Entity`] the application owns across frames: read it
//! with [`InputState::value`], write it with [`InputState::set_value`], and
//! subscribe to [`InputStateEvent`] for changes. [`Input`] is the view built
//! from that state each frame. Call [`bind_input_keys`] once at startup to
//! register the editing keybindings.
//!
//! # Examples
//!
//! ```no_run
//! use ui::input::{Input, InputState, bind_input_keys};
//!
//! # let mut cx: gpui::App = unimplemented!();
//! bind_input_keys(&mut cx, None);
//!
//! let state = InputState::builder()
//!     .text("Ada Lovelace")
//!     .build(&mut cx);
//!
//! let field = Input::new("name", state).placeholder("Full name");
//! ```

use std::ops::Range;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::prelude::FluentBuilder as _;
use gpui::{
    Animation, AnimationExt as _, AnyElement, App, AppContext as _, Bounds, ClipboardItem,
    ContentMask, Context, CursorStyle, DefiniteLength, DispatchPhase, ElementId, Entity,
    EntityInputHandler, EventEmitter, Font, HitboxBehavior, InteractiveElement as _, IntoElement,
    KeyBinding, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    ParentElement as _, Pixels, RenderOnce, Role, ScrollDelta, ScrollEvent, SharedString,
    StatefulInteractiveElement as _, StyleRefinement, Styled as _, TextAlign, TextRun,
    UTF16Selection, Window, WrappedLine, actions, canvas, div, fill, point, px,
};
use unicode_segmentation::UnicodeSegmentation;

use crate::components::button::{Button, ButtonVariant};
use crate::components::common::{ControlSizing, Sizable, StyledExt as _, h_flex};
use crate::icons::IconRole;
use crate::theme::{
    ActiveTheme as _, CARET_WIDTH, ColorTokens, DISABLED_OPACITY, Size, UNDERLINE_WIDTH,
    WHEEL_LINE_HEIGHT,
};

/// The key context [`InputBindings`] match against.
pub const INPUT_CONTEXT: &str = "Input";
/// Consecutive edits within this interval merge into one undo entry.
const DEFAULT_GROUP_INTERVAL: Duration = Duration::from_millis(300);
/// How often the focused caret blinks.
const CARET_BLINK_INTERVAL: Duration = Duration::from_millis(500);
/// Pixel height of one scroll "line" for wheel scrolling.
const SCROLL_LINES_HEIGHT: f32 = WHEEL_LINE_HEIGHT;
/// The row count a multiline field shows until the app sets one.
const DEFAULT_ROWS: usize = 4;
/// Maximum undo entries kept; the oldest drops off the front.
const MAX_HISTORY: usize = 100;
/// The character a password field paints instead of each grapheme.
const MASK: &str = "\u{2022}";
/// The byte length of [`MASK`]; the masked display is `graphemes × MASK_LEN`.
const MASK_LEN: usize = 3;

/// Handler for key down events.
type KeyDownHandler = std::rc::Rc<dyn Fn(&KeyDownEvent, &mut Window, &mut App)>;
/// Handler for mouse down events.
type MouseDownHandler = std::rc::Rc<dyn Fn(&MouseDownEvent, &mut Window, &mut App)>;

/// Significant changes in an [`InputState`], observable via `cx.subscribe`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InputStateEvent {
    /// The text content changed: typing, paste, IME commit, or [`InputState::set_value`].
    TextChanged,
    /// The field gained focus.
    Focused,
    /// The field lost focus.
    Blurred,
    /// The field was submitted with Enter.
    Submitted,
}

/// Which keystroke submits a multiline field.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SubmitOn {
    /// Enter inserts a newline; modifier + Enter submits.
    #[default]
    ModifiedEnter,
    /// Enter submits; Shift + Enter inserts a newline.
    Enter,
}

actions!(
    input,
    [
        Escape,
        Submit,
        SecondarySubmit,
        InsertNewline,
        Up,
        Down,
        SelectUp,
        SelectDown,
        Backspace,
        Delete,
        DeleteWordLeft,
        DeleteWordRight,
        DeleteToBeginningOfLine,
        Left,
        Right,
        SelectLeft,
        SelectRight,
        WordLeft,
        WordRight,
        SelectWordLeft,
        SelectWordRight,
        Home,
        End,
        SelectHome,
        SelectEnd,
        SelectAll,
        Copy,
        Cut,
        Paste,
        Undo,
        Redo,
    ]
);

/// The keybindings [`bind_input_keys`] registers on the application.
///
/// A `None` field leaves its action unbound. Start from [`InputBindings::default`]
/// to rebind one action and keep the platform defaults, or from
/// [`InputBindings::empty`] to bind only what you set.
#[derive(Clone, Debug)]
pub struct InputBindings {
    pub escape: Option<KeyBinding>,
    pub submit: Option<KeyBinding>,
    pub secondary_submit: Option<KeyBinding>,
    pub insert_newline: Option<KeyBinding>,
    pub backspace: Option<KeyBinding>,
    pub delete: Option<KeyBinding>,
    pub delete_word_left: Option<KeyBinding>,
    pub delete_word_right: Option<KeyBinding>,
    pub delete_to_beginning_of_line: Option<KeyBinding>,
    pub left: Option<KeyBinding>,
    pub right: Option<KeyBinding>,
    pub up: Option<KeyBinding>,
    pub down: Option<KeyBinding>,
    pub select_left: Option<KeyBinding>,
    pub select_right: Option<KeyBinding>,
    pub select_up: Option<KeyBinding>,
    pub select_down: Option<KeyBinding>,
    pub word_left: Option<KeyBinding>,
    pub word_right: Option<KeyBinding>,
    pub select_word_left: Option<KeyBinding>,
    pub select_word_right: Option<KeyBinding>,
    pub home: Option<KeyBinding>,
    pub end: Option<KeyBinding>,
    pub select_home: Option<KeyBinding>,
    pub select_end: Option<KeyBinding>,
    pub select_all: Option<KeyBinding>,
    pub copy: Option<KeyBinding>,
    pub cut: Option<KeyBinding>,
    pub paste: Option<KeyBinding>,
    pub undo: Option<KeyBinding>,
    pub redo: Option<KeyBinding>,
}

impl Default for InputBindings {
    /// The platform-specific defaults: cmd on macOS, ctrl elsewhere.
    fn default() -> Self {
        let context = Some(INPUT_CONTEXT);

        #[cfg(target_os = "macos")]
        let bindings = Self {
            escape: Some(KeyBinding::new("escape", Escape, context)),
            submit: Some(KeyBinding::new("enter", Submit, context)),
            secondary_submit: Some(KeyBinding::new("cmd-enter", SecondarySubmit, context)),
            insert_newline: Some(KeyBinding::new("shift-enter", InsertNewline, context)),
            backspace: Some(KeyBinding::new("backspace", Backspace, context)),
            delete: Some(KeyBinding::new("delete", Delete, context)),
            delete_word_left: Some(KeyBinding::new("alt-backspace", DeleteWordLeft, context)),
            delete_word_right: Some(KeyBinding::new("alt-delete", DeleteWordRight, context)),
            delete_to_beginning_of_line: Some(KeyBinding::new(
                "cmd-backspace",
                DeleteToBeginningOfLine,
                context,
            )),
            left: Some(KeyBinding::new("left", Left, context)),
            right: Some(KeyBinding::new("right", Right, context)),
            up: Some(KeyBinding::new("up", Up, context)),
            down: Some(KeyBinding::new("down", Down, context)),
            select_left: Some(KeyBinding::new("shift-left", SelectLeft, context)),
            select_right: Some(KeyBinding::new("shift-right", SelectRight, context)),
            select_up: Some(KeyBinding::new("shift-up", SelectUp, context)),
            select_down: Some(KeyBinding::new("shift-down", SelectDown, context)),
            word_left: Some(KeyBinding::new("alt-left", WordLeft, context)),
            word_right: Some(KeyBinding::new("alt-right", WordRight, context)),
            select_word_left: Some(KeyBinding::new("alt-shift-left", SelectWordLeft, context)),
            select_word_right: Some(KeyBinding::new("alt-shift-right", SelectWordRight, context)),
            home: Some(KeyBinding::new("home", Home, context)),
            end: Some(KeyBinding::new("end", End, context)),
            select_home: Some(KeyBinding::new("shift-home", SelectHome, context)),
            select_end: Some(KeyBinding::new("shift-end", SelectEnd, context)),
            select_all: Some(KeyBinding::new("cmd-a", SelectAll, context)),
            copy: Some(KeyBinding::new("cmd-c", Copy, context)),
            cut: Some(KeyBinding::new("cmd-x", Cut, context)),
            paste: Some(KeyBinding::new("cmd-v", Paste, context)),
            undo: Some(KeyBinding::new("cmd-z", Undo, context)),
            redo: Some(KeyBinding::new("cmd-shift-z", Redo, context)),
        };

        #[cfg(not(target_os = "macos"))]
        let bindings = Self {
            escape: Some(KeyBinding::new("escape", Escape, context)),
            submit: Some(KeyBinding::new("enter", Submit, context)),
            secondary_submit: Some(KeyBinding::new("ctrl-enter", SecondarySubmit, context)),
            insert_newline: Some(KeyBinding::new("shift-enter", InsertNewline, context)),
            backspace: Some(KeyBinding::new("backspace", Backspace, context)),
            delete: Some(KeyBinding::new("delete", Delete, context)),
            delete_word_left: Some(KeyBinding::new("ctrl-backspace", DeleteWordLeft, context)),
            delete_word_right: Some(KeyBinding::new("ctrl-delete", DeleteWordRight, context)),
            delete_to_beginning_of_line: Some(KeyBinding::new(
                "ctrl-shift-backspace",
                DeleteToBeginningOfLine,
                context,
            )),
            left: Some(KeyBinding::new("left", Left, context)),
            right: Some(KeyBinding::new("right", Right, context)),
            up: Some(KeyBinding::new("up", Up, context)),
            down: Some(KeyBinding::new("down", Down, context)),
            select_left: Some(KeyBinding::new("shift-left", SelectLeft, context)),
            select_right: Some(KeyBinding::new("shift-right", SelectRight, context)),
            select_up: Some(KeyBinding::new("shift-up", SelectUp, context)),
            select_down: Some(KeyBinding::new("shift-down", SelectDown, context)),
            word_left: Some(KeyBinding::new("ctrl-left", WordLeft, context)),
            word_right: Some(KeyBinding::new("ctrl-right", WordRight, context)),
            select_word_left: Some(KeyBinding::new("ctrl-shift-left", SelectWordLeft, context)),
            select_word_right: Some(KeyBinding::new(
                "ctrl-shift-right",
                SelectWordRight,
                context,
            )),
            home: Some(KeyBinding::new("home", Home, context)),
            end: Some(KeyBinding::new("end", End, context)),
            select_home: Some(KeyBinding::new("shift-home", SelectHome, context)),
            select_end: Some(KeyBinding::new("shift-end", SelectEnd, context)),
            select_all: Some(KeyBinding::new("ctrl-a", SelectAll, context)),
            copy: Some(KeyBinding::new("ctrl-c", Copy, context)),
            cut: Some(KeyBinding::new("ctrl-x", Cut, context)),
            paste: Some(KeyBinding::new("ctrl-v", Paste, context)),
            undo: Some(KeyBinding::new("ctrl-z", Undo, context)),
            redo: Some(KeyBinding::new("ctrl-shift-z", Redo, context)),
        };

        bindings
    }
}

impl InputBindings {
    /// A table with every key unbound, for building custom bindings.
    pub fn empty() -> Self {
        Self {
            escape: None,
            submit: None,
            secondary_submit: None,
            insert_newline: None,
            backspace: None,
            delete: None,
            delete_word_left: None,
            delete_word_right: None,
            delete_to_beginning_of_line: None,
            left: None,
            right: None,
            up: None,
            down: None,
            select_left: None,
            select_right: None,
            select_up: None,
            select_down: None,
            word_left: None,
            word_right: None,
            select_word_left: None,
            select_word_right: None,
            home: None,
            end: None,
            select_home: None,
            select_end: None,
            select_all: None,
            copy: None,
            cut: None,
            paste: None,
            undo: None,
            redo: None,
        }
    }

    /// Collects the table into keybindings.
    pub(crate) fn into_bindings(self) -> Vec<KeyBinding> {
        let mut bindings: Vec<KeyBinding> = [
            self.escape,
            self.submit,
            self.secondary_submit,
            self.insert_newline,
            self.backspace,
            self.delete,
            self.delete_word_left,
            self.delete_word_right,
            self.delete_to_beginning_of_line,
            self.left,
            self.right,
            self.up,
            self.down,
            self.select_left,
            self.select_right,
            self.select_up,
            self.select_down,
            self.word_left,
            self.word_right,
            self.select_word_left,
            self.select_word_right,
            self.home,
            self.end,
            self.select_home,
            self.select_end,
            self.select_all,
            self.copy,
            self.cut,
            self.paste,
            self.undo,
            self.redo,
        ]
        .into_iter()
        .flatten()
        .collect();

        #[cfg(target_os = "macos")]
        {
            let context = Some(INPUT_CONTEXT);
            bindings.extend([
                KeyBinding::new("cmd-left", Home, context),
                KeyBinding::new("cmd-right", End, context),
                KeyBinding::new("cmd-up", Home, context),
                KeyBinding::new("cmd-down", End, context),
                KeyBinding::new("cmd-shift-left", SelectHome, context),
                KeyBinding::new("cmd-shift-right", SelectEnd, context),
            ]);
        }

        bindings
    }
}

/// The shaped text: one wrapped line per buffer line, plus the mask map when masking.
struct ShapedText {
    lines: Vec<ShapedLineEntry>,
    mask_map: Option<Vec<(usize, usize)>>,
}

/// One shaped source line and its first byte's offset in the display text.
struct ShapedLineEntry {
    start: usize,
    line: WrappedLine,
}

/// The field's inner padding.
#[derive(Clone, Copy)]
struct FieldPadding {
    left: Pixels,
    right: Pixels,
    /// Top inset; multiline fields balance it below the last row.
    top: Pixels,
}

/// One visual (wrapped) row: a display-byte range and its y offset from the text origin.
struct VisualRow {
    range: Range<usize>,
    y: Pixels,
    /// Index of the source line this row belongs to.
    line_ix: usize,
}

/// Input layout information for hit testing and caret positioning.
struct InputLayout {
    lines: Vec<ShapedLineEntry>,
    /// The visual rows in order, top to bottom.
    rows: Vec<VisualRow>,
    bounds: Bounds<Pixels>,
    /// Where the text origin (top-left of row 0) sits inside `bounds`.
    origin: gpui::Point<Pixels>,
    /// Maps display offsets to real buffer offsets for masked input.
    mask_map: Option<Vec<(usize, usize)>>,
    /// The height of one visual row.
    line_height: Pixels,
    /// Inner width available to the text.
    text_width: Pixels,
    /// Height of all rows stacked.
    content_height: Pixels,
    /// Height of the region the rows scroll within.
    visible_height: Pixels,
}

impl InputLayout {
    /// The x of a display index relative to its row's start.
    fn row_x(&self, row_ix: usize, display: usize) -> Pixels {
        let row = &self.rows[row_ix];
        let entry = &self.lines[row.line_ix];
        let layout = &entry.line.unwrapped_layout;
        let row_start = row.range.start - entry.start;
        layout.x_for_index(display - entry.start) - layout.x_for_index(row_start)
    }

    /// The row containing `display` and the caret's x relative to the row start.
    fn row_for_index(&self, display: usize) -> Option<(usize, Pixels)> {
        if self.rows.is_empty() {
            return None;
        }
        let row_ix = row_ix_for_display(&self.rows, display);
        let display = display.min(self.rows[row_ix].range.end);
        Some((row_ix, self.row_x(row_ix, display)))
    }

    /// The display index within `row_ix` closest to `x`, which is relative to the row start.
    fn index_at_row_x(&self, row_ix: usize, x: Pixels) -> usize {
        let row = &self.rows[row_ix];
        let entry = &self.lines[row.line_ix];
        let layout = &entry.line.unwrapped_layout;
        let row_start = row.range.start - entry.start;
        layout
            .closest_index_for_x(layout.x_for_index(row_start) + x)
            .clamp(row_start, row.range.end - entry.start)
            + entry.start
    }

    /// The display index at a window point, using the last painted layout.
    fn index_at(&self, position: gpui::Point<Pixels>) -> usize {
        if self.rows.is_empty() {
            return 0;
        }
        let rel_y = (position.y - self.origin.y) / self.line_height;
        let row_ix = (rel_y.floor().max(0.0) as usize).min(self.rows.len() - 1);
        self.index_at_row_x(row_ix, position.x - self.origin.x)
    }

    /// The display range of the row containing `display`.
    fn row_range_at(&self, display: usize) -> Option<Range<usize>> {
        self.rows
            .iter()
            .find(|row| display <= row.range.end)
            .map(|row| row.range.clone())
    }

    /// The overlaps of a display range with each visual row:
    /// `(row index, clipped range, the range continues past the row)`.
    ///
    /// A range that strictly passes an empty row paints that row full width.
    fn row_overlaps(&self, range: Range<usize>) -> Vec<(usize, Range<usize>, bool)> {
        let mut overlaps = Vec::new();
        for (row_ix, row) in self.rows.iter().enumerate() {
            let touches = range.start < row.range.end
                || (row.range.is_empty() && range.start <= row.range.start);
            let beyond = touches && range.end > row.range.end;
            let start = range.start.clamp(row.range.start, row.range.end);
            let end = range.end.clamp(row.range.start, row.range.end);
            if start < end || beyond {
                overlaps.push((row_ix, start..end, beyond));
            }
        }
        overlaps
    }
}

/// History entry for undo/redo operations.
#[derive(Clone, Debug)]
struct HistoryEntry {
    /// The range the replacement occupies after the edit.
    range: Range<usize>,
    /// The text the edit replaced, restored on undo.
    old_text: String,
    /// The length of the replacement.
    new_len: usize,
    /// The selection before the edit.
    cursor: usize,
    anchor: usize,
    /// When the edit happened, for grouping.
    timestamp: Instant,
}

impl HistoryEntry {
    /// Applies this entry in reverse, returning the entry that reverses it back.
    fn reverse(&self, text: &mut String) -> HistoryEntry {
        let end = (self.range.start + self.new_len).min(text.len());
        let removed = text[self.range.start..end].to_string();
        text.replace_range(self.range.start..end, &self.old_text);

        HistoryEntry {
            range: self.range.start..self.range.start + self.old_text.len(),
            old_text: removed,
            new_len: self.old_text.len(),
            cursor: self.cursor,
            anchor: self.anchor,
            timestamp: self.timestamp,
        }
    }
}

/// The application-owned state of a text field that [`Input`] renders.
pub struct InputState {
    text: String,
    /// The caret's byte offset.
    cursor: usize,
    /// The byte offset the selection extends from.
    anchor: usize,
    /// The byte range of in-progress IME composition, if any.
    marked_range: Option<Range<usize>>,
    /// Whether the field had focus at the last paint, for focus transitions.
    focused: bool,
    focus_handle: gpui::FocusHandle,
    /// The key-down handler, bound to this entity with `cx.listener`.
    key_down_handler: KeyDownHandler,
    /// The mouse-down handler, bound to this entity with `cx.listener`.
    mouse_down_handler: MouseDownHandler,
    /// Whether a drag-select is in progress.
    dragging: bool,
    /// Patch history. `redo_stack` holds undone entries awaiting redo.
    undo_stack: Vec<HistoryEntry>,
    redo_stack: Vec<HistoryEntry>,
    /// Consecutive edits within this interval merge into one undo entry.
    group_interval: Duration,
    /// When set, user edits are refused; focus, movement, selection, and copy still work.
    read_only: bool,
    /// When set, the field is a password field.
    password: bool,
    /// Whether a password field currently shows its content.
    revealed: bool,
    /// Whether the field holds multiple lines of text.
    multiline: bool,
    /// Which keystroke submits a multiline field.
    submit_on: SubmitOn,
    /// The x the caret keeps across vertical movement, reset on any other move.
    goal_x: Option<Pixels>,
    /// Content scroll inside the field: x for single-line, y for multiline, clamped at paint.
    scroll_offset: gpui::Point<Pixels>,
    /// Set by every caret-affecting change.
    scroll_to_caret_pending: bool,
    /// Bumped on every edit.
    caret_epoch: usize,
    /// When the current caret epoch started.
    caret_epoch_started: Instant,
    /// The shaped line and element bounds, recorded during each paint.
    layout: Option<InputLayout>,
}

impl EventEmitter<InputStateEvent> for InputState {}

/// Builder for [`InputState`] settings that are fixed at creation.
#[derive(Default)]
pub struct InputStateBuilder {
    text: String,
    multiline: bool,
    password: bool,
    read_only: bool,
}

impl InputStateBuilder {
    /// Sets the initial text, caret at the end.
    pub fn text<T>(mut self, text: T) -> Self
    where
        T: Into<String>,
    {
        self.text = text.into();
        self
    }

    /// Makes the field hold multiple lines.
    pub fn multiline(mut self) -> Self {
        self.multiline = true;
        self
    }

    /// Makes the field a password field.
    pub fn password(mut self) -> Self {
        self.password = true;
        self
    }

    /// Refuses user edits; focus, movement, selection, and copy still work.
    pub fn read_only(mut self) -> Self {
        self.read_only = true;
        self
    }

    /// Creates the state entity.
    pub fn build(self, cx: &mut App) -> Entity<InputState> {
        let InputStateBuilder {
            text,
            multiline,
            password,
            read_only,
        } = self;

        cx.new(|cx| {
            let mut state = InputState::new_inner(cx);
            state.text = sanitize(text, multiline);
            state.cursor = state.text.len();
            state.anchor = state.cursor;
            state.multiline = multiline;
            state.password = password;
            state.read_only = read_only;
            state
        })
    }
}

impl InputState {
    fn new_inner(cx: &mut Context<Self>) -> Self {
        Self {
            text: String::new(),
            cursor: 0,
            anchor: 0,
            marked_range: None,
            focused: false,
            focus_handle: cx.focus_handle(),
            key_down_handler: Rc::new(cx.listener(|state, event: &KeyDownEvent, window, cx| {
                state.handle_key_down(event, window, cx);
            })),
            mouse_down_handler: Rc::new(cx.listener(
                |state, event: &MouseDownEvent, window, cx| {
                    state.handle_mouse_down(event, window, cx);
                },
            )),
            dragging: false,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            group_interval: DEFAULT_GROUP_INTERVAL,
            read_only: false,
            password: false,
            revealed: false,
            multiline: false,
            submit_on: SubmitOn::default(),
            goal_x: None,
            scroll_offset: point(Pixels::ZERO, Pixels::ZERO),
            scroll_to_caret_pending: true,
            caret_epoch: 0,
            caret_epoch_started: Instant::now(),
            layout: None,
        }
    }

    /// Creates the state entity for an application-owned input.
    pub fn new(cx: &mut App) -> Entity<Self> {
        cx.new(Self::new_inner)
    }

    /// Returns a builder for creation-time settings like text, multiline and password.
    pub fn builder() -> InputStateBuilder {
        InputStateBuilder::default()
    }

    /// The current text.
    pub fn value(&self) -> &str {
        &self.text
    }

    /// Replaces the whole value and collapses the selection to the end.
    pub fn set_value<V>(&mut self, value: V, window: &mut Window, cx: &mut Context<Self>)
    where
        V: Into<String>,
    {
        self.set_value_inner(value, true, window, cx);
    }

    /// Like [`Self::set_value`], but does not emit [`InputStateEvent::TextChanged`].
    pub fn set_value_silent<V>(&mut self, value: V, window: &mut Window, cx: &mut Context<Self>)
    where
        V: Into<String>,
    {
        self.set_value_inner(value, false, window, cx);
    }

    fn set_value_inner<V>(
        &mut self,
        value: V,
        emit: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) where
        V: Into<String>,
    {
        let text = sanitize(value.into(), self.multiline);
        if text == self.text {
            return;
        }
        self.text = text;
        self.cursor = self.text.len();
        self.anchor = self.cursor;
        self.marked_range = None;
        self.undo_stack.clear();
        self.redo_stack.clear();
        if emit {
            cx.emit(InputStateEvent::TextChanged);
        }
        self.notify(window, cx);
    }

    /// Sets how long consecutive edits keep merging into one undo entry.
    pub fn set_group_interval(&mut self, interval: Duration) {
        self.group_interval = interval;
    }

    /// Sets whether user edits are refused.
    ///
    /// Focus, movement, selection, and copy still work.
    pub fn set_read_only(&mut self, read_only: bool, cx: &mut Context<Self>) {
        if self.read_only != read_only {
            self.read_only = read_only;
            cx.notify();
        }
    }

    /// Whether user edits are refused.
    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    /// Whether the field is a password field.
    pub fn is_password(&self) -> bool {
        self.password
    }

    /// Sets whether a password field currently shows its content.
    pub fn set_revealed(&mut self, revealed: bool, cx: &mut Context<Self>) {
        if self.revealed != revealed {
            self.revealed = revealed;
            cx.notify();
        }
    }

    /// Whether a password field currently shows its content.
    pub fn is_revealed(&self) -> bool {
        self.revealed
    }

    /// Whether the field holds multiple lines.
    pub fn is_multiline(&self) -> bool {
        self.multiline
    }

    /// Sets which keystroke submits a multiline field.
    pub fn set_submit_on(&mut self, submit_on: SubmitOn, cx: &mut Context<Self>) {
        if self.submit_on != submit_on {
            self.submit_on = submit_on;
            cx.notify();
        }
    }

    // Whether the painted line masks the buffer.
    fn masked(&self) -> bool {
        self.password && !self.revealed
    }

    /// Records `old_text` at `range` being replaced by `new_len` bytes.
    fn push_history(&mut self, range: Range<usize>, old_text: String, new_len: usize) {
        self.redo_stack.clear();

        let now = Instant::now();
        if let Some(last) = self.undo_stack.last_mut() {
            let last_end = last.range.start + last.new_len;

            if self.marked_range.is_some() {
                // A pre-edit step overlapping the previous entry's region.
                if range.start >= last.range.start {
                    last.new_len += new_len;
                    let replaced = range.len();
                    last.new_len -= replaced;
                    last.timestamp = now;
                    return;
                }
            }

            if now.duration_since(last.timestamp) < self.group_interval {
                // Typing that continues right after the previous edit.
                if range.start == last_end {
                    last.new_len += new_len;
                    last.timestamp = now;
                    return;
                }

                // A deletion of text that sat right before the previous edit.
                if range.end == last.range.start && new_len == 0 {
                    last.range.start = range.start;
                    last.old_text = old_text + &last.old_text;
                    last.timestamp = now;
                    return;
                }

                // An edit fully inside the previous edit's replacement.
                if range.start >= last.range.start && range.end <= last_end {
                    last.new_len += new_len;
                    last.new_len -= range.len();
                    last.timestamp = now;
                    return;
                }
            }
        }

        self.undo_stack.push(HistoryEntry {
            range: range.start..range.start + new_len,
            old_text,
            new_len,
            cursor: self.cursor,
            anchor: self.anchor,
            timestamp: now,
        });

        if self.undo_stack.len() > MAX_HISTORY {
            self.undo_stack.remove(0);
        }
    }

    /// The focus handle, for applications that focus the field programmatically.
    pub fn focus_handle(&self) -> gpui::FocusHandle {
        self.focus_handle.clone()
    }

    /// The selected byte range of the buffer.
    fn selection(&self) -> Range<usize> {
        self.cursor.min(self.anchor)..self.cursor.max(self.anchor)
    }

    fn notify(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.notify();
        self.scroll_to_caret_pending = true;
        self.goal_x = None;
        self.caret_epoch += 1;
        self.caret_epoch_started = Instant::now();
        window.notify_text_input_changed(&self.focus_handle);
    }

    /// Scrolls the field by a wheel delta, clamped at paint.
    fn scroll_by(
        &mut self,
        delta: gpui::Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.multiline {
            let next_y = (self.scroll_offset.y + delta.y).max(Pixels::ZERO);
            if next_y == self.scroll_offset.y {
                return self.multiline_scrollable();
            }
            self.scroll_offset.y = next_y;
        } else {
            let next_x = (self.scroll_offset.x - delta.x).max(Pixels::ZERO);
            if next_x == self.scroll_offset.x {
                return false;
            }
            self.scroll_offset.x = next_x;
        }

        let scrollable = self
            .layout
            .as_ref()
            .is_some_and(|layout| layout.content_height > layout.visible_height);

        cx.notify();
        window.notify_text_input_changed(&self.focus_handle);
        self.multiline && scrollable
    }

    /// Whether a multiline field's content overflows vertically.
    fn multiline_scrollable(&self) -> bool {
        self.multiline
            && self
                .layout
                .as_ref()
                .is_some_and(|layout| layout.content_height > layout.visible_height)
    }

    /// Emits `Focused`/`Blurred` when the field's focus state changed since the last paint.
    fn sync_focus(&mut self, focused: bool, cx: &mut Context<Self>) {
        if self.focused == focused {
            return;
        }
        self.focused = focused;

        if focused {
            self.caret_epoch += 1;
            self.caret_epoch_started = Instant::now();
        }

        cx.emit(if focused {
            InputStateEvent::Focused
        } else {
            InputStateEvent::Blurred
        });
    }

    /// Whether the caret should paint.
    pub fn caret_visible(&self, focused: bool) -> bool {
        let elapsed = self.caret_epoch_started.elapsed();
        self.caret_blink_visible_at(focused, elapsed)
    }

    /// Whether the caret should paint `elapsed` after the current caret epoch
    /// started; [`InputState::caret_visible`] passes the real elapsed time.
    pub fn caret_blink_visible_at(&self, focused: bool, elapsed: Duration) -> bool {
        if !focused || self.read_only {
            return false;
        }
        let period = CARET_BLINK_INTERVAL * 2;
        (elapsed.as_millis() as u64 % period.as_millis() as u64)
            < CARET_BLINK_INTERVAL.as_millis() as u64
    }

    /// Replaces a byte range with `text` and collapses the caret to its end.
    fn replace_range(
        &mut self,
        range: Range<usize>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.read_only {
            return;
        }

        if range.is_empty() && text.is_empty() {
            return;
        }

        let text = sanitize(text, self.multiline);
        let old_text = self.text[range.clone()].to_string();
        self.push_history(range.clone(), old_text, text.len());
        self.text.replace_range(range.clone(), &text);
        self.cursor = range.start + text.len();
        self.anchor = self.cursor;
        self.marked_range = None;

        cx.emit(InputStateEvent::TextChanged);
        self.notify(window, cx);
    }

    /// Deletes the selection, or one character/word before the caret.
    fn delete_backward(&mut self, word: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selection().is_empty() {
            let range = self.selection();
            self.replace_range(range, "", window, cx);
        } else if self.cursor > 0 {
            let start = if word {
                previous_word_start(&self.text, self.cursor)
            } else {
                previous_grapheme_start(&self.text, self.cursor)
            };
            self.replace_range(start..self.cursor, "", window, cx);
        }
    }

    /// Deletes the selection, or one character/word after the caret.
    fn delete_forward(&mut self, word: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selection().is_empty() {
            let range = self.selection();
            self.replace_range(range, "", window, cx);
        } else if self.cursor < self.text.len() {
            let end = if word {
                next_word_end(&self.text, self.cursor)
            } else {
                next_grapheme_start(&self.text, self.cursor)
            };
            self.replace_range(self.cursor..end, "", window, cx);
        }
    }

    /// Moves the caret, extending the selection when `extend`.
    fn move_cursor(
        &mut self,
        to: usize,
        extend: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let cursor = to.clamp(0, self.text.len());
        let anchor = if extend { self.anchor } else { cursor };
        if cursor == self.cursor && anchor == self.anchor {
            return;
        }
        self.cursor = cursor;
        self.anchor = anchor;
        self.notify(window, cx);
    }

    /// Selects `range`.
    fn select_range(&mut self, range: Range<usize>, window: &mut Window, cx: &mut Context<Self>) {
        if range.start == self.anchor && range.end == self.cursor {
            return;
        }
        self.anchor = range.start;
        self.cursor = range.end;
        self.notify(window, cx);
    }

    /// Copies the selected text to the clipboard.
    fn copy_selection(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.password {
            return false;
        }
        let range = self.selection();
        if range.is_empty() {
            return false;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(
            self.text[range.clone()].to_string(),
        ));
        true
    }

    /// Deletes the selected range, if any.
    fn delete_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let range = self.selection();
        if !range.is_empty() {
            self.replace_range(range, "", window, cx);
        }
    }

    /// Shapes the masked or plain buffer, wrapping to `wrap_width` when set,
    /// with the paint-time text style.
    fn shape_current_text(&self, wrap_width: Option<Pixels>, window: &Window) -> ShapedText {
        let (text, mask_map) = if self.masked() {
            masked_display(&self.text)
        } else {
            (self.text.clone(), None)
        };
        let text_style = window.text_style();

        let font = Font {
            family: text_style.font_family.clone(),
            features: text_style.font_features.clone(),
            weight: text_style.font_weight,
            style: text_style.font_style,
            fallbacks: text_style.font_fallbacks.clone(),
        };

        let font_size = text_style.font_size.to_pixels(window.rem_size());
        let runs = [TextRun {
            len: text.len(),
            font,
            color: text_style.color,
            background_color: None,
            underline: None,
            strikethrough: None,
        }];

        let shaped =
            window
                .text_system()
                .shape_text(text.into(), font_size, &runs, wrap_width, None);

        let mut lines = Vec::new();
        match shaped {
            Ok(shaped) => {
                let mut start = 0;
                for line in shaped {
                    let end = start + line.len();
                    lines.push(ShapedLineEntry { start, line });
                    start = end + '\n'.len_utf8();
                }
            }
            Err(err) => log::warn!("shaping input text failed: {err}"),
        }

        ShapedText { lines, mask_map }
    }

    /// Stores the paint geometry for hit-testing and the IME conversions.
    fn store_layout(
        &mut self,
        text: ShapedText,
        bounds: Bounds<Pixels>,
        padding: FieldPadding,
        line_height: Pixels,
    ) -> gpui::Point<Pixels> {
        // The visual row table: one entry per wrapped row, top to bottom.
        let mut rows = Vec::new();
        let mut y = Pixels::ZERO;

        for (line_ix, entry) in text.lines.iter().enumerate() {
            let mut row_start = 0;
            let boundaries =
                entry.line.wrap_boundaries().iter().map(|boundary| {
                    entry.line.runs()[boundary.run_ix].glyphs[boundary.glyph_ix].index
                });
            for end in boundaries.chain([entry.line.len()]) {
                rows.push(VisualRow {
                    range: entry.start + row_start..entry.start + end,
                    y,
                    line_ix,
                });
                y += line_height;
                row_start = end;
            }
        }

        let content_height = y;

        let visible_height = if self.multiline {
            (bounds.size.height - padding.top * 2.0).max(Pixels::ZERO)
        } else {
            bounds.size.height
        };

        let caret = if rows.is_empty() {
            (Pixels::ZERO, Pixels::ZERO)
        } else {
            let display = display_index(&text.mask_map, self.selection().end);
            let row_ix = row_ix_for_display(&rows, display);
            let row = &rows[row_ix];
            let entry = &text.lines[row.line_ix];
            let shaped = &entry.line.unwrapped_layout;
            let row_start = row.range.start - entry.start;
            let x = shaped.x_for_index(display.min(row.range.end) - entry.start)
                - shaped.x_for_index(row_start);
            (row.y, x)
        };

        if self.multiline {
            let max_scroll = (content_height - visible_height).max(Pixels::ZERO);
            if self.scroll_to_caret_pending {
                self.scroll_to_caret_pending = false;
                let (caret_y, _) = caret;
                if caret_y + line_height > self.scroll_offset.y + visible_height {
                    self.scroll_offset.y = caret_y + line_height - visible_height;
                }
                if caret_y < self.scroll_offset.y {
                    self.scroll_offset.y = caret_y;
                }
            }
            self.scroll_offset.y = self.scroll_offset.y.clamp(Pixels::ZERO, max_scroll);
            self.scroll_offset.x = Pixels::ZERO;
        } else {
            let visible = (bounds.size.width - padding.left - padding.right).max(Pixels::ZERO);
            let line_width = text
                .lines
                .first()
                .map(|entry| entry.line.width())
                .unwrap_or(Pixels::ZERO);
            let max_scroll = (line_width - visible).max(Pixels::ZERO);
            if self.scroll_to_caret_pending {
                self.scroll_to_caret_pending = false;
                // Pull the scroll just far enough to reveal the caret on whichever edge it left.
                let (_, caret_x) = caret;
                if caret_x - self.scroll_offset.x > visible {
                    self.scroll_offset.x = caret_x - visible;
                }
                if caret_x < self.scroll_offset.x {
                    self.scroll_offset.x = caret_x;
                }
            }
            self.scroll_offset.x = self.scroll_offset.x.clamp(Pixels::ZERO, max_scroll);
            self.scroll_offset.y = Pixels::ZERO;
        }

        let origin = point(
            bounds.left() + padding.left - self.scroll_offset.x,
            if self.multiline {
                bounds.top() + padding.top - self.scroll_offset.y
            } else {
                bounds.top() + (bounds.size.height - line_height) / 2.0
            },
        );

        let text_width = (bounds.size.width - padding.left - padding.right).max(Pixels::ZERO);
        self.layout = Some(InputLayout {
            lines: text.lines,
            rows,
            bounds,
            origin,
            mask_map: text.mask_map,
            line_height,
            text_width,
            content_height,
            visible_height,
        });

        origin
    }

    /// Paints the selection highlight behind the text.
    fn paint_selection(&self, origin: gpui::Point<Pixels>, color: gpui::Hsla, window: &mut Window) {
        let selection = self.selection();
        if selection.is_empty() {
            return;
        }

        let Some(layout) = self.layout.as_ref() else {
            return;
        };

        let start = display_index(&layout.mask_map, selection.start);
        let end = display_index(&layout.mask_map, selection.end);

        for (row_ix, clipped, beyond) in layout.row_overlaps(start..end) {
            let top = origin.y + layout.rows[row_ix].y;
            let left = origin.x + layout.row_x(row_ix, clipped.start);
            let right = origin.x
                + if beyond {
                    layout.text_width
                } else {
                    layout.row_x(row_ix, clipped.end)
                };
            let rect =
                Bounds::from_corners(point(left, top), point(right, top + layout.line_height));
            window.paint_quad(fill(rect, color));
        }
    }

    /// Paints the text itself.
    fn paint_text(
        &self,
        origin: gpui::Point<Pixels>,
        right_padding: Pixels,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(layout) = self.layout.as_ref() else {
            return;
        };

        let mask = Bounds::from_corners(
            point(layout.bounds.left(), layout.bounds.top()),
            point(
                layout.bounds.right() - right_padding,
                layout.bounds.bottom(),
            ),
        );

        window.with_content_mask(Some(ContentMask { bounds: mask }), |window| {
            let mut line_top = Pixels::ZERO;
            for entry in &layout.lines {
                let line_origin = point(origin.x, origin.y + line_top);
                if let Err(err) = entry.line.paint(
                    line_origin,
                    layout.line_height,
                    TextAlign::Left,
                    None,
                    window,
                    cx,
                ) {
                    log::warn!("painting input text failed: {err}");
                }
                line_top += layout.line_height * (entry.line.wrap_boundaries().len() + 1) as f32;
            }
        });
    }

    /// Paints the caret at the cursor position.
    fn paint_caret(
        &self,
        origin: gpui::Point<Pixels>,
        font_size: Pixels,
        color: gpui::Hsla,
        window: &mut Window,
    ) {
        let Some(layout) = self.layout.as_ref() else {
            return;
        };

        let display = display_index(&layout.mask_map, self.cursor);
        let Some((row_ix, caret_x)) = layout.row_for_index(display) else {
            return;
        };

        let caret_top = origin.y + layout.rows[row_ix].y + (layout.line_height - font_size) / 2.0;
        let caret = Bounds::from_corners(
            point(origin.x + caret_x, caret_top),
            point(origin.x + caret_x + CARET_WIDTH, caret_top + font_size),
        );

        window.paint_quad(fill(caret, color));
    }

    /// Paints the field contents; returns whether a drag-select is in progress.
    fn paint(
        &mut self,
        text: ShapedText,
        bounds: Bounds<Pixels>,
        padding: FieldPadding,
        colors: ColorTokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let focused = self.focus_handle.is_focused(window);
        self.sync_focus(focused, cx);

        let text_style = window.text_style();
        let font_size = text_style.font_size.to_pixels(window.rem_size());
        let line_height = text_style
            .line_height
            .to_pixels(text_style.font_size, window.rem_size());

        let origin = self.store_layout(text, bounds, padding, line_height);

        if focused && !self.selection().is_empty() {
            self.paint_selection(origin, colors.selection, window);
        }

        self.paint_text(origin, padding.right, window, cx);

        // The composition underline shows the in-progress IME text.
        if let Some(range) = self.marked_range.clone()
            && !range.is_empty()
        {
            self.paint_marked_underline(origin, range, colors.foreground, window);
        }

        // A caret promises that typing lands there; in read-only it does not.
        if focused && self.caret_visible(focused) && self.selection().is_empty() {
            self.paint_caret(origin, font_size, colors.foreground, window);
        }

        self.dragging
    }

    /// The text index at a window point, using the last painted layout.
    fn index_at(&self, position: gpui::Point<Pixels>) -> usize {
        let Some(layout) = self.layout.as_ref() else {
            return 0;
        };
        real_index(&layout.mask_map, layout.index_at(position))
    }

    /// Extends the selection to `position` while drag-selecting.
    fn drag_select_to(
        &mut self,
        position: gpui::Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let cursor = self.index_at(position);
        if cursor == self.cursor {
            return;
        }
        self.cursor = cursor;
        self.notify(window, cx);
    }

    /// Paints the composition underline under the marked range.
    fn paint_marked_underline(
        &self,
        origin: gpui::Point<Pixels>,
        range: Range<usize>,
        color: gpui::Hsla,
        window: &mut Window,
    ) {
        let Some(layout) = self.layout.as_ref() else {
            return;
        };

        let start = display_index(&layout.mask_map, range.start);
        let end = display_index(&layout.mask_map, range.end);
        let thickness = UNDERLINE_WIDTH;

        for (row_ix, clipped, beyond) in layout.row_overlaps(start..end) {
            let top = origin.y + layout.rows[row_ix].y + layout.line_height - thickness;
            let left = origin.x + layout.row_x(row_ix, clipped.start);
            let right = origin.x
                + if beyond {
                    layout.text_width
                } else {
                    layout.row_x(row_ix, clipped.end)
                };
            let rect = Bounds::from_corners(point(left, top), point(right, top + thickness));
            window.paint_quad(fill(rect, color));
        }
    }

    /// Register drag listeners.
    fn register_drag_listeners(entity: &Entity<Self>, window: &mut Window) {
        window.on_mouse_event({
            let state = entity.clone();
            move |event: &MouseMoveEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble || event.pressed_button != Some(MouseButton::Left)
                {
                    return;
                }
                state.update(cx, |state, cx| {
                    state.drag_select_to(event.position, window, cx)
                });
            }
        });
        window.on_mouse_event({
            let state = entity.clone();
            move |event: &MouseUpEvent, phase, _window, cx| {
                if phase != DispatchPhase::Bubble || event.button != MouseButton::Left {
                    return;
                }
                state.update(cx, |state, _| state.dragging = false);
            }
        });
    }

    /// The mouse-down handler: starts a drag and places or extends the selection.
    fn handle_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dragging = true;

        let index = self.index_at(event.position);

        if event.click_count >= 3 {
            // Triple-click selects the clicked row, which is all of it when single-line.
            let row_range = self.layout.as_ref().and_then(|layout| {
                let display = display_index(&layout.mask_map, index);
                layout.row_range_at(display).map(|range| {
                    real_index(&layout.mask_map, range.start)
                        ..real_index(&layout.mask_map, range.end)
                })
            });

            match row_range {
                Some(range) => self.select_range(range, window, cx),
                None => {
                    let len = self.text.len();
                    self.select_range(0..len, window, cx);
                }
            }
        } else if event.click_count == 2 {
            let start = previous_word_start(&self.text, index);
            let end = next_word_end(&self.text, index);
            self.select_range(start..end, window, cx);
        } else if event.modifiers.shift {
            self.cursor = index;
            self.notify(window, cx);
        } else {
            self.cursor = index;
            self.anchor = index;
            self.notify(window, cx);
        }
    }

    /// The key-down handler: inserts typed characters.
    fn handle_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.focus_handle.contains_focused(window, cx) {
            return;
        }

        let keystroke = &event.keystroke;
        // Chars typed with a menu modifier are shortcuts, not text,
        // and control characters (enter, tab) have their own semantics.
        if let Some(text) = &keystroke.key_char
            && !keystroke.modifiers.platform
            && !keystroke.modifiers.control
            && text.chars().all(|ch| !ch.is_control())
        {
            let range = self.selection();
            self.replace_range(range, text, window, cx);
            cx.stop_propagation();
        }
    }

    fn left(&mut self, _: &Left, window: &mut Window, cx: &mut Context<Self>) {
        let to = previous_grapheme_start(&self.text, self.cursor);
        self.move_cursor(to, false, window, cx);
    }

    fn select_left(&mut self, _: &SelectLeft, window: &mut Window, cx: &mut Context<Self>) {
        let to = previous_grapheme_start(&self.text, self.cursor);
        self.move_cursor(to, true, window, cx);
    }

    fn right(&mut self, _: &Right, window: &mut Window, cx: &mut Context<Self>) {
        let to = next_grapheme_start(&self.text, self.cursor);
        self.move_cursor(to, false, window, cx);
    }

    fn select_right(&mut self, _: &SelectRight, window: &mut Window, cx: &mut Context<Self>) {
        let to = next_grapheme_start(&self.text, self.cursor);
        self.move_cursor(to, true, window, cx);
    }

    fn word_left(&mut self, _: &WordLeft, window: &mut Window, cx: &mut Context<Self>) {
        let to = previous_word_start(&self.text, self.cursor);
        self.move_cursor(to, false, window, cx);
    }

    fn select_word_left(
        &mut self,
        _: &SelectWordLeft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let to = previous_word_start(&self.text, self.cursor);
        self.move_cursor(to, true, window, cx);
    }

    fn word_right(&mut self, _: &WordRight, window: &mut Window, cx: &mut Context<Self>) {
        let to = next_word_end(&self.text, self.cursor);
        self.move_cursor(to, false, window, cx);
    }

    fn select_word_right(
        &mut self,
        _: &SelectWordRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let to = next_word_end(&self.text, self.cursor);
        self.move_cursor(to, true, window, cx);
    }

    fn up(&mut self, _: &Up, window: &mut Window, cx: &mut Context<Self>) {
        self.move_vertical(-1, false, window, cx);
    }

    fn down(&mut self, _: &Down, window: &mut Window, cx: &mut Context<Self>) {
        self.move_vertical(1, false, window, cx);
    }

    fn select_up(&mut self, _: &SelectUp, window: &mut Window, cx: &mut Context<Self>) {
        self.move_vertical(-1, true, window, cx);
    }

    fn select_down(&mut self, _: &SelectDown, window: &mut Window, cx: &mut Context<Self>) {
        self.move_vertical(1, true, window, cx);
    }

    /// Moves the caret one visual row up or down, keeping the goal column.
    fn move_vertical(
        &mut self,
        delta: isize,
        extend: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.multiline {
            return;
        }

        let target = {
            let Some(layout) = self.layout.as_ref() else {
                return;
            };
            let display = display_index(&layout.mask_map, self.cursor);

            let Some((row_ix, caret_x)) = layout.row_for_index(display) else {
                return;
            };
            let target = row_ix as isize + delta;

            let (display, goal_x) = if target < 0 {
                (0, None)
            } else if target as usize >= layout.rows.len() {
                (display_index(&layout.mask_map, self.text.len()), None)
            } else {
                let goal_x = self.goal_x.unwrap_or(caret_x);
                (layout.index_at_row_x(target as usize, goal_x), Some(goal_x))
            };

            Some((real_index(&layout.mask_map, display), goal_x))
        };

        let Some((to, goal_x)) = target else {
            return;
        };

        self.move_cursor(to, extend, window, cx);
        // Set after `move_cursor`, which resets the goal on every move.
        self.goal_x = goal_x;
    }

    fn home(&mut self, _: &Home, window: &mut Window, cx: &mut Context<Self>) {
        self.move_cursor(0, false, window, cx);
    }

    fn end(&mut self, _: &End, window: &mut Window, cx: &mut Context<Self>) {
        self.move_cursor(self.text.len(), false, window, cx);
    }

    fn select_home(&mut self, _: &SelectHome, window: &mut Window, cx: &mut Context<Self>) {
        self.move_cursor(0, true, window, cx);
    }

    fn select_end(&mut self, _: &SelectEnd, window: &mut Window, cx: &mut Context<Self>) {
        self.move_cursor(self.text.len(), true, window, cx);
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        self.delete_backward(false, window, cx);
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        self.delete_forward(false, window, cx);
    }

    fn delete_word_left(
        &mut self,
        _: &DeleteWordLeft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.delete_backward(true, window, cx);
    }

    fn delete_word_right(
        &mut self,
        _: &DeleteWordRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.delete_forward(true, window, cx);
    }

    fn delete_to_beginning_of_line(
        &mut self,
        _: &DeleteToBeginningOfLine,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = 0..self.cursor;
        self.replace_range(range, "", window, cx);
    }

    fn select_all(&mut self, _: &SelectAll, window: &mut Window, cx: &mut Context<Self>) {
        let len = self.text.len();
        self.select_range(0..len, window, cx);
    }

    fn copy(&mut self, _: &Copy, window: &mut Window, cx: &mut Context<Self>) {
        if self.selection().is_empty() {
            // Let an outer handler copy its own selection instead.
            cx.propagate();
            return;
        }
        self.copy_selection(window, cx);
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only || self.password {
            return;
        }

        if self.copy_selection(window, cx) {
            self.delete_selection(window, cx);
        }
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(item) = cx.read_from_clipboard()
            && let Some(text) = item.text()
        {
            let range = self.selection();
            self.replace_range(range, &text, window, cx);
        }
    }

    fn undo(&mut self, _: &Undo, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }

        let Some(entry) = self.undo_stack.pop() else {
            return;
        };

        let inverse = entry.reverse(&mut self.text);
        self.redo_stack.push(inverse);
        self.cursor = entry.cursor.min(self.text.len());
        self.anchor = entry.anchor.min(self.text.len());

        cx.emit(InputStateEvent::TextChanged);
        self.notify(window, cx);
    }

    fn redo(&mut self, _: &Redo, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }

        let Some(entry) = self.redo_stack.pop() else {
            return;
        };

        let inverse = entry.reverse(&mut self.text);
        self.undo_stack.push(inverse);

        // The caret lands at the end of the restored text,
        // not where it was before the original edit.
        let caret = entry.range.start + entry.old_text.len();
        self.cursor = caret.min(self.text.len());
        self.anchor = self.cursor;

        cx.emit(InputStateEvent::TextChanged);
        self.notify(window, cx);
    }

    fn submit(&mut self, _: &Submit, _window: &mut Window, cx: &mut Context<Self>) {
        // A multiline field in the default mode turns Enter into a newline
        if self.multiline && self.submit_on == SubmitOn::ModifiedEnter {
            let range = self.selection();
            self.replace_range(range, "\n", _window, cx);
        } else {
            cx.emit(InputStateEvent::Submitted);
        }
    }

    fn secondary_submit(
        &mut self,
        _: &SecondarySubmit,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.emit(InputStateEvent::Submitted);
    }

    fn insert_newline(&mut self, _: &InsertNewline, window: &mut Window, cx: &mut Context<Self>) {
        if self.multiline {
            let range = self.selection();
            self.replace_range(range, "\n", window, cx);
        }
    }

    fn escape(&mut self, _: &Escape, window: &mut Window, cx: &mut Context<Self>) {
        window.blur(cx);
    }
}

impl EntityInputHandler for InputState {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = utf16_range_to_utf8(&self.text, range_utf16);
        adjusted_range.replace(utf8_range_to_utf16(&self.text, range.clone()));
        Some(self.text[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: utf8_range_to_utf16(&self.text, self.selection()),
            reversed: self.cursor < self.anchor,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked_range
            .clone()
            .map(|range| utf8_range_to_utf16(&self.text, range))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Simulated typing delivers named keys' control characters here
        if text.chars().all(|ch| ch.is_control()) {
            return;
        }
        let range = range_utf16.map_or_else(
            || self.selection(),
            |range| utf16_range_to_utf8(&self.text, range),
        );
        self.replace_range(range, text, window, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.read_only {
            return;
        }

        let range = range_utf16.map_or_else(
            || self.selection(),
            |range| utf16_range_to_utf8(&self.text, range),
        );

        let new_text = sanitize(new_text, self.multiline);
        let old_text = self.text[range.clone()].to_string();
        self.push_history(range.clone(), old_text, new_text.len());
        self.text.replace_range(range.clone(), &new_text);

        let start = range.start;
        let end = start + new_text.len();
        self.marked_range = Some(start..end);

        let selected_end = new_selected_range_utf16
            .map(|selected| utf16_range_to_utf8(&self.text, selected).end)
            .unwrap_or(end);

        self.cursor = selected_end;
        self.anchor = selected_end;

        cx.emit(InputStateEvent::TextChanged);
        self.notify(window, cx);
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        _element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let layout = self.layout.as_ref()?;

        let start = display_index(
            &layout.mask_map,
            utf16_to_utf8_offset(&self.text, range_utf16.start),
        );
        let end = display_index(
            &layout.mask_map,
            utf16_to_utf8_offset(&self.text, range_utf16.end),
        );

        // `origin` is already an absolute window point.
        if start == end {
            let (row_ix, x) = layout.row_for_index(start)?;
            let top = layout.origin.y + layout.rows[row_ix].y;
            return Some(Bounds::from_corners(
                point(layout.origin.x + x, top),
                point(layout.origin.x + x, top + layout.line_height),
            ));
        }

        let overlaps = layout.row_overlaps(start..end);
        let (first_ix, first_range, _) = overlaps.first().cloned()?;
        let (last_ix, last_range, last_beyond) = overlaps.last().cloned()?;

        let left = layout.origin.x + layout.row_x(first_ix, first_range.start);
        let right = layout.origin.x
            + if last_beyond {
                layout.text_width
            } else {
                layout.row_x(last_ix, last_range.end)
            };

        Some(Bounds::from_corners(
            point(left, layout.origin.y + layout.rows[first_ix].y),
            point(
                right,
                layout.origin.y + layout.rows[last_ix].y + layout.line_height,
            ),
        ))
    }

    fn character_index_for_point(
        &mut self,
        window_point: gpui::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let layout = self.layout.as_ref()?;
        let index =
            real_index(&layout.mask_map, layout.index_at(window_point)).min(self.text.len());
        Some(utf8_range_to_utf16(&self.text, index..index).start)
    }

    fn text_length_utf16(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.text.encode_utf16().count())
    }
}

/// A text field bound to an application-owned [`InputState`].
#[derive(IntoElement)]
#[must_use]
pub struct Input {
    id: ElementId,
    state: Entity<InputState>,
    placeholder: SharedString,
    size: Size,
    disabled: bool,
    /// The placeholder stays visible while focused.
    placeholder_when_focused: bool,
    fully_rounded: bool,
    a11y_label: Option<SharedString>,
    /// Multiline field height in row, ignored when the state is single-line.
    rows: usize,
    prefix: Option<AnyElement>,
    suffix: Option<AnyElement>,
    style: StyleRefinement,
}

impl Input {
    /// Creates a text field bound to `state`.
    pub fn new<I>(id: I, state: Entity<InputState>) -> Self
    where
        I: Into<ElementId>,
    {
        Self {
            id: id.into(),
            state,
            placeholder: SharedString::default(),
            size: Size::default(),
            disabled: false,
            placeholder_when_focused: false,
            fully_rounded: false,
            a11y_label: None,
            rows: DEFAULT_ROWS,
            prefix: None,
            suffix: None,
            style: StyleRefinement::default(),
        }
    }

    /// Sets the placeholder shown when the field is empty and unfocused.
    pub fn placeholder<P>(mut self, placeholder: P) -> Self
    where
        P: Into<SharedString>,
    {
        self.placeholder = placeholder.into();
        self
    }

    /// Sets the height of a multiline field, in rows; ignored for single-line state.
    pub fn rows(mut self, rows: usize) -> Self {
        self.rows = rows.max(1);
        self
    }

    /// Shows the placeholder while the field is focused.
    pub fn placeholder_when_focused(mut self, when_focused: bool) -> Self {
        self.placeholder_when_focused = when_focused;
        self
    }

    /// Renders the field as a pill instead of the control radius.
    pub fn fully_rounded(mut self, fully_rounded: bool) -> Self {
        self.fully_rounded = fully_rounded;
        self
    }

    /// Renders the field inert, unfocusable, and dimmed.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Overrides the accessibility label, which defaults to the placeholder,
    /// then the element ID.
    pub fn accessibility_label<L>(mut self, label: L) -> Self
    where
        L: Into<SharedString>,
    {
        self.a11y_label = Some(label.into());
        self
    }

    /// Docks an element inside the field's leading edge.
    pub fn prefix<E>(mut self, prefix: E) -> Self
    where
        E: IntoElement,
    {
        self.prefix = Some(prefix.into_any_element());
        self
    }

    /// Docks an element inside the field's trailing edge.
    pub fn suffix<E>(mut self, suffix: E) -> Self
    where
        E: IntoElement,
    {
        self.suffix = Some(suffix.into_any_element());
        self
    }
}

impl gpui::Styled for Input {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl Sizable for Input {
    fn apply_size(&mut self, size: Size) {
        self.size = size;
    }
}

impl RenderOnce for Input {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let colors = theme.colors;
        let radius = theme.radius;
        let rem = window.rem_size();
        let input_height = self.size.control_height_rems().to_pixels(rem);
        let text_size = self.size.text_rems().to_pixels(rem);
        let padding_x = self.size.padding_x_rems().to_pixels(rem);
        let dense_padding_x = self.size.dense_padding_x_rems().to_pixels(rem);
        let focus_handle = self.state.read(cx).focus_handle.clone();
        let focused = focus_handle.is_focused(window);

        let is_empty = self.state.read(cx).text.is_empty();
        let is_password = self.state.read(cx).is_password();
        let revealed = self.state.read(cx).is_revealed();
        let multiline = self.state.read(cx).is_multiline();
        let recording = focused && !self.disabled && !self.state.read(cx).is_read_only();

        let docked_height = Size::Small.control_height_rems().to_pixels(rem);
        let edge_inset = ((input_height - docked_height) / 2.0).max(Pixels::ZERO);

        let vertical_inset = dense_padding_x * 0.5;
        let reveal_width = Size::Small.dense_padding_x_rems().to_pixels(rem) * 2.0
            + Size::Small.icon_rems().to_pixels(rem);

        let field_padding = FieldPadding {
            left: Pixels::ZERO,
            right: if is_password {
                (edge_inset + reveal_width - dense_padding_x).max(Pixels::ZERO)
            } else {
                Pixels::ZERO
            },
            top: vertical_inset,
        };

        let line_height = multiline.then(|| match window.text_style().line_height {
            DefiniteLength::Fraction(ratio) => text_size * ratio,
            DefiniteLength::Absolute(length) => length.to_pixels(window.rem_size()),
        });

        let field_height = line_height.map_or(input_height, |line_height| {
            line_height * self.rows as f32 + vertical_inset * 2.0
        });

        let fallback_label = self
            .a11y_label
            .clone()
            .or_else(|| (!self.placeholder.is_empty()).then_some(self.placeholder.clone()))
            .unwrap_or_else(|| SharedString::from(self.id.to_string()));

        let edge_zone = |name: &'static str, element: AnyElement| {
            div()
                .debug_selector(|| name.to_string())
                .flex_none()
                .h_full()
                .flex()
                .items_center()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(element)
        };

        let canvas_element = {
            let canvas = canvas(
                {
                    let state = self.state.clone();
                    move |bounds, window, cx| {
                        // The hitbox backs the I-beam hover cursor.
                        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
                        let wrap_width = multiline.then(|| {
                            (bounds.size.width - field_padding.left - field_padding.right)
                                .max(Pixels::ZERO)
                        });
                        let text = state.read(cx).shape_current_text(wrap_width, window);
                        (text, hitbox)
                    }
                },
                {
                    let state = self.state.clone();
                    let focus = focus_handle.clone();
                    let disabled = self.disabled;

                    move |bounds, (text, hitbox), window, cx| {
                        if !disabled {
                            window.set_cursor_style(CursorStyle::IBeam, &hitbox);
                        }

                        let dragging = state.update(cx, |state, cx| {
                            state.paint(text, bounds, field_padding, colors, window, cx)
                        });

                        if disabled {
                            return;
                        }

                        window.handle_input(
                            &focus,
                            gpui::ElementInputHandler::new(bounds, state.clone()),
                            cx,
                        );

                        window.on_mouse_event({
                            let state = state.clone();
                            move |event: &ScrollEvent, phase, window, cx| {
                                if phase != DispatchPhase::Bubble
                                    || !bounds.contains(&event.position)
                                {
                                    return;
                                }

                                let delta = match event.delta {
                                    ScrollDelta::Pixels(delta) => delta,
                                    ScrollDelta::Lines(lines) => point(
                                        px(lines.x * SCROLL_LINES_HEIGHT),
                                        px(lines.y * SCROLL_LINES_HEIGHT),
                                    ),
                                };

                                let consumed = state
                                    .update(cx, |state, cx| state.scroll_by(delta, window, cx));

                                if consumed {
                                    cx.stop_propagation();
                                }
                            }
                        });

                        if dragging {
                            InputState::register_drag_listeners(&state, window);
                        }
                    }
                },
            )
            .absolute()
            .inset_0();
            if recording {
                let epoch = self.state.read(cx).caret_epoch;
                let animation_id = ElementId::from(SharedString::from(format!(
                    "{self_id}-caret-blink-{epoch}",
                    self_id = self.id,
                )));
                canvas
                    .with_animation(
                        animation_id,
                        // The blink needs four repaints per period,
                        // not the display's refresh rate.
                        Animation::new(CARET_BLINK_INTERVAL * 2)
                            .repeat()
                            .with_max_fps(4.0),
                        |canvas, _| canvas,
                    )
                    .into_any_element()
            } else {
                canvas.into_any_element()
            }
        };

        h_flex()
            .id(self.id.clone())
            .debug_selector(|| "input-field".to_string())
            .key_context(INPUT_CONTEXT)
            .role(Role::TextInput)
            .aria_label(fallback_label)
            .relative()
            .overflow_hidden()
            .w_full()
            .h(field_height)
            .pl(if self.prefix.is_some() {
                padding_x
            } else {
                dense_padding_x
            })
            .pr(if self.suffix.is_some() {
                padding_x
            } else {
                dense_padding_x
            })
            .gap_2()
            .rounded(if self.fully_rounded {
                radius.full
            } else {
                radius.control
            })
            .bg(colors.input)
            .control_text(self.size)
            .text_color(colors.foreground)
            .border_1()
            .border_color(colors.border)
            .refine_style(&self.style)
            .when(self.disabled, |input| input.opacity(DISABLED_OPACITY))
            .when(!self.disabled, |input| {
                input.track_focus(&focus_handle).tab_stop(true)
            })
            .when(focused, |input| input.border_color(colors.ring))
            .when_some(self.prefix, |input, prefix| {
                input.child(edge_zone("input-prefix", prefix))
            })
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_hidden()
                    .flex()
                    .flex_row()
                    .items_center()
                    .when_some(line_height, |region, line_height| {
                        region.items_start().line_height(line_height)
                    })
                    .when(
                        is_empty && (!focused || self.placeholder_when_focused),
                        |region| {
                            region.child(
                                div()
                                    .text_color(colors.muted_foreground)
                                    .when(multiline, |placeholder| placeholder.pt(vertical_inset))
                                    .child(self.placeholder.clone()),
                            )
                        },
                    )
                    .child(canvas_element),
            )
            .when_some(self.suffix, |input, suffix| {
                input.child(edge_zone("input-suffix", suffix))
            })
            .when(is_password && !self.disabled, |input| {
                input.child(
                    div()
                        .id((self.id.clone(), "input-reveal"))
                        .debug_selector(|| "input-reveal".to_string())
                        .absolute()
                        .right(edge_inset)
                        .top_0()
                        .bottom_0()
                        .flex()
                        .items_center()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            Button::new((self.id.clone(), "input-reveal-button"))
                                .debug_selector(|| "input-reveal-button".to_string())
                                .icon(if revealed {
                                    IconRole::EyeOff
                                } else {
                                    IconRole::Eye
                                })
                                .variant(ButtonVariant::Ghost)
                                .small()
                                .accessibility_label(if revealed {
                                    "Hide password"
                                } else {
                                    "Show password"
                                })
                                .on_click({
                                    let state = self.state.clone();
                                    move |_, _, cx| {
                                        state.update(cx, |state, cx| {
                                            state.set_revealed(!state.is_revealed(), cx)
                                        });
                                    }
                                }),
                        ),
                )
            })
            .when(!self.disabled, |input| {
                input
                    .on_mouse_down(MouseButton::Left, {
                        let mouse_down_handler = self.state.read(cx).mouse_down_handler.clone();
                        move |event: &MouseDownEvent, window, cx| {
                            mouse_down_handler(event, window, cx);
                        }
                    })
                    .on_key_down({
                        let key_down_handler = self.state.read(cx).key_down_handler.clone();
                        move |event: &KeyDownEvent, window, cx| {
                            key_down_handler(event, window, cx);
                        }
                    })
            })
            .on_action({
                let state = self.state.clone();
                move |action: &Escape, window, cx| {
                    state.update(cx, |state, cx| state.escape(action, window, cx))
                }
            })
            .on_action({
                let state = self.state.clone();
                move |action: &Submit, window, cx| {
                    state.update(cx, |state, cx| state.submit(action, window, cx))
                }
            })
            .on_action({
                let state = self.state.clone();
                move |action: &SecondarySubmit, window, cx| {
                    state.update(cx, |state, cx| state.secondary_submit(action, window, cx))
                }
            })
            .on_action({
                let state = self.state.clone();
                move |action: &InsertNewline, window, cx| {
                    state.update(cx, |state, cx| state.insert_newline(action, window, cx))
                }
            })
            .on_action({
                let state = self.state.clone();
                move |action: &Backspace, window, cx| {
                    state.update(cx, |state, cx| state.backspace(action, window, cx))
                }
            })
            .on_action({
                let state = self.state.clone();
                move |action: &Delete, window, cx| {
                    state.update(cx, |state, cx| state.delete(action, window, cx))
                }
            })
            .on_action({
                let state = self.state.clone();
                move |action: &DeleteWordLeft, window, cx| {
                    state.update(cx, |state, cx| state.delete_word_left(action, window, cx))
                }
            })
            .on_action({
                let state = self.state.clone();
                move |action: &DeleteWordRight, window, cx| {
                    state.update(cx, |state, cx| state.delete_word_right(action, window, cx))
                }
            })
            .on_action({
                let state = self.state.clone();
                move |action: &DeleteToBeginningOfLine, window, cx| {
                    state.update(cx, |state, cx| {
                        state.delete_to_beginning_of_line(action, window, cx)
                    })
                }
            })
            .on_action({
                let state = self.state.clone();
                move |action: &Left, window, cx| {
                    state.update(cx, |state, cx| state.left(action, window, cx))
                }
            })
            .on_action({
                let state = self.state.clone();
                move |action: &Right, window, cx| {
                    state.update(cx, |state, cx| state.right(action, window, cx))
                }
            })
            .on_action({
                let state = self.state.clone();
                move |action: &SelectLeft, window, cx| {
                    state.update(cx, |state, cx| state.select_left(action, window, cx))
                }
            })
            .on_action({
                let state = self.state.clone();
                move |action: &SelectRight, window, cx| {
                    state.update(cx, |state, cx| state.select_right(action, window, cx))
                }
            })
            .on_action({
                let state = self.state.clone();
                move |action: &Up, window, cx| {
                    state.update(cx, |state, cx| state.up(action, window, cx))
                }
            })
            .on_action({
                let state = self.state.clone();
                move |action: &Down, window, cx| {
                    state.update(cx, |state, cx| state.down(action, window, cx))
                }
            })
            .on_action({
                let state = self.state.clone();
                move |action: &SelectUp, window, cx| {
                    state.update(cx, |state, cx| state.select_up(action, window, cx))
                }
            })
            .on_action({
                let state = self.state.clone();
                move |action: &SelectDown, window, cx| {
                    state.update(cx, |state, cx| state.select_down(action, window, cx))
                }
            })
            .on_action({
                let state = self.state.clone();
                move |action: &WordLeft, window, cx| {
                    state.update(cx, |state, cx| state.word_left(action, window, cx))
                }
            })
            .on_action({
                let state = self.state.clone();
                move |action: &WordRight, window, cx| {
                    state.update(cx, |state, cx| state.word_right(action, window, cx))
                }
            })
            .on_action({
                let state = self.state.clone();
                move |action: &SelectWordLeft, window, cx| {
                    state.update(cx, |state, cx| state.select_word_left(action, window, cx))
                }
            })
            .on_action({
                let state = self.state.clone();
                move |action: &SelectWordRight, window, cx| {
                    state.update(cx, |state, cx| state.select_word_right(action, window, cx))
                }
            })
            .on_action({
                let state = self.state.clone();
                move |action: &Home, window, cx| {
                    state.update(cx, |state, cx| state.home(action, window, cx))
                }
            })
            .on_action({
                let state = self.state.clone();
                move |action: &End, window, cx| {
                    state.update(cx, |state, cx| state.end(action, window, cx))
                }
            })
            .on_action({
                let state = self.state.clone();
                move |action: &SelectHome, window, cx| {
                    state.update(cx, |state, cx| state.select_home(action, window, cx))
                }
            })
            .on_action({
                let state = self.state.clone();
                move |action: &SelectEnd, window, cx| {
                    state.update(cx, |state, cx| state.select_end(action, window, cx))
                }
            })
            .on_action({
                let state = self.state.clone();
                move |action: &SelectAll, window, cx| {
                    state.update(cx, |state, cx| state.select_all(action, window, cx))
                }
            })
            .on_action({
                let state = self.state.clone();
                move |action: &Copy, window, cx| {
                    state.update(cx, |state, cx| state.copy(action, window, cx))
                }
            })
            .on_action({
                let state = self.state.clone();
                move |action: &Cut, window, cx| {
                    state.update(cx, |state, cx| state.cut(action, window, cx))
                }
            })
            .on_action({
                let state = self.state.clone();
                move |action: &Paste, window, cx| {
                    state.update(cx, |state, cx| state.paste(action, window, cx))
                }
            })
            .on_action({
                let state = self.state.clone();
                move |action: &Undo, window, cx| {
                    state.update(cx, |state, cx| state.undo(action, window, cx))
                }
            })
            .on_action({
                let state = self.state.clone();
                move |action: &Redo, window, cx| {
                    state.update(cx, |state, cx| state.redo(action, window, cx))
                }
            })
    }
}

/// Registers the input keybindings on the application, once at startup.
///
/// Pass `None` for the platform defaults or an [`InputBindings`] table for
/// custom ones.
pub fn bind_input_keys<B>(cx: &mut App, bindings: B)
where
    B: Into<Option<InputBindings>>,
{
    cx.bind_keys(bindings.into().unwrap_or_default().into_bindings());
}

/// The row holding `display`.
fn row_ix_for_display(rows: &[VisualRow], display: usize) -> usize {
    rows.iter()
        .position(|row| display <= row.range.end)
        .unwrap_or(rows.len() - 1)
}

/// Builds the masked display: one bullet per grapheme, newlines kept as line breaks.
fn masked_display(text: &str) -> (String, Option<Vec<(usize, usize)>>) {
    let mut display = String::new();
    let mut map = Vec::with_capacity(text.len() + 1);
    for (offset, grapheme) in text.grapheme_indices(true) {
        if grapheme == "\n" {
            display.push('\n');
            map.push((offset, 1));
        } else {
            display.push_str(MASK);
            map.push((offset, MASK_LEN));
        }
    }
    // The end sentinel maps a click past the last glyph to the buffer end.
    map.push((text.len(), 0));
    (display, Some(map))
}

/// Maps a real buffer offset onto the masked display; identity when unmasked.
fn display_index(mask_map: &Option<Vec<(usize, usize)>>, real: usize) -> usize {
    match mask_map {
        None => real,
        Some(map) => map
            .iter()
            .take_while(|&&(offset, _)| offset < real)
            .map(|&(_, len)| len)
            .sum(),
    }
}

/// Maps a display offset from a click back onto the real buffer.
fn real_index(mask_map: &Option<Vec<(usize, usize)>>, display: usize) -> usize {
    match mask_map {
        None => display,
        Some(map) => {
            let mut consumed = 0;
            for &(offset, len) in map {
                if consumed >= display {
                    return offset;
                }
                consumed += len;
            }
            map.last().map(|&(offset, _)| offset).unwrap_or(0)
        }
    }
}

/// Newlines have no meaning in a single-line field.
fn sanitize<T>(text: T, multiline: bool) -> String
where
    T: Into<String>,
{
    let text = text.into();
    if multiline {
        text.replace('\r', "")
    } else {
        text.replace('\n', " ").replace('\r', "")
    }
}

/// Floors `index` to the nearest grapheme-cluster boundary at or below it.
fn previous_grapheme_start(text: &str, index: usize) -> usize {
    text.grapheme_indices(true)
        .rev()
        .find(|(offset, _)| *offset < index)
        .map(|(offset, _)| offset)
        .unwrap_or(0)
}

/// Advances `index` to the next grapheme-cluster boundary.
fn next_grapheme_start(text: &str, index: usize) -> usize {
    text.grapheme_indices(true)
        .find(|(offset, _)| *offset > index)
        .map(|(offset, _)| offset)
        .unwrap_or(text.len())
}

/// The start of the word before `from`.
fn previous_word_start(text: &str, from: usize) -> usize {
    let mut best = 0;
    for (offset, word) in text.split_word_bound_indices() {
        if offset >= from {
            break;
        }
        if !word.starts_with(char::is_whitespace) {
            best = offset;
        }
    }
    best
}

/// The end of the word after `from`.
fn next_word_end(text: &str, from: usize) -> usize {
    for (offset, word) in text.split_word_bound_indices() {
        let end = offset + word.len();
        if end > from && !word.starts_with(char::is_whitespace) {
            return end;
        }
    }
    text.len()
}

/// Converts a UTF-16 offset into a UTF-8 byte offset.
fn utf16_to_utf8_offset(text: &str, utf16_offset: usize) -> usize {
    let mut utf16_count = 0;
    for (byte_offset, char) in text.char_indices() {
        if utf16_count >= utf16_offset {
            return byte_offset;
        }
        utf16_count += char.len_utf16();
    }
    text.len()
}

/// Converts a UTF-8 byte range to a UTF-16 range.
fn utf8_range_to_utf16(text: &str, range: Range<usize>) -> Range<usize> {
    let start = text[..range.start].encode_utf16().count();
    let end = start + text[range.start..range.end].encode_utf16().count();
    start..end
}

/// Converts a UTF-16 range to a UTF-8 byte range.
fn utf16_range_to_utf8(text: &str, range: Range<usize>) -> Range<usize> {
    utf16_to_utf8_offset(text, range.start)..utf16_to_utf8_offset(text, range.end)
}

//! Renders a markdown document as formatted, selectable text. Parsing runs
//! once per source change and produces [`MarkdownBlock`]s; rendering
//! virtualizes the visible blocks and resolves styles from the theme at
//! draw time. Text, code, and tables drag-select and copy; task checkboxes
//! report toggles and code blocks offer a copy action. Images render inline
//! or as standalone blocks. Footnotes, math, and HTML render nothing; long
//! code lines clip rather than scroll.
//!
//! The view is the state: build a [`MarkdownViewState`] in a
//! [`gpui::Entity`] and embed it with [`TextView::new`], or let
//! [`TextView::markdown`] own its state under an element id.
//!
//! # Examples
//!
//! ```no_run
//! use gpui::{App, AppContext as _};
//! use ui::markdown::{MarkdownViewState, TextView};
//!
//! fn notes(cx: &mut App) -> TextView {
//!     let state = cx.new(|_| MarkdownViewState::new("notes", "# Notes\n- [x] shipped"));
//!     TextView::new(&state)
//!         .on_link_click(|destination, _window, _cx| println!("{destination}"))
//!         .on_task_toggle(|index, checked, _window, _cx| println!("{index}: {checked}"))
//! }
//! ```

use std::borrow::Borrow as _;
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, Bounds, ClipboardItem, Context, Div, Element, ElementId, Entity, FontStyle,
    GlobalElementId, HighlightStyle, InspectorElementId, InteractiveElement as _, InteractiveText,
    IntoElement, KeyBinding, KeyBindingContextPredicate, LayoutId, ListAlignment, ListOffset,
    ListState, MouseButton, MouseDownEvent, MouseMoveEvent, ObjectFit, ParentElement as _, Pixels,
    Refineable as _, Render, SharedString, StatefulInteractiveElement as _, StrikethroughStyle,
    StyleRefinement, Styled, StyledImage as _, StyledText, TextLayout, UnderlineStyle, Window,
    actions, div, img, list, px, relative,
};
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use crate::components::button::{Button, ButtonVariant};
use crate::components::checkbox::Checkbox;
use crate::components::common::Sizable as _;
use crate::components::highlight::{HighlightedCode, Language, highlight, plain};
use crate::components::scrollbar::{Scrollbar, ScrollbarVisibility};
use crate::components::utils::code_line;
use crate::theme::{
    ActiveTheme as _, BORDER_WIDTH, CODE_LINE_HEIGHT, CODE_OVERSCROLL_MAX, Size, TextRole,
    Typeset as _,
};

/// The bottom padding between sibling blocks, in root font sizes.
const BLOCK_GAP: f32 = 0.75;
/// The tighter bottom padding between list items, in root font sizes.
const ITEM_GAP: f32 = 0.2;
/// The room a list marker column takes, in root font sizes.
const MARKER_WIDTH: f32 = 1.75;
/// The text inset behind a quote bar, in root font sizes.
const QUOTE_INSET: f32 = 1.0;
/// The indent each list level adds, in root font sizes.
const INDENT_STEP: f32 = 1.5;
/// The body line height.
const BODY_LINE_HEIGHT: f32 = 1.6;
/// The tallest a standalone image renders, in root font sizes.
const IMAGE_MAX_HEIGHT: f32 = 14.0;
/// The side of an inline image atom relative to the body text size.
const INLINE_IMAGE_SCALE: f32 = 1.15;
/// The key context the text view declares while it holds focus.
pub const TEXT_VIEW_CONTEXT: &str = "TextView";

/// The callback a view invokes when the user clicks a markdown link.
pub type LinkClickHandler = Arc<dyn Fn(&SharedString, &mut Window, &mut App)>;
/// The callback a view invokes when the user toggles a task checkbox.
pub type TaskToggleHandler = Arc<dyn Fn(usize, bool, &mut Window, &mut App)>;

actions!(text_view, [Copy]);

/// A heading level, 1 to 6.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MarkdownHeading {
    H1,
    H2,
    H3,
    H4,
    H5,
    H6,
}

impl MarkdownHeading {
    /// The level as a number, 1 to 6.
    pub fn level(self) -> usize {
        match self {
            MarkdownHeading::H1 => 1,
            MarkdownHeading::H2 => 2,
            MarkdownHeading::H3 => 3,
            MarkdownHeading::H4 => 4,
            MarkdownHeading::H5 => 5,
            MarkdownHeading::H6 => 6,
        }
    }
}

/// The marker drawn before a list item.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ListMarker {
    /// An unordered bullet.
    Bullet,
    /// An ordered item, carrying its number in the list.
    Ordered(u64),
    /// A task item, `true` when checked.
    Task(bool),
}

/// One inline item of a paragraph or list item: styled text or an image.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum InlineItem {
    Text(InlineSpan),
    Image {
        /// Where the image loads from.
        source: SharedString,
        /// The image's alt text.
        alt: SharedString,
    },
}

/// The inline style a text run carries.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum InlineStyle {
    /// Plain body text.
    Text,
    /// Italic emphasis.
    Emphasis,
    /// Bold emphasis.
    Strong,
    /// Bold and italic together.
    StrongEmphasis,
    /// Struck-through text.
    Strikethrough,
    /// Inline code.
    Code,
    /// A link with its destination.
    Link { destination: SharedString },
}

/// One rendered block of a markdown document.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum MarkdownBlock {
    Heading {
        level: MarkdownHeading,
        spans: Vec<InlineSpan>,
        /// How many lists the heading's content sits under.
        indent: usize,
        /// Whether a blockquote wraps the heading.
        quote: bool,
    },
    Paragraph {
        items: Vec<InlineItem>,
        /// How many lists the paragraph's content sits under.
        indent: usize,
        /// Whether a blockquote wraps the paragraph.
        quote: bool,
    },
    ListItem {
        marker: ListMarker,
        items: Vec<InlineItem>,
        /// How many lists wrap this item's list, zero for the outermost.
        indent: usize,
        /// Whether a blockquote wraps the item.
        quote: bool,
    },
    /// An image that stands alone in its paragraph.
    Image {
        source: SharedString,
        alt: SharedString,
        /// How many lists the block sits under.
        indent: usize,
        /// Whether a blockquote wraps the block.
        quote: bool,
    },
    Code {
        language: Option<SharedString>,
        lines: HighlightedCode,
        /// How many lists the block sits under.
        indent: usize,
        /// Whether a blockquote wraps the block.
        quote: bool,
    },
    ThematicBreak {
        indent: usize,
        quote: bool,
    },
    Table {
        table: MarkdownTable,
        indent: usize,
        quote: bool,
    },
}

impl MarkdownBlock {
    /// Hashes the block so element ids track content, not list position.
    fn content_hash(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.hash(&mut hasher);
        hasher.finish()
    }
}

/// One run of inline text sharing a style.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct InlineSpan {
    /// The run's text.
    pub text: SharedString,
    /// The style applied to the text.
    pub style: InlineStyle,
}

/// One cell of a table row.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct MarkdownTableCell {
    pub spans: Vec<InlineSpan>,
}

/// A table row; the header row precedes the body rows.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct MarkdownTableRow {
    pub cells: Vec<MarkdownTableCell>,
    pub header: bool,
}

/// A table, header row first.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct MarkdownTable {
    pub rows: Vec<MarkdownTableRow>,
}

/// Retained state for a markdown view.
///
/// Build one in a [`gpui::Entity`] and embed it with [`TextView::new`], or
/// let [`TextView::markdown`] own its state under an element id.
pub struct MarkdownViewState {
    id: ElementId,
    source: SharedString,
    blocks: Vec<MarkdownBlock>,
    block_ids: Vec<ElementId>,
    /// Whether fenced code blocks highlight with the code view's palette.
    highlight_code: bool,
    /// Drives the vertical scroll of the virtualized block list.
    handle: ListState,
    a11y_label: Option<SharedString>,
    link_handler: Option<LinkClickHandler>,
    task_toggle_handler: Option<TaskToggleHandler>,
    /// `(anchor, head)` in the order they were set, over the rendered text.
    selection: Option<(usize, usize)>,
    /// Whether a drag is still extending the selection.
    selecting: bool,
    focus_handle: Option<gpui::FocusHandle>,
}

impl MarkdownViewState {
    /// Builds the state, parsing `source`.
    pub fn new<I, S>(id: I, source: S) -> Self
    where
        I: Into<ElementId>,
        S: Into<SharedString>,
    {
        let id = id.into();
        let source = source.into();
        let blocks = parse(&source, true);
        let block_ids = Self::stable_block_ids(&id, &blocks);
        let count = blocks.len();

        Self {
            id,
            source,
            blocks,
            block_ids,
            highlight_code: true,
            handle: ListState::new(count, ListAlignment::Top, CODE_OVERSCROLL_MAX),
            a11y_label: None,
            link_handler: None,
            task_toggle_handler: None,
            selection: None,
            selecting: false,
            focus_handle: None,
        }
    }

    /// Renders fenced code blocks without the code view's highlighting.
    pub fn plain_code(mut self) -> Self {
        self.highlight_code = false;
        self.reparse();
        self
    }

    /// Replaces the source and re-parses it; an equal source is a no-op.
    pub fn set_source<S>(&mut self, source: S, cx: &mut Context<Self>)
    where
        S: Into<SharedString>,
    {
        let source = source.into();
        if self.source == source {
            return;
        }
        self.source = source;
        self.reparse();
        cx.notify();
    }

    fn reparse(&mut self) {
        let blocks = parse(&self.source, self.highlight_code);
        let block_ids = Self::stable_block_ids(&self.id, &blocks);
        self.blocks = blocks;
        self.block_ids = block_ids;
        self.selection = None;
        self.selecting = false;
        self.handle.reset(self.blocks.len());
        self.handle.scroll_to_reveal_item(0);
    }

    /// Ids built from the block's content hash plus an ordinal among blocks of equal content.
    fn stable_block_ids(id: &ElementId, blocks: &[MarkdownBlock]) -> Vec<ElementId> {
        let mut ids = Vec::with_capacity(blocks.len());
        let mut occurrences = HashMap::with_capacity(blocks.len());
        for block in blocks {
            let hash = block.content_hash();
            let occurrence = *occurrences
                .entry(hash)
                .and_modify(|count| *count += 1)
                .or_insert(0);
            ids.push(ElementId::from(SharedString::from(format!(
                "{id}-block-{hash:x}-{occurrence}"
            ))));
        }
        ids
    }

    /// The markdown source the view renders.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// The parsed blocks.
    pub fn blocks(&self) -> &[MarkdownBlock] {
        &self.blocks
    }

    /// The stable element id of the block at `block_index`.
    pub fn block_id(&self, block_index: usize) -> Option<ElementId> {
        self.block_ids.get(block_index).cloned()
    }

    /// Whether fenced code highlights with the code view's palette.
    pub fn highlight_code(&self) -> bool {
        self.highlight_code
    }

    /// Scrolls `block` to the top of the view.
    pub fn scroll_to_block(&self, block: usize) {
        let item_ix = block.min(self.blocks.len().saturating_sub(1));
        self.handle.scroll_to(ListOffset {
            item_ix,
            offset_in_item: px(0.),
        });
    }

    /// Overrides the accessibility label of the view.
    pub fn accessibility_label<L>(mut self, label: L) -> Self
    where
        L: Into<SharedString>,
    {
        self.a11y_label = Some(label.into());
        self
    }

    /// Invokes `handler` when the user clicks a link, instead of opening it.
    pub fn on_link_click<F>(mut self, handler: F) -> Self
    where
        F: Fn(&SharedString, &mut Window, &mut App) + 'static,
    {
        self.link_handler = Some(Arc::new(handler));
        self
    }

    /// Replaces the link click handler.
    pub fn set_link_handler(&mut self, handler: LinkClickHandler) {
        self.link_handler = Some(handler);
    }

    /// Invokes `handler(block, checked)` when the user toggles a task checkbox.
    pub fn on_task_toggle<F>(mut self, handler: F) -> Self
    where
        F: Fn(usize, bool, &mut Window, &mut App) + 'static,
    {
        self.task_toggle_handler = Some(Arc::new(handler));
        self
    }

    /// Replaces the task toggle handler.
    pub fn set_task_toggle_handler(&mut self, handler: TaskToggleHandler) {
        self.task_toggle_handler = Some(handler);
    }

    /// Selects a range over the rendered text, endpoints in byte offsets.
    pub fn select_range(&mut self, start: usize, end: usize, cx: &mut Context<Self>) {
        self.selection = Some((start, end));
        self.selecting = false;
        cx.notify();
    }

    /// The selected range over the rendered text, endpoints in document
    /// order, when there is one.
    pub fn selection(&self) -> Option<(usize, usize)> {
        let (anchor, head) = self.selection?;
        Some(if anchor <= head {
            (anchor, head)
        } else {
            (head, anchor)
        })
    }

    /// The selected text, or `None` when nothing is selected.
    pub fn selected_text(&self) -> Option<String> {
        let document = text_document(&self.blocks);
        let (start, end) = self.selection()?;
        let end = end.min(document.len);
        let start = start.min(end);
        Some(document.text(&self.blocks)[start..end].to_string()).filter(|text| !text.is_empty())
    }

    /// The focus handle the view claims on click, created on first use.
    fn focus_handle(&mut self, cx: &mut App) -> gpui::FocusHandle {
        self.focus_handle
            .get_or_insert_with(|| cx.focus_handle().tab_stop(true))
            .clone()
    }
}

impl Render for MarkdownViewState {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let id = self.id.clone();
        let state = cx.entity();
        let focus_handle = self.focus_handle(cx);
        let frame = std::rc::Rc::new(ViewFrame {
            state: state.clone(),
            link_handler: self.link_handler.clone(),
            task_toggle: self.task_toggle_handler.clone(),
            selection: self.selection(),
            focus_handle,
            pressed_on_piece: std::rc::Rc::new(std::cell::Cell::new(false)),
        });
        let doc = std::rc::Rc::new(text_document(&self.blocks));

        let blocks = list(self.handle.clone(), {
            let frame = frame.clone();
            let doc = doc.clone();
            move |block_index, _window, cx| {
                let view = state.read(cx);
                match view
                    .blocks
                    .get(block_index)
                    .zip(view.block_ids.get(block_index))
                {
                    Some((block, block_id)) => {
                        let ctx = BlockContext {
                            frame: &frame,
                            doc: &doc,
                            block_index,
                            block_id,
                        };
                        render_block(block, block_index, &ctx, cx)
                    }
                    None => div().into_any_element(),
                }
            }
        })
        .size_full();

        view_container(&frame)
            .id((id.clone(), "view"))
            .relative()
            .size_full()
            .child(blocks)
            .child(
                Scrollbar::vertical((self.id.clone(), "v-scrollbar"), self.handle.clone())
                    .visibility(ScrollbarVisibility::Platform),
            )
            .when_some(self.a11y_label.clone(), |view, label| {
                view.aria_label(label)
            })
    }
}

/// The per-frame context the view's renderers wire themselves from.
struct ViewFrame {
    state: Entity<MarkdownViewState>,
    link_handler: Option<LinkClickHandler>,
    task_toggle: Option<TaskToggleHandler>,
    /// The selection over the rendered text, endpoints in document order.
    selection: Option<(usize, usize)>,
    focus_handle: gpui::FocusHandle,
    /// Set when a press landed on a piece.
    pressed_on_piece: std::rc::Rc<std::cell::Cell<bool>>,
}

/// The per-block context, one per block as it renders.
struct BlockContext<'a> {
    frame: &'a ViewFrame,
    doc: &'a TextDocument,
    block_index: usize,
    block_id: &'a ElementId,
}

impl BlockContext<'_> {
    /// The block's pieces, in render order.
    fn pieces(&self) -> &[TextPiece] {
        let start = self
            .doc
            .pieces
            .partition_point(|piece| piece.block_index < self.block_index);

        let count = self.doc.pieces[start..]
            .iter()
            .take_while(|piece| piece.block_index == self.block_index)
            .count();

        &self.doc.pieces[start..start + count]
    }

    /// The piece at `offset`.
    fn piece_context(&self, offset: usize) -> Option<PieceContext> {
        let piece = self.pieces().get(offset)?;
        let selected = self.frame.selection.and_then(|(start, end)| {
            let selected = start.max(piece.range.start)..end.min(piece.range.end);
            (!selected.is_empty())
                .then(|| (selected.start - piece.range.start)..(selected.end - piece.range.start))
        });

        Some(PieceContext {
            state: self.frame.state.clone(),
            link_handler: self.frame.link_handler.clone(),
            doc_range: piece.range.clone(),
            selected,
            pressed_on_piece: self.frame.pressed_on_piece.clone(),
        })
    }
}

/// What one selectable text element wires itself from.
struct PieceContext {
    state: Entity<MarkdownViewState>,
    link_handler: Option<LinkClickHandler>,
    /// The piece's byte range in the rendered text.
    doc_range: Range<usize>,
    /// The selection overlap, in piece-local bytes.
    selected: Option<Range<usize>>,
    pressed_on_piece: std::rc::Rc<std::cell::Cell<bool>>,
}

/// One selectable stretch of rendered text.
#[derive(Clone, Debug)]
pub struct TextPiece {
    /// The block the piece belongs to.
    pub block_index: usize,
    /// Which selectable unit of its block this is, in render order.
    pub sub_index: usize,
    /// The piece's byte range in the view's rendered text.
    pub range: Range<usize>,
}

/// The rendered text of a document: its selectable pieces and total length.
#[derive(Clone, Debug, Default)]
pub struct TextDocument {
    /// The pieces, in document order.
    pub pieces: Vec<TextPiece>,
    /// The rendered text length in bytes.
    pub len: usize,
}

impl TextDocument {
    /// The rendered text, built on demand.
    fn text(&self, blocks: &[MarkdownBlock]) -> String {
        let mut out = String::with_capacity(self.len);
        let mut piece_ix = 0;

        for (block_index, block) in blocks.iter().enumerate() {
            if piece_ix >= self.pieces.len() || self.pieces[piece_ix].block_index != block_index {
                continue;
            }

            let block_end = self.pieces[piece_ix..]
                .iter()
                .position(|piece| piece.block_index != block_index)
                .map_or(self.pieces.len(), |offset| piece_ix + offset);
            let units = block_text_units(block);

            for piece in &self.pieces[piece_ix..block_end] {
                let Some(unit) = units.get(piece.sub_index).filter(|unit| !unit.is_empty()) else {
                    continue;
                };
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(unit);
            }
            piece_ix = block_end;
        }

        out
    }
}

/// An embeddable markdown text view: fit-content height, no scrolling.
pub struct TextView {
    id: ElementId,
    source: Option<SharedString>,
    state: Option<Entity<MarkdownViewState>>,
    link_handler: Option<LinkClickHandler>,
    task_toggle_handler: Option<TaskToggleHandler>,
    style: StyleRefinement,
}

impl TextView {
    /// Renders `source`, keeping its parsed state across frames under `id`.
    pub fn markdown<I, S>(id: I, source: S) -> Self
    where
        I: Into<ElementId>,
        S: Into<SharedString>,
    {
        Self {
            id: id.into(),
            source: Some(source.into()),
            state: None,
            link_handler: None,
            task_toggle_handler: None,
            style: StyleRefinement::default(),
        }
    }

    /// Wraps an owned state; its source drives what the view renders.
    pub fn new(state: &Entity<MarkdownViewState>) -> Self {
        Self {
            id: ElementId::from(SharedString::from(format!("{}", state.entity_id()))),
            source: None,
            state: Some(state.clone()),
            link_handler: None,
            task_toggle_handler: None,
            style: StyleRefinement::default(),
        }
    }

    /// Invokes `handler` when the user clicks a link, instead of opening it.
    pub fn on_link_click<F>(mut self, handler: F) -> Self
    where
        F: Fn(&SharedString, &mut Window, &mut App) + 'static,
    {
        self.link_handler = Some(Arc::new(handler));
        self
    }

    /// Invokes `handler(block, checked)` when the user toggles a task checkbox.
    pub fn on_task_toggle<F>(mut self, handler: F) -> Self
    where
        F: Fn(usize, bool, &mut Window, &mut App) + 'static,
    {
        self.task_toggle_handler = Some(Arc::new(handler));
        self
    }
}

impl Styled for TextView {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl Element for TextView {
    type PrepaintState = ();
    type RequestLayoutState = AnyElement;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let state = match self.state.clone() {
            Some(state) => state,
            None => {
                let view_id = self.id.clone();
                let source = self.source.clone().unwrap_or_default();
                let state_key = SharedString::from(format!("{}/state", self.id));
                window.use_keyed_state(state_key, cx, move |_, _| {
                    MarkdownViewState::new(view_id, source)
                })
            }
        };

        let source = self.source.clone();
        let link_handler = self.link_handler.clone();
        let task_toggle = self.task_toggle_handler.clone();

        let (selection, focus_handle) =
            state.update(cx, |view, cx| (view.selection(), view.focus_handle(cx)));

        state.update(cx, |view, cx| {
            if let Some(source) = source {
                view.set_source(source, cx);
            }
            if let Some(link_handler) = link_handler {
                view.set_link_handler(link_handler);
            }
            if let Some(task_toggle) = task_toggle {
                view.set_task_toggle_handler(task_toggle);
            }
        });

        let view = state.read(cx);
        let frame = ViewFrame {
            state: state.clone(),
            link_handler: view.link_handler.clone(),
            task_toggle: view.task_toggle_handler.clone(),
            selection,
            focus_handle,
            pressed_on_piece: std::rc::Rc::new(std::cell::Cell::new(false)),
        };
        let doc = text_document(&view.blocks);

        let mut element = render_blocks(&view.blocks, &frame, &doc, &view.block_ids, cx);
        element.style().refine(&self.style);

        let mut element = element.into_any_element();
        let layout_id = element.request_layout(window, cx);

        (layout_id, element)
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) {
        request_layout.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        request_layout.paint(window, cx);
    }
}

/// One open list, tracking ordered numbering and an in-flight task marker.
struct ListFrame {
    ordered: bool,
    next_number: u64,
    task: Option<bool>,
}

/// Accumulates inline spans across inline events.
#[derive(Default)]
struct InlineBuilder {
    runs: Vec<(String, InlineStyle)>,
    styles: Vec<Option<InlineStyle>>,
}

impl InlineBuilder {
    fn push_style(&mut self, style: Option<InlineStyle>) {
        self.styles.push(style);
    }

    fn pop_style(&mut self) {
        self.styles.pop();
    }

    fn active(&self) -> InlineStyle {
        let mut strong = false;
        let mut emphasis = false;

        for style in self.styles.iter().rev().flatten() {
            match style {
                InlineStyle::Strong if emphasis => return InlineStyle::StrongEmphasis,
                InlineStyle::Strong => strong = true,
                InlineStyle::Emphasis if strong => return InlineStyle::StrongEmphasis,
                InlineStyle::Emphasis => emphasis = true,
                other => return other.clone(),
            }
        }

        match (strong, emphasis) {
            (true, true) => InlineStyle::StrongEmphasis,
            (true, false) => InlineStyle::Strong,
            (false, true) => InlineStyle::Emphasis,
            (false, false) => InlineStyle::Text,
        }
    }

    fn push_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let style = self.active();
        if let Some((run, run_style)) = self.runs.last_mut()
            && *run_style == style
        {
            run.push_str(text);
            return;
        }
        self.runs.push((text.to_string(), style));
    }

    /// Emits one span per contiguous style run.
    fn finish(&mut self) -> Vec<InlineSpan> {
        self.runs
            .drain(..)
            .map(|(text, style)| InlineSpan {
                text: SharedString::from(text),
                style,
            })
            .collect()
    }
}

/// Binds the text view's copy chord: cmd-c on macOS, ctrl-c elsewhere.
///
/// Call once at startup. Without it, selection copy has no key.
pub fn bind_text_view_keys(cx: &mut App) {
    let context = match KeyBindingContextPredicate::parse(TEXT_VIEW_CONTEXT) {
        Ok(context) => Some(Rc::new(context)),
        Err(error) => {
            log::error!("failed to parse text view key context: {error}");
            return;
        }
    };
    let binding = match KeyBinding::load("secondary-c", Box::new(Copy), context, false, None, cx) {
        Ok(binding) => binding,
        Err(error) => {
            log::error!("failed to parse text view copy keystroke: {error}");
            return;
        }
    };
    cx.bind_keys([binding]);
}

/// The view's base container: selection wiring, focus, and copy.
fn view_container(frame: &ViewFrame) -> Div {
    let clear_flag = frame.pressed_on_piece.clone();

    div()
        .w_full()
        .flex()
        .flex_col()
        .key_context(TEXT_VIEW_CONTEXT)
        .track_focus(&frame.focus_handle)
        .on_action({
            let state = frame.state.clone();
            move |_: &Copy, _window, cx| {
                if let Some(text) = state.read(cx).selected_text() {
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                }
            }
        })
        .on_mouse_down(MouseButton::Left, {
            let state = frame.state.clone();
            move |_, window, cx| {
                let focus_handle = state.update(cx, |view, cx| view.focus_handle(cx));
                window.focus(&focus_handle, cx);
                if !clear_flag.replace(false) {
                    state.update(cx, |view, cx| {
                        if view.selection.take().is_some() {
                            cx.notify();
                        }
                    });
                }
            }
        })
        .on_mouse_up(MouseButton::Left, {
            let state = frame.state.clone();
            move |_, _, cx| {
                if state.read(cx).selecting {
                    state.update(cx, |view, cx| {
                        view.selecting = false;
                        cx.notify();
                    });
                }
            }
        })
}

/// Renders every block stacked, without virtualization or scrolling.
fn render_blocks(
    blocks: &[MarkdownBlock],
    frame: &ViewFrame,
    doc: &TextDocument,
    block_ids: &[ElementId],
    cx: &App,
) -> Div {
    view_container(frame).children(blocks.iter().zip(block_ids).enumerate().map(
        |(block_index, (block, block_id))| {
            let ctx = BlockContext {
                frame,
                doc,
                block_index,
                block_id,
            };
            render_block(block, block_index, &ctx, cx)
        },
    ))
}

fn spans_text_of(spans: &[InlineSpan]) -> String {
    spans.iter().map(|span| span.text.as_ref()).collect()
}

/// The selectable text units of a block.
fn block_text_units(block: &MarkdownBlock) -> Vec<String> {
    match block {
        MarkdownBlock::Heading { spans, .. } => vec![spans_text_of(spans)],
        MarkdownBlock::Paragraph { items, .. } | MarkdownBlock::ListItem { items, .. } => {
            let mut units: Vec<String> = Vec::new();
            for item in items {
                match item {
                    InlineItem::Text(span) => match units.last_mut() {
                        // Consecutive text items share the unit between images.
                        Some(unit) => unit.push_str(&span.text),
                        None => units.push(span.text.as_ref().to_string()),
                    },
                    // An image starts the next unit; the render emits no
                    // element for it.
                    InlineItem::Image { .. } => units.push(String::new()),
                }
            }
            units
        }
        MarkdownBlock::Code { lines, .. } => lines.lines.iter().map(|line| line.text()).collect(),
        MarkdownBlock::Table { table, .. } => table
            .rows
            .iter()
            .flat_map(|row| row.cells.iter())
            .map(|cell| spans_text_of(&cell.spans))
            .collect(),
        MarkdownBlock::Image { .. } | MarkdownBlock::ThematicBreak { .. } => Vec::new(),
    }
}

/// Enumerates the selectable text pieces of `blocks`, in render order.
fn text_document(blocks: &[MarkdownBlock]) -> TextDocument {
    let mut pieces = Vec::new();
    let mut len = 0usize;

    for (block_index, block) in blocks.iter().enumerate() {
        // Paragraphs render no element for an empty unit.
        let skip_empty = matches!(
            block,
            MarkdownBlock::Paragraph { .. } | MarkdownBlock::ListItem { .. }
        );

        for (sub_index, unit) in block_text_units(block).into_iter().enumerate() {
            if unit.is_empty() && skip_empty {
                continue;
            }

            let mut range = len..len;
            if !unit.is_empty() {
                if len > 0 {
                    len += 1; // The newline that joins pieces.
                }
                range = len..len + unit.len();
                len = range.end;
            }

            pieces.push(TextPiece {
                block_index,
                sub_index,
                range,
            });
        }
    }

    TextDocument { pieces, len }
}

/// The markdown extensions the view parses, on top of CommonMark.
fn options() -> Options {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);
    options
}

fn heading(level: HeadingLevel) -> MarkdownHeading {
    match level {
        HeadingLevel::H1 => MarkdownHeading::H1,
        HeadingLevel::H2 => MarkdownHeading::H2,
        HeadingLevel::H3 => MarkdownHeading::H3,
        HeadingLevel::H4 => MarkdownHeading::H4,
        HeadingLevel::H5 => MarkdownHeading::H5,
        HeadingLevel::H6 => MarkdownHeading::H6,
    }
}

/// Parses `source` into blocks.
fn parse(source: &str, highlight_code: bool) -> Vec<MarkdownBlock> {
    let mut blocks = Vec::new();
    let mut inline = InlineBuilder::default();
    let mut inline_items: Vec<InlineItem> = Vec::new();
    let mut lists: Vec<ListFrame> = Vec::new();
    // Whether each open list item already flushed content rows.
    let mut items: Vec<bool> = Vec::new();
    let mut quote_depth = 0usize;
    let mut table: Option<Vec<MarkdownTableRow>> = None;
    let mut code: Option<String> = None;
    // The fence info string arrives on Start, its text follows.
    let mut code_language: Option<SharedString> = None;
    // The image being parsed: its source and its alt text so far.
    let mut image: Option<(SharedString, String)> = None;

    for event in Parser::new_ext(source, options()) {
        match event {
            Event::Start(tag) => match tag {
                Tag::Paragraph | Tag::TableCell | Tag::Item => {
                    inline = InlineBuilder::default();
                    inline_items = Vec::new();
                    if matches!(tag, Tag::Item) {
                        // The marker is only in flight while its own item is open.
                        if let Some(frame) = lists.last_mut() {
                            frame.task = None;
                        }
                        items.push(false);
                    }
                }
                Tag::Heading { level, .. } => {
                    flush_item_inline(
                        &mut inline,
                        &mut inline_items,
                        &mut items,
                        &mut lists,
                        &mut blocks,
                        quote_depth,
                    );
                    inline = InlineBuilder::default();
                    inline_items = Vec::new();
                    blocks.push(MarkdownBlock::Heading {
                        level: heading(level),
                        spans: Vec::new(),
                        indent: lists.len(),
                        quote: quote_depth > 0,
                    });
                }
                Tag::BlockQuote(_) => {
                    flush_item_inline(
                        &mut inline,
                        &mut inline_items,
                        &mut items,
                        &mut lists,
                        &mut blocks,
                        quote_depth,
                    );
                    quote_depth += 1;
                }
                Tag::List(start) => {
                    flush_item_inline(
                        &mut inline,
                        &mut inline_items,
                        &mut items,
                        &mut lists,
                        &mut blocks,
                        quote_depth,
                    );
                    lists.push(ListFrame {
                        ordered: start.is_some(),
                        next_number: start.unwrap_or_default(),
                        task: None,
                    });
                }
                Tag::CodeBlock(kind) => {
                    flush_item_inline(
                        &mut inline,
                        &mut inline_items,
                        &mut items,
                        &mut lists,
                        &mut blocks,
                        quote_depth,
                    );
                    code = Some(String::new());
                    if let CodeBlockKind::Fenced(info) = kind {
                        let info = info.trim();
                        code_language =
                            (!info.is_empty()).then(|| SharedString::from(info.to_string()));
                    }
                }
                Tag::Table(_) => {
                    flush_item_inline(
                        &mut inline,
                        &mut inline_items,
                        &mut items,
                        &mut lists,
                        &mut blocks,
                        quote_depth,
                    );
                    table = Some(Vec::new())
                }
                Tag::TableHead => {
                    if let Some(rows) = &mut table {
                        rows.push(MarkdownTableRow {
                            cells: Vec::new(),
                            header: true,
                        });
                    }
                }
                Tag::TableRow => {
                    if let Some(rows) = &mut table {
                        rows.push(MarkdownTableRow {
                            cells: Vec::new(),
                            header: false,
                        });
                    }
                }
                Tag::Emphasis | Tag::Strong | Tag::Strikethrough => {
                    let style = match tag {
                        Tag::Emphasis => InlineStyle::Emphasis,
                        Tag::Strong => InlineStyle::Strong,
                        _ => InlineStyle::Strikethrough,
                    };
                    inline.push_style(Some(style));
                }
                Tag::Link { dest_url, .. } => inline.push_style(Some(InlineStyle::Link {
                    destination: SharedString::from(dest_url.to_string()),
                })),
                Tag::Image { dest_url, .. } => {
                    image = Some((SharedString::from(dest_url.to_string()), String::new()))
                }
                Tag::HtmlBlock | Tag::MetadataBlock(_) => {}
                _ => {}
            },
            Event::End(tag_end) => match tag_end {
                TagEnd::Paragraph => {
                    let mut paragraph_items = std::mem::take(&mut inline_items);
                    for span in inline.finish() {
                        paragraph_items.push(InlineItem::Text(span));
                    }
                    let quote = quote_depth > 0;
                    match items.last_mut() {
                        // The item's first paragraph carries its marker row.
                        Some(flushed @ false) => {
                            *flushed = true;
                            blocks.push(MarkdownBlock::ListItem {
                                marker: take_marker(&mut lists),
                                items: paragraph_items,
                                indent: lists.len().saturating_sub(1),
                                quote,
                            });
                        }
                        _ => {
                            if let Some(item) = items.last_mut() {
                                *item = true;
                            }
                            push_paragraph_items(paragraph_items, lists.len(), quote, &mut blocks);
                        }
                    }
                }
                TagEnd::Heading(_) => {
                    let spans = inline.finish();
                    if let Some(MarkdownBlock::Heading { spans: slots, .. }) = blocks.last_mut() {
                        *slots = spans;
                    }
                }
                TagEnd::BlockQuote(_) => quote_depth = quote_depth.saturating_sub(1),
                TagEnd::List(_) => {
                    lists.pop();
                }
                TagEnd::Item => {
                    let flushed = items.pop().unwrap_or(false);
                    let mut block_items = std::mem::take(&mut inline_items);
                    for span in inline.finish() {
                        block_items.push(InlineItem::Text(span));
                    }

                    if block_items.is_empty() && flushed {
                        continue;
                    }

                    let indent = lists.len();
                    let quote = quote_depth > 0;

                    if flushed {
                        push_paragraph_items(block_items, indent, quote, &mut blocks);
                    } else {
                        blocks.push(MarkdownBlock::ListItem {
                            marker: take_marker(&mut lists),
                            items: block_items,
                            indent: indent.saturating_sub(1),
                            quote,
                        });
                    }
                }
                TagEnd::CodeBlock => {
                    let built = code.take().unwrap_or_default();
                    let text = built.trim_end_matches('\n');
                    let language = code_language.take();
                    let lines = match highlight_code
                        .then(|| language.as_deref().and_then(Language::from_fence_info))
                        .flatten()
                    {
                        Some(grammar) => highlight(text, grammar),
                        None => plain(text),
                    };
                    blocks.push(MarkdownBlock::Code {
                        language,
                        lines,
                        indent: lists.len(),
                        quote: quote_depth > 0,
                    });
                }
                TagEnd::Table => {
                    if let Some(rows) = table.take() {
                        blocks.push(MarkdownBlock::Table {
                            table: MarkdownTable { rows },
                            indent: lists.len(),
                            quote: quote_depth > 0,
                        });
                    }
                }
                TagEnd::TableCell => {
                    let spans = inline.finish();
                    let Some(rows) = &mut table else {
                        continue;
                    };
                    let Some(row) = rows.last_mut() else {
                        continue;
                    };
                    row.cells.push(MarkdownTableCell { spans });
                }
                TagEnd::Image => {
                    if let Some((source, alt)) = image.take() {
                        let in_cell = table.is_some();
                        let in_heading = matches!(
                            blocks.last(),
                            Some(MarkdownBlock::Heading { spans, .. }) if spans.is_empty()
                        );
                        if in_cell || in_heading {
                            // Headings and table cells keep images as alt text.
                            inline.push_text(&alt);
                        } else {
                            for span in inline.finish() {
                                inline_items.push(InlineItem::Text(span));
                            }
                            inline_items.push(InlineItem::Image {
                                source,
                                alt: SharedString::from(alt),
                            });
                        }
                    }
                }
                TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough | TagEnd::Link => {
                    inline.pop_style()
                }
                _ => {}
            },
            Event::Text(text) => {
                if let Some(buffer) = code.as_mut() {
                    buffer.push_str(&text);
                } else if let Some((_, alt)) = &mut image {
                    alt.push_str(&text);
                } else {
                    inline.push_text(&text);
                }
            }
            Event::Code(text) => {
                if let Some((_, alt)) = &mut image {
                    alt.push_str(&text);
                } else {
                    inline.push_style(Some(InlineStyle::Code));
                    inline.push_text(&text);
                    inline.pop_style();
                }
            }
            Event::SoftBreak | Event::HardBreak => {
                if let Some((_, alt)) = &mut image {
                    alt.push(' ');
                } else {
                    inline.push_text(" ");
                }
            }
            Event::Rule => {
                flush_item_inline(
                    &mut inline,
                    &mut inline_items,
                    &mut items,
                    &mut lists,
                    &mut blocks,
                    quote_depth,
                );
                blocks.push(MarkdownBlock::ThematicBreak {
                    indent: lists.len(),
                    quote: quote_depth > 0,
                });
            }
            Event::TaskListMarker(checked) => {
                if let Some(frame) = lists.last_mut() {
                    frame.task = Some(checked);
                }
            }
            // Footnotes are disabled in the parser options.
            Event::FootnoteReference(_)
            | Event::InlineHtml(_)
            | Event::Html(_)
            | Event::InlineMath(_)
            | Event::DisplayMath(_) => {}
        }
    }

    blocks
}

/// Computes the current item's marker and advances ordered numbering.
fn take_marker(lists: &mut [ListFrame]) -> ListMarker {
    let Some(frame) = lists.last_mut() else {
        return ListMarker::Bullet;
    };

    let marker = match frame.task {
        Some(checked) => ListMarker::Task(checked),
        None if frame.ordered => ListMarker::Ordered(frame.next_number),
        None => ListMarker::Bullet,
    };

    if frame.ordered {
        frame.next_number += 1;
    }

    marker
}

/// Pushes `items` as standalone image blocks when they hold nothing but images.
fn push_paragraph_items(
    items: Vec<InlineItem>,
    indent: usize,
    quote: bool,
    blocks: &mut Vec<MarkdownBlock>,
) {
    let blank = |item: &InlineItem| match item {
        InlineItem::Image { .. } => true,
        InlineItem::Text(span) => span.text.chars().all(char::is_whitespace),
    };
    let image_only = !items.is_empty()
        && items
            .iter()
            .any(|item| matches!(item, InlineItem::Image { .. }))
        && items.iter().all(blank);
    if image_only {
        for item in items {
            if let InlineItem::Image { source, alt } = item {
                blocks.push(MarkdownBlock::Image {
                    source,
                    alt,
                    indent,
                    quote,
                });
            }
        }
    } else {
        blocks.push(MarkdownBlock::Paragraph {
            items,
            indent,
            quote,
        });
    }
}

/// Flushes an open tight item's inline text as its marker row.
fn flush_item_inline(
    inline: &mut InlineBuilder,
    inline_items: &mut Vec<InlineItem>,
    items: &mut [bool],
    lists: &mut [ListFrame],
    blocks: &mut Vec<MarkdownBlock>,
    quote_depth: usize,
) {
    let Some(item) = items.last_mut() else {
        return;
    };

    let mut block_items = std::mem::take(inline_items);
    for span in inline.finish() {
        block_items.push(InlineItem::Text(span));
    }

    if block_items.is_empty() {
        return;
    }
    *item = true;

    blocks.push(MarkdownBlock::ListItem {
        marker: take_marker(lists),
        items: block_items,
        indent: lists.len().saturating_sub(1),
        quote: quote_depth > 0,
    });
}

/// Renders one parsed block as a list row.
fn render_block(
    block: &MarkdownBlock,
    block_index: usize,
    ctx: &BlockContext,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let gap = theme.root_font_size * BLOCK_GAP;

    match block {
        MarkdownBlock::Heading {
            level,
            spans,
            indent,
            quote,
        } => heading_row(
            *level,
            spans,
            *indent,
            *quote,
            if block_index == 0 { px(0.) } else { gap * 0.5 },
            ctx,
            cx,
        ),
        MarkdownBlock::Paragraph {
            items,
            indent,
            quote,
        } => text_row(items, *indent, *quote, gap, None, ctx, cx),
        MarkdownBlock::ListItem {
            marker,
            items,
            indent,
            quote,
        } => text_row(
            items,
            *indent,
            *quote,
            theme.root_font_size * ITEM_GAP,
            Some(*marker),
            ctx,
            cx,
        ),
        MarkdownBlock::Image {
            source,
            alt,
            indent,
            quote,
        } => image_row(source, alt, *indent, *quote, ctx, cx),
        MarkdownBlock::Code {
            language,
            lines,
            indent,
            quote,
        } => code_row(lines, language.as_ref(), *indent, *quote, ctx, cx),
        MarkdownBlock::ThematicBreak { indent, quote } => rule_row(*indent, *quote, ctx, cx),
        MarkdownBlock::Table {
            table,
            indent,
            quote,
        } => table_row(table, *indent, *quote, ctx, cx),
    }
}

/// The shared block frame: ambient text style, spacing, quote bar.
fn block_frame(quote: bool, top: Pixels, bottom: Pixels, id: &ElementId, cx: &App) -> Div {
    let theme = cx.theme();
    div()
        .debug_selector({
            let id = id.clone();
            move || id.to_string()
        })
        .w_full()
        .flex()
        .flex_col()
        .font_family(".SystemUIFont")
        .text_size(theme.sizes.small.text_size)
        .line_height(relative(BODY_LINE_HEIGHT))
        .text_color(theme.colors.foreground)
        .pt(top)
        .pb(bottom)
        .when(quote, |frame| {
            frame
                .border_l(BORDER_WIDTH * 2.0)
                .border_color(theme.colors.border)
                .pl(theme.root_font_size * QUOTE_INSET)
        })
}

/// Wraps the finished block in the list indent.
fn indented_frame(indent: usize, frame: Div, cx: &App) -> Div {
    let indent_px = cx.theme().root_font_size * INDENT_STEP * indent as f32;

    if indent_px > px(0.) {
        div().w_full().pl(indent_px).child(frame)
    } else {
        frame
    }
}

#[allow(clippy::too_many_arguments)]
fn heading_row(
    level: MarkdownHeading,
    spans: &[InlineSpan],
    indent: usize,
    quote: bool,
    top: Pixels,
    ctx: &BlockContext,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let role = match level {
        MarkdownHeading::H1 => TextRole::Title,
        MarkdownHeading::H2 => TextRole::Title2,
        MarkdownHeading::H3 => TextRole::Title3,
        MarkdownHeading::H4 | MarkdownHeading::H5 | MarkdownHeading::H6 => TextRole::Headline,
    };

    indented_frame(
        indent,
        block_frame(
            quote,
            top,
            theme.root_font_size * BLOCK_GAP,
            ctx.block_id,
            cx,
        )
        .typeset(role, cx)
        .child(text_child(spans, ctx.block_id, ctx.piece_context(0), cx)),
        cx,
    )
    .into_any_element()
}

fn text_row(
    items: &[InlineItem],
    indent: usize,
    quote: bool,
    bottom: Pixels,
    marker: Option<ListMarker>,
    ctx: &BlockContext,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();

    indented_frame(
        indent,
        block_frame(quote, px(0.), bottom, ctx.block_id, cx).map(|frame| match marker {
            Some(marker) => {
                let line_height = theme.sizes.small.text_size * BODY_LINE_HEIGHT;
                let marker_cell = div()
                    .flex_none()
                    .w(theme.root_font_size * MARKER_WIDTH)
                    .h(line_height)
                    .flex()
                    .items_center()
                    .justify_end()
                    .pr_1()
                    .child(match marker {
                        ListMarker::Bullet => div()
                            .size_1()
                            .rounded(theme.radius.full)
                            .bg(theme.colors.muted_foreground)
                            .into_any_element(),
                        ListMarker::Ordered(number) => div()
                            .text_color(theme.colors.muted_foreground)
                            .whitespace_nowrap()
                            .child(format!("{number}."))
                            .into_any_element(),
                        ListMarker::Task(checked) => task_toggle(checked, item_text(items), ctx),
                    });

                frame.child(
                    div().flex().items_start().child(marker_cell).child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(paragraph_child(items, ctx, cx)),
                    ),
                )
            }
            None => frame.child(paragraph_child(items, ctx, cx)),
        }),
        cx,
    )
    .into_any_element()
}

/// The task marker: a checkbox that reports toggles when a handler is set.
fn task_toggle(checked: bool, label: SharedString, ctx: &BlockContext) -> AnyElement {
    let block_index = ctx.block_index;
    let selector = SharedString::from(format!("task-toggle-{block_index}"));
    let checkbox = Checkbox::new((ctx.block_id.clone(), "task"))
        .debug_selector(move || selector.to_string())
        .checked(checked)
        .with_size(Size::Small)
        .accessibility_label(label);

    match ctx.frame.task_toggle.clone() {
        Some(handler) => checkbox
            .on_change(move |next, window, cx| handler(block_index, next, window, cx))
            .into_any_element(),
        None => checkbox.disabled(true).into_any_element(),
    }
}

/// The item's plain text, for the checkbox's accessibility label.
fn item_text(items: &[InlineItem]) -> SharedString {
    let text = items
        .iter()
        .filter_map(|item| match item {
            InlineItem::Text(span) => Some(span.text.as_ref()),
            InlineItem::Image { .. } => None,
        })
        .collect::<Vec<_>>()
        .join("");
    SharedString::from(text)
}

/// Builds a paragraph's content.
fn paragraph_child(items: &[InlineItem], ctx: &BlockContext, cx: &App) -> AnyElement {
    let has_image = items
        .iter()
        .any(|item| matches!(item, InlineItem::Image { .. }));

    if !has_image {
        let spans = items.iter().filter_map(|item| match item {
            InlineItem::Text(span) => Some(span),
            InlineItem::Image { .. } => None,
        });
        return text_child(spans, ctx.block_id, ctx.piece_context(0), cx);
    }

    let theme = cx.theme();
    let side = theme.sizes.small.text_size * INLINE_IMAGE_SCALE;
    let mut flow = div().flex().flex_wrap().items_center();
    let mut segment: Vec<&InlineSpan> = Vec::new();
    let mut segment_index = 0;
    for item in items {
        match item {
            InlineItem::Text(span) => segment.push(span),
            InlineItem::Image { source, .. } => {
                if !segment.is_empty() {
                    flow = flow.child(text_child(
                        segment,
                        &text_segment_id(ctx.block_id, segment_index),
                        ctx.piece_context(segment_index),
                        cx,
                    ));
                    segment = Vec::new();
                    segment_index += 1;
                }
                flow = flow.child(
                    img(source.clone())
                        .size(side)
                        .object_fit(ObjectFit::ScaleDown)
                        .flex_none(),
                );
            }
        }
    }
    if !segment.is_empty() {
        flow = flow.child(text_child(
            segment,
            &text_segment_id(ctx.block_id, segment_index),
            ctx.piece_context(segment_index),
            cx,
        ));
    }
    flow.into_any_element()
}

/// A standalone image block, capped in height.
fn image_row(
    source: &SharedString,
    alt: &SharedString,
    indent: usize,
    quote: bool,
    ctx: &BlockContext,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    indented_frame(
        indent,
        block_frame(
            quote,
            px(0.),
            theme.root_font_size * BLOCK_GAP,
            ctx.block_id,
            cx,
        )
        .child(
            img(source.clone())
                .max_h(theme.root_font_size * IMAGE_MAX_HEIGHT)
                .max_w_full()
                .rounded(theme.radius.control)
                .overflow_hidden(),
        ),
        cx,
    )
    .id((ctx.block_id.clone(), "image"))
    .when(!alt.is_empty(), |frame| frame.aria_label(alt.clone()))
    .into_any_element()
}

/// The element id of one text segment of a paragraph with images.
fn text_segment_id(id: &ElementId, segment_index: usize) -> ElementId {
    ElementId::from(SharedString::from(format!("{id}-text-{segment_index}")))
}

fn code_row(
    lines: &HighlightedCode,
    language: Option<&SharedString>,
    indent: usize,
    quote: bool,
    ctx: &BlockContext,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let text_size = theme.sizes.small.text_size;
    let line_height = text_size * CODE_LINE_HEIGHT;
    let plain = lines
        .lines
        .iter()
        .map(|line| line.text())
        .collect::<Vec<_>>()
        .join("\n");

    let actions_row = div()
        .flex()
        .items_center()
        .justify_between()
        .pb_1()
        .when_some(language.cloned(), |row, language| {
            row.child(
                div()
                    .typeset(TextRole::Caption, cx)
                    .text_color(theme.colors.muted_foreground)
                    .child(language),
            )
        })
        .child(
            Button::new((ctx.block_id.clone(), "copy-code"))
                .label("Copy")
                .variant(ButtonVariant::Ghost)
                .debug_selector(|| "copy-code".to_string())
                .on_click(move |_, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(plain.clone()));
                }),
        );

    let plate = div()
        .w_full()
        .flex()
        .flex_col()
        .rounded(theme.radius.control)
        .bg(theme.colors.muted)
        .p_2()
        .font_family(theme.mono_font)
        .text_size(text_size)
        .line_height(relative(CODE_LINE_HEIGHT))
        .overflow_hidden()
        .child(actions_row)
        .children(lines.lines.iter().enumerate().map(|(line_ix, line)| {
            let styled = code_line(line, theme.colors, theme.ramps);
            let layout = styled.layout().clone();
            with_selection_handlers(
                div()
                    .h(line_height)
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .child(styled),
                ctx.piece_context(line_ix),
                layout,
            )
        }));

    indented_frame(
        indent,
        block_frame(
            quote,
            px(0.),
            theme.root_font_size * BLOCK_GAP,
            ctx.block_id,
            cx,
        )
        .child(plate),
        cx,
    )
    .into_any_element()
}

fn rule_row(indent: usize, quote: bool, ctx: &BlockContext, cx: &App) -> AnyElement {
    let theme = cx.theme();
    indented_frame(
        indent,
        block_frame(
            quote,
            px(0.),
            theme.root_font_size * BLOCK_GAP,
            ctx.block_id,
            cx,
        )
        .child(div().h(BORDER_WIDTH).w_full().bg(theme.colors.border)),
        cx,
    )
    .into_any_element()
}

fn table_row(
    table: &MarkdownTable,
    indent: usize,
    quote: bool,
    ctx: &BlockContext,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let colors = theme.colors;
    let width = table
        .rows
        .iter()
        .map(|row| row.cells.len())
        .max()
        .unwrap_or(0);
    let row_count = table.rows.len();
    let mut cell_ix = 0usize;

    let grid = div()
        .w_full()
        .flex()
        .flex_col()
        .border_1()
        .border_color(colors.border)
        .rounded(theme.radius.control)
        .overflow_hidden()
        .children(table.rows.iter().enumerate().map(|(row_index, row)| {
            div()
                .w_full()
                .flex()
                .when(row_index + 1 < row_count, |row_element| {
                    row_element.border_b_1().border_color(colors.border)
                })
                .when(row.header, |row_element| row_element.bg(colors.muted))
                .children(
                    row.cells
                        .iter()
                        .enumerate()
                        .map(|(column_index, cell)| {
                            let cell_id = ElementId::from(SharedString::from(format!(
                                "{}-cell-{row_index}-{column_index}",
                                ctx.block_id
                            )));
                            let piece = ctx.piece_context(cell_ix);
                            cell_ix += 1;
                            div()
                                .flex_1()
                                .min_w_0()
                                .px_2()
                                .py_1()
                                .when(row.header, |cell_element| {
                                    cell_element.font_weight(theme.typography.headline.weight)
                                })
                                .child(text_child(&cell.spans, &cell_id, piece, cx))
                        })
                        // Ragged tables pad missing cells so rows align.
                        .chain(
                            (row.cells.len()..width)
                                .map(|_| div().flex_1().min_w_0().px_2().py_1()),
                        ),
                )
        }));

    indented_frame(
        indent,
        block_frame(
            quote,
            px(0.),
            theme.root_font_size * BLOCK_GAP,
            ctx.block_id,
            cx,
        )
        .child(grid),
        cx,
    )
    .into_any_element()
}

/// Attaches drag-selection handlers to `element` when it renders a piece.
fn with_selection_handlers(element: Div, piece: Option<PieceContext>, layout: TextLayout) -> Div {
    let Some(piece) = piece else {
        return element;
    };

    let state = piece.state.clone();
    let doc_range = piece.doc_range.clone();
    let pressed_on_piece = piece.pressed_on_piece.clone();
    let layout_for_move = layout.clone();
    element
        .on_mouse_down(
            MouseButton::Left,
            move |event: &MouseDownEvent, window: &mut Window, cx: &mut App| {
                pressed_on_piece.set(true);
                let focus_handle = state.update(cx, |view, cx| view.focus_handle(cx));
                window.focus(&focus_handle, cx);

                let local = match layout.index_for_position(event.position) {
                    Ok(index) | Err(index) => index,
                };
                let index = doc_range.start + local;
                state.update(cx, |view, cx| {
                    if event.modifiers.shift {
                        if let Some((anchor, _)) = view.selection {
                            view.selection = Some((anchor, index));
                        } else {
                            view.selection = Some((index, index));
                        }
                    } else {
                        view.selection = Some((index, index));
                    }
                    view.selecting = true;
                    cx.notify();
                });
            },
        )
        .on_mouse_move({
            let state = piece.state.clone();
            let doc_range = piece.doc_range.clone();
            let layout = layout_for_move;
            move |event: &MouseMoveEvent, _window: &mut Window, cx: &mut App| {
                if !state.read(cx).selecting {
                    return;
                }
                let local = match layout.index_for_position(event.position) {
                    Ok(index) | Err(index) => index,
                };
                let index = doc_range.start + local;
                let changed = state
                    .read(cx)
                    .selection
                    .map(|(_, head)| head != index)
                    .unwrap_or(true);
                if changed {
                    state.update(cx, |view, cx| {
                        if let Some((anchor, _)) = view.selection {
                            view.selection = Some((anchor, index));
                            cx.notify();
                        }
                    });
                }
            }
        })
        .on_mouse_up(MouseButton::Left, {
            let state = piece.state.clone();
            move |_, _, cx| {
                if state.read(cx).selecting {
                    state.update(cx, |view, cx| {
                        view.selecting = false;
                        cx.notify();
                    });
                }
            }
        })
}

/// Builds the block's text element, selectable when it renders a piece.
fn text_child<I>(spans: I, id: &ElementId, piece: Option<PieceContext>, cx: &App) -> AnyElement
where
    I: IntoIterator,
    I::Item: std::borrow::Borrow<InlineSpan>,
{
    let theme = cx.theme();
    let mut text = String::new();
    let mut highlights: Vec<(Range<usize>, HighlightStyle)> = Vec::new();
    let mut family_overrides: Vec<(Range<usize>, SharedString)> = Vec::new();
    let mut link_ranges: Vec<Range<usize>> = Vec::new();
    let mut destinations: Vec<SharedString> = Vec::new();

    for span in spans {
        let span = span.borrow();
        let start = text.len();
        text.push_str(&span.text);

        let range = start..text.len();

        match &span.style {
            InlineStyle::Text => {}
            InlineStyle::Emphasis => highlights.push((
                range,
                HighlightStyle {
                    font_style: Some(FontStyle::Italic),
                    ..Default::default()
                },
            )),
            InlineStyle::Strong => highlights.push((
                range,
                HighlightStyle {
                    font_weight: Some(theme.typography.headline.weight),
                    ..Default::default()
                },
            )),
            InlineStyle::StrongEmphasis => highlights.push((
                range,
                HighlightStyle {
                    font_style: Some(FontStyle::Italic),
                    font_weight: Some(theme.typography.headline.weight),
                    ..Default::default()
                },
            )),
            InlineStyle::Strikethrough => highlights.push((
                range,
                HighlightStyle {
                    strikethrough: Some(StrikethroughStyle {
                        thickness: BORDER_WIDTH,
                        color: Some(theme.colors.muted_foreground),
                    }),
                    ..Default::default()
                },
            )),
            InlineStyle::Code => {
                highlights.push((
                    range.clone(),
                    HighlightStyle {
                        background_color: Some(theme.colors.muted),
                        ..Default::default()
                    },
                ));
                family_overrides.push((range, SharedString::from(theme.mono_font)));
            }
            InlineStyle::Link { destination } => {
                highlights.push((
                    range.clone(),
                    HighlightStyle {
                        color: Some(theme.ramps.primary.base),
                        underline: Some(UnderlineStyle {
                            thickness: BORDER_WIDTH,
                            color: None,
                            wavy: false,
                        }),
                        ..Default::default()
                    },
                ));
                link_ranges.push(range);
                destinations.push(destination.clone());
            }
        }
    }

    if let Some(local) = piece.as_ref().and_then(|piece| piece.selected.clone()) {
        highlights.push((
            local,
            HighlightStyle {
                background_color: Some(theme.colors.selection),
                ..Default::default()
            },
        ));
    }

    let styled = StyledText::new(text)
        .with_highlights(highlights)
        .with_font_family_overrides(family_overrides);
    let layout = styled.layout().clone();

    let inner = if destinations.is_empty() {
        styled.into_any_element()
    } else {
        let link_handler = piece.as_ref().and_then(|piece| piece.link_handler.clone());
        InteractiveText::new(id.clone(), styled)
            .on_click(link_ranges, move |range_ix, window, cx| {
                let Some(destination) = destinations.get(range_ix) else {
                    return;
                };
                match link_handler.as_ref() {
                    Some(handler) => handler(destination, window, cx),
                    None => cx.open_url(destination.as_ref()),
                }
            })
            .into_any_element()
    };

    let mut element = div().child(inner);
    if let Some(piece) = piece {
        element = with_selection_handlers(element, Some(piece), layout);
    }
    element.into_any_element()
}

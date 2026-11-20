//! Renders a unified, syntax-highlighted diff of two versions of a file:
//! change bars or `+`/`-` markers, tinted number gutters, word-level
//! intra-line highlights, and collapsed hunk separators.
//!
//! Rows reuse the code view's highlighting. Consumers that already hold a
//! git diff can build [`DiffRow`]s themselves instead of calling
//! [`DiffViewState::diff_rows`].
//!
//! # Examples
//!
//! ```no_run
//! use gpui::{App, AppContext as _};
//! use ui::diff::{DiffIndicators, DiffViewState};
//! use ui::highlight::Language;
//!
//! fn patch(cx: &mut App) -> gpui::Entity<DiffViewState> {
//!     let state = DiffViewState::new(
//!         "patch",
//!         "# gpui\nA UI kit for gpui.",
//!         "# gpui\nA UI toolkit for gpui.",
//!         Language::Rust,
//!     )
//!     .indicators(DiffIndicators::Bars);
//!
//!     cx.new(|_| state)
//! }
//! ```

use std::ops::Range;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, Context, ElementId, HighlightStyle, InteractiveElement as _, IntoElement,
    ListHorizontalSizingBehavior, ParentElement as _, Pixels, Render, ScrollStrategy, SharedString,
    StatefulInteractiveElement as _, Styled as _, TextStyle, UniformListScrollHandle, Window, div,
    point, px, rems, uniform_list,
};
use similar::{Change, ChangeTag, TextDiff};

use crate::components::highlight::{HighlightedCode, Language, Line, Role, highlight};
use crate::components::scrollbar::{Scrollbar, ScrollbarHandle, ScrollbarVisibility};
use crate::components::utils::{gutter_cell, wash, wash_gutter, wash_strong};
use crate::theme::{ActiveTheme as _, CODE_GUTTER_WIDTH, CODE_LINE_HEIGHT, MONO_ADVANCE_RATIO};

/// The left change-indicator bar's width, four hairlines.
const CHANGE_BAR_WIDTH: Pixels = px(4.0);

/// What a diff row did to the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffRowKind {
    /// Present in both versions.
    Context,
    /// Only in the new version.
    Added,
    /// Only in the old version.
    Removed,
    /// A hunk separator introducing the rows after it.
    Hunk,
}

/// One row of a unified diff: a diff line or a hunk separator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffRow {
    pub kind: DiffRowKind,
    /// The highlighted line content; empty for hunk separators.
    pub content: Line,
    /// 1-based line number in the old version, when the row exists there.
    pub old_number: Option<usize>,
    /// 1-based line number in the new version, when the row exists there.
    pub new_number: Option<usize>,
    /// Byte ranges of intra-line word changes, tinted on changed rows.
    pub changed_ranges: Vec<Range<usize>>,
    /// The `@@ -a,b +c,d @@` header, for hunk separator rows.
    pub header: SharedString,
    /// Unchanged lines collapsed into this separator row; zero elsewhere.
    pub hidden_lines: usize,
}

/// How changed rows are marked.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DiffIndicators {
    /// A two-pixel bar on the row's left edge.
    #[default]
    Bars,
    /// Classic `+`/`-` glyphs in a marker column.
    Classic,
    /// Only the background washes.
    None,
}

/// How collapsed regions between hunks render.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HunkSeparator {
    /// The collapsed unchanged line count, centered.
    #[default]
    LineInfo,
    /// The patch-style `@@ -a,b +c,d @@` header.
    Metadata,
    /// A minimal `···` bar.
    Simple,
}

/// Presentation options for the diff view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiffViewOptions {
    /// How changed rows are marked.
    pub indicators: DiffIndicators,
    /// How collapsed regions between hunks render.
    pub separator: HunkSeparator,
    /// Whether the line-number gutter renders.
    pub line_numbers: bool,
}

impl Default for DiffViewOptions {
    fn default() -> Self {
        Self {
            indicators: DiffIndicators::Bars,
            separator: HunkSeparator::LineInfo,
            line_numbers: true,
        }
    }
}

impl DiffRow {
    /// Assembles a diff line row from a `similar` change.
    pub(crate) fn from_change(
        change: &Change<&str>,
        code: &HighlightedCode,
        changed_ranges: Vec<Range<usize>>,
    ) -> DiffRow {
        let content = match change.tag() {
            ChangeTag::Equal => change
                .new_index()
                .and_then(|ix| code.lines.get(ix))
                .cloned()
                .unwrap_or_default(),
            ChangeTag::Delete => change
                .old_index()
                .and_then(|ix| code.lines.get(ix))
                .cloned()
                .unwrap_or_default(),
            ChangeTag::Insert => change
                .new_index()
                .and_then(|ix| code.lines.get(ix))
                .cloned()
                .unwrap_or_default(),
        };

        let (old_number, new_number) = match change.tag() {
            ChangeTag::Equal => (
                change.old_index().map(|ix| ix + 1),
                change.new_index().map(|ix| ix + 1),
            ),
            ChangeTag::Delete => (change.old_index().map(|ix| ix + 1), None),
            ChangeTag::Insert => (None, change.new_index().map(|ix| ix + 1)),
        };

        DiffRow {
            kind: match change.tag() {
                ChangeTag::Equal => DiffRowKind::Context,
                ChangeTag::Delete => DiffRowKind::Removed,
                ChangeTag::Insert => DiffRowKind::Added,
            },
            content,
            old_number,
            new_number,
            changed_ranges,
            header: Default::default(),
            hidden_lines: 0,
        }
    }

    /// The row's text length, in characters.
    fn char_count(&self) -> usize {
        self.content.text().chars().count()
    }

    /// Stable element id: the kind and line numbers packed into one integer.
    fn element_id(&self, hunk_ordinal: usize) -> ElementId {
        let kind = match self.kind {
            DiffRowKind::Context => 0u64,
            DiffRowKind::Added => 1,
            DiffRowKind::Removed => 2,
            DiffRowKind::Hunk => {
                return ElementId::named_usize("diff-hunk", hunk_ordinal);
            }
        };
        // Line numbers saturate at 2^31 so the two fields never overlap.
        let old = self.old_number.unwrap_or(0).min((1 << 31) - 1) as u64;
        let new = self.new_number.unwrap_or(0).min((1 << 31) - 1) as u64;
        ElementId::named_usize("diff-row", ((kind << 62) | (old << 31) | new) as usize)
    }
}

/// Retained state for a diff view.
///
/// Build one in a [`gpui::Entity`], render the entity, and call its setters
/// to update the rendered view.
pub struct DiffViewState {
    id: ElementId,
    rows: Vec<DiffRow>,
    /// Cached per-row id and text length, parallel to `rows`.
    row_layouts: Vec<RowLayout>,
    /// The row whose width drives horizontal scroll extent, `rows`-derived.
    widest_row: Option<usize>,
    options: DiffViewOptions,
    /// Drives both scroll axes of the virtualized row list.
    handle: UniformListScrollHandle,
    a11y_label: Option<SharedString>,
}

/// Cached per-row render data, computed once when the rows are set.
struct RowLayout {
    id: ElementId,
    char_count: usize,
}

impl DiffViewState {
    /// Builds the state, diffing `old` against `new`.
    pub fn new<I, O, N>(id: I, old: O, new: N, language: Language) -> Self
    where
        I: Into<ElementId>,
        O: AsRef<str>,
        N: AsRef<str>,
    {
        let mut state = Self {
            id: id.into(),
            rows: Vec::new(),
            row_layouts: Vec::new(),
            widest_row: None,
            options: DiffViewOptions::default(),
            handle: UniformListScrollHandle::new(),
            a11y_label: None,
        };
        state.set_rows(old, new, language);
        state
    }

    /// Replaces the rows, recomputing the per-row layout cache with them.
    fn set_rows<O, N>(&mut self, old: O, new: N, language: Language)
    where
        O: AsRef<str>,
        N: AsRef<str>,
    {
        self.rows = Self::diff_rows(old.as_ref(), new.as_ref(), language);

        let mut hunk_ordinal = 0usize;
        self.row_layouts = self
            .rows
            .iter()
            .map(|row| {
                let ordinal = if row.kind == DiffRowKind::Hunk {
                    hunk_ordinal += 1;
                    hunk_ordinal
                } else {
                    0
                };
                RowLayout {
                    id: row.element_id(ordinal),
                    char_count: row.char_count(),
                }
            })
            .collect();

        self.widest_row = self
            .rows
            .iter()
            .zip(&self.row_layouts)
            .enumerate()
            .filter(|(_, (row, _))| row.kind != DiffRowKind::Hunk)
            .max_by_key(|(_, (_, layout))| layout.char_count)
            .map(|(widest, _)| widest);
    }

    /// Computes unified rows between two texts, highlighting every line with
    /// `language`. Rows include [`DiffRowKind::Hunk`] separators between
    /// hunks.
    pub fn diff_rows(old: &str, new: &str, language: Language) -> Vec<DiffRow> {
        let old_code = highlight(old, language);
        let new_code = highlight(new, language);
        let diff = TextDiff::from_lines(old, new);

        let mut rows = Vec::new();
        let mut collapsed_before = 0usize;

        for group in diff.grouped_ops(3) {
            let (Some(first), Some(last)) = (group.first(), group.last()) else {
                continue;
            };

            let old_range = first.old_range().start..last.old_range().end;
            let new_range = first.new_range().start..last.new_range().end;

            // git prints the line before a pure insertion when the count is zero.
            let old_start = if old_range.is_empty() {
                old_range.start
            } else {
                old_range.start + 1
            };

            let new_start = if new_range.is_empty() {
                new_range.start
            } else {
                new_range.start + 1
            };

            let collapsed = first.old_range().start.saturating_sub(collapsed_before);
            if collapsed > 0 {
                rows.push(DiffRow {
                    kind: DiffRowKind::Hunk,
                    content: Line::default(),
                    old_number: None,
                    new_number: None,
                    changed_ranges: Vec::new(),
                    header: format!(
                        "@@ -{},{} +{},{} @@",
                        old_start,
                        old_range.len(),
                        new_start,
                        new_range.len()
                    )
                    .into(),
                    hidden_lines: collapsed,
                });
            }
            collapsed_before = last.old_range().end;

            for op in &group {
                let changes: Vec<_> = diff.iter_changes(op).collect();
                let mut index = 0;

                while index < changes.len() {
                    if changes[index].tag() == ChangeTag::Equal {
                        rows.push(DiffRow::from_change(&changes[index], &new_code, Vec::new()));
                        index += 1;
                        continue;
                    }

                    let deletions = changes[index..]
                        .iter()
                        .take_while(|change| change.tag() == ChangeTag::Delete)
                        .count();

                    let insertions = changes[index + deletions..]
                        .iter()
                        .take_while(|change| change.tag() == ChangeTag::Insert)
                        .count();

                    let mut deletion_ranges = vec![Vec::new(); deletions];
                    let mut insertion_ranges = vec![Vec::new(); insertions];

                    for pair in 0..deletions.min(insertions) {
                        let old_text = changes[index + pair].value().trim_end_matches('\n');
                        let new_text = changes[index + deletions + pair]
                            .value()
                            .trim_end_matches('\n');
                        let words = TextDiff::from_words(old_text, new_text);

                        let (mut old_offset, mut new_offset) = (0usize, 0usize);

                        for word in words.iter_all_changes() {
                            let len = word.value().len();

                            match word.tag() {
                                ChangeTag::Delete => {
                                    deletion_ranges[pair].push(old_offset..old_offset + len);
                                    old_offset += len;
                                }
                                ChangeTag::Insert => {
                                    insertion_ranges[pair].push(new_offset..new_offset + len);
                                    new_offset += len;
                                }
                                ChangeTag::Equal => {
                                    old_offset += len;
                                    new_offset += len;
                                }
                            }
                        }
                    }

                    for pair in 0..deletions {
                        rows.push(DiffRow::from_change(
                            &changes[index + pair],
                            &old_code,
                            deletion_ranges[pair].clone(),
                        ));
                    }

                    for pair in 0..insertions {
                        rows.push(DiffRow::from_change(
                            &changes[index + deletions + pair],
                            &new_code,
                            insertion_ranges[pair].clone(),
                        ));
                    }

                    index += deletions + insertions;
                }
            }
        }

        rows
    }

    /// Sets how changed rows are marked.
    pub fn indicators(mut self, indicators: DiffIndicators) -> Self {
        self.options.indicators = indicators;
        self
    }

    /// Sets how collapsed regions between hunks render.
    pub fn hunk_separator(mut self, separator: HunkSeparator) -> Self {
        self.options.separator = separator;
        self
    }

    /// Shows or hides the line-number gutter.
    pub fn line_numbers(mut self, show: bool) -> Self {
        self.options.line_numbers = show;
        self
    }

    /// Replaces the versions and re-diffs them.
    pub fn set_versions<O, N>(&mut self, old: O, new: N, language: Language, cx: &mut Context<Self>)
    where
        O: AsRef<str>,
        N: AsRef<str>,
    {
        self.set_rows(old, new, language);
        self.handle.scroll_to_item(0, ScrollStrategy::Top);
        // The replaced versions rarely share the old width; start at the left.
        let offset = ScrollbarHandle::offset(&self.handle);
        ScrollbarHandle::set_offset(&self.handle, point(Pixels::ZERO, offset.y));
        cx.notify();
    }

    /// The diff rows.
    pub fn rows(&self) -> &[DiffRow] {
        &self.rows
    }

    /// Scrolls the first added or removed row to the top of the view.
    pub fn scroll_to_first_change(&self) {
        let Some(row) = self
            .rows
            .iter()
            .position(|row| matches!(row.kind, DiffRowKind::Added | DiffRowKind::Removed))
        else {
            return;
        };
        self.handle.scroll_to_item(row, ScrollStrategy::Top);
    }

    /// Overrides the accessibility label of the view.
    pub fn accessibility_label<L>(mut self, label: L) -> Self
    where
        L: Into<SharedString>,
    {
        self.a11y_label = Some(label.into());
        self
    }
}

impl Render for DiffViewState {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let text_size = rems(0.75).to_pixels(window.rem_size());

        let id = self.id.clone();
        let state = cx.entity();
        let row_count = self.rows.len();
        let options = self.options;
        let widest_row = self.widest_row;

        let mut rows = uniform_list((id.clone(), "rows"), row_count, {
            move |range, _window, cx| {
                let diff = state.read(cx);
                range
                    .map(|row_index| {
                        render_row(
                            &diff.rows[row_index],
                            &diff.row_layouts[row_index],
                            row_index,
                            options,
                            text_size,
                            cx,
                        )
                    })
                    .collect()
            }
        })
        .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
        .with_width_from_item(widest_row)
        .track_scroll(&self.handle)
        .h_full();

        // Lock each gesture to its initial axis
        rows.interactivity().base_style.restrict_scroll_to_axis = Some(true);

        div()
            .id((id.clone(), "view"))
            .relative()
            .size_full()
            .text_xs()
            .when_some(self.a11y_label.clone(), |view, label| {
                view.aria_label(label)
            })
            .child(rows)
            .child(
                Scrollbar::vertical((self.id.clone(), "v-scrollbar"), self.handle.clone())
                    .visibility(ScrollbarVisibility::Platform),
            )
            .child(
                Scrollbar::horizontal((self.id.clone(), "h-scrollbar"), self.handle.clone())
                    .visibility(ScrollbarVisibility::Platform),
            )
    }
}

fn render_row(
    row: &DiffRow,
    layout: &RowLayout,
    row_index: usize,
    options: DiffViewOptions,
    text_size: Pixels,
    cx: &App,
) -> gpui::AnyElement {
    let theme = cx.theme();
    let mono_font = theme.mono_font;
    let colors = theme.colors;
    let ramps = theme.ramps;
    let line_height = text_size * CODE_LINE_HEIGHT;
    let char_width = text_size * MONO_ADVANCE_RATIO;

    let frame = div()
        .debug_selector(move || format!("diff-row-{row_index}"))
        .id(layout.id.clone())
        .w_full()
        .h(line_height)
        .font_family(mono_font)
        .text_xs()
        .overflow_hidden();

    if row.kind == DiffRowKind::Hunk {
        let content = match options.separator {
            HunkSeparator::LineInfo => {
                SharedString::from(format!("{} unchanged lines", row.hidden_lines))
            }
            HunkSeparator::Metadata => row.header.clone(),
            HunkSeparator::Simple => SharedString::from("···"),
        };

        return frame
            .flex()
            .items_center()
            .text_color(colors.muted_foreground)
            .when(
                options.indicators == DiffIndicators::Classic,
                |row_element| row_element.child(div().w(CODE_GUTTER_WIDTH).flex_none()),
            )
            .when(options.line_numbers, |row_element| {
                row_element.child(div().w(CODE_GUTTER_WIDTH).flex_none())
            })
            .when(
                options.indicators == DiffIndicators::Bars && !options.line_numbers,
                |row_element| {
                    row_element.child(
                        div()
                            .flex_none()
                            .border_l(CHANGE_BAR_WIDTH)
                            .border_color(gpui::transparent_black()),
                    )
                },
            )
            .child(div().flex_none().ml_2().child(content))
            .into_any_element();
    }

    let change_color = match row.kind {
        DiffRowKind::Added => ramps.success.base,
        DiffRowKind::Removed => ramps.destructive.base,
        DiffRowKind::Context | DiffRowKind::Hunk => gpui::transparent_black(),
    };

    let wash = match row.kind {
        DiffRowKind::Added => Some(wash(ramps.success.base)),
        DiffRowKind::Removed => Some(wash(ramps.destructive.base)),
        DiffRowKind::Context | DiffRowKind::Hunk => None,
    };

    let mut text = String::new();
    let mut spans: Vec<(Range<usize>, Role)> = Vec::new();

    for span in &row.content.spans {
        let start = text.len();
        text.push_str(&span.text);
        if span.role != Role::Text {
            spans.push((start..text.len(), span.role));
        }
    }

    let mut boundaries: Vec<usize> = spans
        .iter()
        .flat_map(|(range, _)| [range.start, range.end])
        .chain(
            row.changed_ranges
                .iter()
                .flat_map(|range| [range.start, range.end]),
        )
        .collect();

    boundaries.sort_unstable();
    boundaries.dedup();

    let mut highlights: Vec<(std::ops::Range<usize>, HighlightStyle)> = Vec::new();
    let mut span_ix = 0;

    for window in boundaries.windows(2) {
        let (start, end) = (window[0], window[1]);
        while span_ix < spans.len() && spans[span_ix].0.end <= start {
            span_ix += 1;
        }

        let mut segment = HighlightStyle::default();
        if let Some((range, role)) = spans.get(span_ix)
            && range.contains(&start)
            && *role != Role::Text
        {
            segment = segment.highlight(HighlightStyle {
                color: Some(role.color(colors, ramps)),
                ..Default::default()
            });
        }

        if row
            .changed_ranges
            .iter()
            .any(|range| range.contains(&start))
        {
            segment = segment.highlight(HighlightStyle {
                background_color: Some(wash_strong(change_color)),
                ..Default::default()
            });
        }

        highlights.push((start..end, segment));
    }

    let style = TextStyle {
        color: colors.foreground,
        font_family: SharedString::from(mono_font),
        font_size: text_size.into(),
        line_height: line_height.into(),
        ..Default::default()
    };

    frame
        .flex()
        .items_center()
        .when_some(wash, |row_element, tint| row_element.bg(tint))
        .when(
            options.indicators == DiffIndicators::Classic,
            |row_element| {
                row_element.child(gutter_cell(
                    cx,
                    if row.kind == DiffRowKind::Context {
                        gpui::transparent_black()
                    } else {
                        change_color
                    },
                    match row.kind {
                        DiffRowKind::Added => "+",
                        DiffRowKind::Removed => "-",
                        _ => "",
                    },
                ))
            },
        )
        .when(options.line_numbers, |row_element| {
            row_element.child(
                gutter_cell(
                    cx,
                    colors.muted_foreground,
                    row.new_number
                        .or(row.old_number)
                        .map(|number| number.to_string())
                        .unwrap_or_default(),
                )
                .w(CODE_GUTTER_WIDTH)
                .whitespace_nowrap()
                .bg(match row.kind {
                    DiffRowKind::Context => wash_gutter(&colors),
                    _ => wash_strong(change_color),
                })
                .when(options.indicators == DiffIndicators::Bars, |number| {
                    number.border_l(CHANGE_BAR_WIDTH).border_color(change_color)
                }),
            )
        })
        .when(
            options.indicators == DiffIndicators::Bars && !options.line_numbers,
            |row_element| {
                row_element.child(
                    div()
                        .h_full()
                        .flex_none()
                        .border_l(CHANGE_BAR_WIDTH)
                        .border_color(change_color),
                )
            },
        )
        .child(
            div()
                .flex_none()
                .ml_2()
                .w(char_width * layout.char_count as f32)
                .whitespace_nowrap()
                .child(gpui::StyledText::new(text).with_default_highlights(&style, highlights)),
        )
        .into_any_element()
}

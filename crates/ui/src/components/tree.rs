//! Renders a single-selection tree of [`TreeItem`] rows with disclosure chevrons.
//!
//! # Examples
//!
//! ```no_run
//! use ui::tree::{Tree, TreeItem};
//!
//! let project = Tree::new("project")
//!     .items(|_, _| {
//!         vec![
//!             TreeItem::new("src", "src").expandable(true).expanded(true),
//!             TreeItem::new("main", "main.rs").depth(1),
//!         ]
//!     })
//!     .on_change(|_selected, _window, _cx| println!("selection changed"));
//! ```

use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, App, ElementId, FocusHandle, InteractiveElement as _, IntoElement, KeyDownEvent,
    ParentElement as _, RenderOnce, Role, SharedString, StatefulInteractiveElement as _,
    StyleRefinement, Styled as _, Window, div, rems,
};

use crate::components::common::{
    StyledExt as _, nav_key, paint_selectable_row, selectable_label_color, v_flex,
};
use crate::icons::{Icon, IconRole, icon};
use crate::theme::ActiveTheme as _;

/// The change reporter, fired with the next selection by pointer and keys.
type ChangeHandler = Rc<dyn Fn(Option<usize>, &mut Window, &mut App)>;
/// The disclosure reporter, fired with the toggled row's index.
type DisclosureHandler = Rc<dyn Fn(usize, &mut Window, &mut App)>;
/// The visible row builder, run during render.
type ItemsFn = Box<dyn Fn(&mut Window, &mut App) -> Vec<TreeItem>>;

/// The deepest indent the tree paints, bounding misbuilt item trees.
const TREE_MAX_DEPTH: usize = 16;

/// One row of a [`Tree`].
#[derive(Clone)]
#[must_use]
pub struct TreeItem {
    key: ElementId,
    label: SharedString,
    icon: Option<Icon>,
    depth: usize,
    expandable: bool,
    expanded: bool,
    disabled: bool,
}

impl TreeItem {
    /// Creates a leaf row shown as `label` at the root depth, identified by `key`.
    pub fn new<K, L>(key: K, label: L) -> Self
    where
        K: Into<ElementId>,
        L: Into<SharedString>,
    {
        Self {
            key: key.into(),
            label: label.into(),
            icon: None,
            depth: 0,
            expandable: false,
            expanded: false,
            disabled: false,
        }
    }

    /// Shows a custom icon leading the label.
    pub fn icon<I>(mut self, icon: I) -> Self
    where
        I: Into<Icon>,
    {
        self.icon = Some(icon.into());
        self
    }

    /// Nests the row one level per count.
    pub fn depth(mut self, depth: usize) -> Self {
        self.depth = depth;
        self
    }

    /// Marks the row as having children.
    pub fn expandable(mut self, expandable: bool) -> Self {
        self.expandable = expandable;
        self
    }

    /// Sets the row's disclosure state.
    pub fn expanded(mut self, expanded: bool) -> Self {
        self.expanded = expanded;
        self
    }

    /// Renders the row dimmed; clicks and keyboard skip it.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

// TreeItem is a data builder, not an element, adopt the combinators explicitly.
impl FluentBuilder for TreeItem {}

/// A tree of [`TreeItem`] rows with single selection and disclosure handling.
#[derive(IntoElement)]
#[must_use]
pub struct Tree {
    id: ElementId,
    selected: Option<usize>,
    a11y_label: Option<SharedString>,
    on_change: Option<ChangeHandler>,
    on_disclosure: Option<DisclosureHandler>,
    items: Option<ItemsFn>,
    style: StyleRefinement,
}

impl Tree {
    /// Creates a tree with nothing selected.
    pub fn new<I>(id: I) -> Self
    where
        I: Into<ElementId>,
    {
        Self {
            id: id.into(),
            selected: None,
            a11y_label: None,
            on_change: None,
            on_disclosure: None,
            items: None,
            style: StyleRefinement::default(),
        }
    }

    /// Sets the current selection.
    pub fn selected(mut self, selected: Option<usize>) -> Self {
        self.selected = selected;
        self
    }

    /// Reports the next selection, from clicks and keys alike.
    pub fn on_change<F>(mut self, handler: F) -> Self
    where
        F: Fn(Option<usize>, &mut Window, &mut App) + 'static,
    {
        self.on_change = Some(Rc::new(handler));
        self
    }

    /// Reports the row whose disclosure the user toggled.
    pub fn on_disclosure<F>(mut self, handler: F) -> Self
    where
        F: Fn(usize, &mut Window, &mut App) + 'static,
    {
        self.on_disclosure = Some(Rc::new(handler));
        self
    }

    /// Sets the visible rows; the closure runs during render and must not mutate entities or notify.
    pub fn items<F>(mut self, items: F) -> Self
    where
        F: Fn(&mut Window, &mut App) -> Vec<TreeItem> + 'static,
    {
        self.items = Some(Box::new(items));
        self
    }

    /// Overrides the accessibility label, which defaults to the id.
    pub fn accessibility_label<L>(mut self, label: L) -> Self
    where
        L: Into<SharedString>,
    {
        self.a11y_label = Some(label.into());
        self
    }

    /// Maps each visible row to its 1-based position-in-set and set size among its siblings.
    fn sibling_sets(items: &[TreeItem]) -> Vec<(usize, usize)> {
        let mut sets = vec![(0usize, 0usize); items.len()];
        // Open sibling runs along the ancestry path, deeper runs above shallower ones.
        let mut runs: Vec<(usize, Vec<usize>)> = Vec::new();

        for (index, item) in items.iter().enumerate() {
            while runs
                .last()
                .is_some_and(|(run_depth, _)| *run_depth > item.depth)
            {
                if let Some((_, pending)) = runs.pop() {
                    Self::stamp_sibling_run(&mut sets, pending);
                }
            }

            match runs.last_mut() {
                // A row at the running depth extends the open sibling run.
                Some((run_depth, pending)) if *run_depth == item.depth => pending.push(index),
                _ => runs.push((item.depth, vec![index])),
            }
        }

        for (_, pending) in runs {
            Self::stamp_sibling_run(&mut sets, pending);
        }

        sets
    }

    fn stamp_sibling_run(sets: &mut [(usize, usize)], pending: Vec<usize>) {
        let size_of_set = pending.len();
        for (position, index) in pending.into_iter().enumerate() {
            if let Some(set) = sets.get_mut(index) {
                *set = (position + 1, size_of_set);
            }
        }
    }

    /// The nearest row above `index` with a smaller depth.
    fn parent_of(items: &[TreeItem], index: usize) -> Option<usize> {
        let depth = items.get(index)?.depth;
        items[..index]
            .iter()
            .rposition(|candidate| candidate.depth < depth)
    }

    /// The first selectable row deeper than `index`, when its branch is open.
    fn first_descendant(items: &[TreeItem], selectable: &[usize], index: usize) -> Option<usize> {
        let depth = items.get(index)?.depth;
        selectable.iter().copied().find(|candidate| {
            *candidate > index && items.get(*candidate).is_some_and(|row| row.depth > depth)
        })
    }
}

impl gpui::Styled for Tree {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for Tree {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let items = match self.items {
            Some(build) => build(window, cx),
            None => Vec::new(),
        };

        let selectable: Vec<usize> = items
            .iter()
            .enumerate()
            .filter(|(_, item)| !item.disabled)
            .map(|(index, _)| index)
            .collect();

        let focus = crate::components::common::focus_handle(&self.id, window, cx);

        let label = self
            .a11y_label
            .clone()
            .unwrap_or_else(|| SharedString::from(self.id.to_string()));

        let sibling_sets = Self::sibling_sets(&items);

        let rows: Vec<AnyElement> = items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                let (position_in_set, size_of_set) = sibling_sets[index];
                item.render_row(
                    cx,
                    index,
                    position_in_set,
                    size_of_set,
                    self.selected == Some(index),
                    &self.on_change,
                    &self.on_disclosure,
                    &focus,
                )
            })
            .collect();

        v_flex()
            .id(self.id)
            .gap_0p5()
            .text_sm()
            .refine_style(&self.style)
            .track_focus(&focus)
            .focusable()
            .tab_stop(true)
            .role(Role::Tree)
            .aria_label(label)
            .on_key_down({
                let selected = self.selected;
                let on_change = self.on_change.clone();
                let on_disclosure = self.on_disclosure.clone();

                move |event: &KeyDownEvent, window, cx| {
                    if nav_key(&selectable, selected, &event.keystroke.key, |next| {
                        if let Some(on_change) = &on_change {
                            on_change(next, window, cx);
                        }
                    }) {
                        cx.stop_propagation();
                        return;
                    }
                    match event.keystroke.key.as_str() {
                        "left" | "right" => {
                            let expanding = event.keystroke.key == "right";
                            let mut disclose = None;
                            let mut target = None;

                            if let Some(index) = selected
                                && let Some(item) = items.get(index)
                                && !item.disabled
                            {
                                if item.expandable && item.expanded != expanding {
                                    disclose = Some(index);
                                } else if expanding {
                                    target = Self::first_descendant(&items, &selectable, index);
                                } else {
                                    target = Self::parent_of(&items, index)
                                        .filter(|parent| selectable.contains(parent));
                                }
                            }

                            if let Some(index) = disclose
                                && let Some(on_disclosure) = &on_disclosure
                            {
                                on_disclosure(index, window, cx);
                            } else if let Some(next) = target
                                && let Some(on_change) = &on_change
                            {
                                on_change(Some(next), window, cx);
                            }
                        }
                        _ => return,
                    }
                    cx.stop_propagation();
                }
            })
            .children(rows)
    }
}

impl TreeItem {
    #[allow(clippy::too_many_arguments)]
    fn render_row(
        &self,
        cx: &App,
        index: usize,
        position_in_set: usize,
        size_of_set: usize,
        selected: bool,
        on_change: &Option<ChangeHandler>,
        on_disclosure: &Option<DisclosureHandler>,
        focus: &FocusHandle,
    ) -> AnyElement {
        let theme = cx.theme();
        let label_color = selectable_label_color(cx, selected, self.disabled);
        let disabled = self.disabled;

        let depth = self.depth.min(TREE_MAX_DEPTH) as f32;
        let indent = rems(0.75 * if self.depth == 0 { 0.5 } else { depth + 1.0 });

        div()
            .id((self.key.clone(), "tree-item"))
            .relative()
            .flex()
            .items_center()
            .gap_2()
            .pl(indent)
            .pr_2()
            .py_1()
            .min_h_8()
            .rounded(theme.radius.sm)
            .role(Role::TreeItem)
            .aria_selected(selected)
            .aria_level(self.depth + 1)
            .aria_position_in_set(position_in_set)
            .aria_size_of_set(size_of_set)
            .when(self.expandable, |row| row.aria_expanded(self.expanded))
            .map(|row| paint_selectable_row(cx, row, selected, disabled))
            .when(!disabled, |row| {
                row.cursor_pointer().on_click({
                    let on_change = on_change.clone();
                    let on_disclosure = on_disclosure.clone();
                    let expandable = self.expandable;
                    let focus = focus.clone();

                    move |_, window, cx| {
                        window.focus(&focus, cx);
                        if let Some(on_change) = &on_change {
                            on_change(Some(index), window, cx);
                        }
                        // A folder row toggles with its own click, not just its chevron.
                        if expandable && let Some(on_disclosure) = &on_disclosure {
                            on_disclosure(index, window, cx);
                        }
                    }
                })
            })
            .children((0..self.depth.min(TREE_MAX_DEPTH)).map(|level| {
                let step = level as f32;
                div()
                    .absolute()
                    .left(rems(0.75 * (step + 0.5)))
                    .top_0()
                    .bottom_0()
                    .border_l_1()
                    .border_color(theme.colors.border)
            }))
            .child(self.render_disclosure(cx, selected))
            .when_some(
                self.icon
                    .clone()
                    .or_else(|| (!self.expandable).then(|| icon(IconRole::File))),
                |row, icon| row.child(icon.size(rems(1.0)).color(label_color)),
            )
            .child(
                div()
                    .flex_1()
                    .text_color(label_color)
                    .child(self.label.clone()),
            )
            .into_any_element()
    }

    fn render_disclosure(&self, cx: &App, selected: bool) -> AnyElement {
        let theme = cx.theme();
        let glyph_color = if self.disabled || !selected {
            theme.colors.muted_foreground
        } else {
            theme.colors.primary_foreground
        };

        // Only expandable rows draw a chevron; the empty cell keeps labels aligned with folders.
        div()
            .flex_none()
            .size_4()
            .flex()
            .items_center()
            .justify_center()
            .when(self.expandable, |cell| {
                cell.child(
                    icon(if self.expanded {
                        IconRole::ChevronDown
                    } else {
                        IconRole::ChevronRight
                    })
                    .size(rems(0.75))
                    .color(glyph_color),
                )
            })
            .into_any_element()
    }
}

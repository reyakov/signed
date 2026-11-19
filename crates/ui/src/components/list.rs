//! Renders a single-selection list of [`ListItem`] rows with keyboard navigation.
//!
//! # Examples
//!
//! ```no_run
//! use ui::list::{List, ListItem};
//!
//! let folders = List::new("folders")
//!     .selected(Some(0))
//!     .items(|_, _| {
//!         vec![
//!             ListItem::new("documents", "Documents").detail("24 files"),
//!             ListItem::new("downloads", "Downloads"),
//!         ]
//!     })
//!     .on_change(|_selected, _window, _cx| println!("selection changed"));
//! ```

use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, App, ElementId, FocusHandle, InteractiveElement as _, IntoElement, KeyDownEvent,
    ParentElement as _, RenderOnce, Role, SharedString, StatefulInteractiveElement as _,
    StyleRefinement, Styled as _, Window, div,
};

use crate::components::common::{
    StyledExt as _, nav_key, paint_selectable_row, selectable_label_color,
};
use crate::icons::Icon;
use crate::theme::{ActiveTheme as _, TextRole, Typeset as _};

/// The change reporter, fired with the next selection by pointer and keys.
type ChangeHandler = Rc<dyn Fn(Option<usize>, &mut Window, &mut App)>;
/// The row builder, run during render.
type ItemsFn = Box<dyn Fn(&mut Window, &mut App) -> Vec<ListItem>>;

/// One row of a [`List`].
#[must_use]
pub struct ListItem {
    key: ElementId,
    label: SharedString,
    icon: Option<Icon>,
    detail: Option<SharedString>,
    disabled: bool,
}

impl ListItem {
    /// Creates a row shown as `label`, identified by `key`.
    pub fn new<K, L>(key: K, label: L) -> Self
    where
        K: Into<ElementId>,
        L: Into<SharedString>,
    {
        Self {
            key: key.into(),
            label: label.into(),
            icon: None,
            detail: None,
            disabled: false,
        }
    }

    /// Shows an icon leading the label, a role or any custom [`crate::icons::IconNamed`].
    pub fn icon<I>(mut self, icon: I) -> Self
    where
        I: Into<Icon>,
    {
        self.icon = Some(icon.into());
        self
    }

    /// Shows muted trailing text.
    pub fn detail<D>(mut self, detail: D) -> Self
    where
        D: Into<SharedString>,
    {
        self.detail = Some(detail.into());
        self
    }

    /// Renders the row dimmed; clicks and keyboard skip it.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

// ListItem is a data builder, not an element, adopt the combinators explicitly.
impl FluentBuilder for ListItem {}

/// A single-selection list of [`ListItem`] rows.
#[derive(IntoElement)]
#[must_use]
pub struct List {
    id: ElementId,
    selected: Option<usize>,
    a11y_label: Option<SharedString>,
    on_change: Option<ChangeHandler>,
    items: Option<ItemsFn>,
    style: StyleRefinement,
}

impl List {
    /// Creates a list with nothing selected.
    pub fn new<I>(id: I) -> Self
    where
        I: Into<ElementId>,
    {
        Self {
            id: id.into(),
            selected: None,
            a11y_label: None,
            on_change: None,
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

    /// Sets the rows; the closure runs during render and must not mutate entities or notify.
    pub fn items<F>(mut self, items: F) -> Self
    where
        F: Fn(&mut Window, &mut App) -> Vec<ListItem> + 'static,
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
}

impl gpui::Styled for List {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for List {
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
        let theme = cx.theme();

        let label = self
            .a11y_label
            .clone()
            .unwrap_or_else(|| SharedString::from(self.id.to_string()));

        let rows: Vec<AnyElement> = items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                item.render_row(
                    cx,
                    index,
                    items.len(),
                    self.selected == Some(index),
                    &self.on_change,
                    &focus,
                )
            })
            .collect();

        div()
            .id(self.id)
            .flex()
            .flex_col()
            .gap_0p5()
            .text_size(theme.sizes.medium.text_size)
            .refine_style(&self.style)
            .track_focus(&focus)
            .focusable()
            .tab_stop(true)
            .role(Role::List)
            .aria_label(label)
            .on_key_down({
                let selected = self.selected;
                let on_change = self.on_change.clone();

                move |event: &KeyDownEvent, window, cx| {
                    if nav_key(&selectable, selected, &event.keystroke.key, |next| {
                        if let Some(on_change) = &on_change {
                            on_change(next, window, cx);
                        }
                    }) {
                        cx.stop_propagation();
                    }
                }
            })
            .children(rows)
    }
}

impl ListItem {
    fn render_row(
        &self,
        cx: &App,
        index: usize,
        count: usize,
        selected: bool,
        on_change: &Option<ChangeHandler>,
        focus: &FocusHandle,
    ) -> AnyElement {
        let theme = cx.theme();
        let label_color = selectable_label_color(cx, selected, self.disabled);
        let disabled = self.disabled;

        div()
            .id((self.key.clone(), "list-item"))
            .flex()
            .items_center()
            .gap_2()
            .px_2()
            .py_1()
            .min_h(theme.sizes.medium.control_height)
            .rounded(theme.radius.sm)
            .role(Role::ListItem)
            .aria_selected(selected)
            .aria_position_in_set(index + 1)
            .aria_size_of_set(count)
            .map(|row| paint_selectable_row(cx, row, selected, disabled))
            .when(!disabled, |row| {
                row.cursor_pointer().on_click({
                    let on_change = on_change.clone();
                    let focus = focus.clone();
                    move |_, window, cx| {
                        window.focus(&focus, cx);
                        if let Some(on_change) = &on_change {
                            on_change(Some(index), window, cx);
                        }
                    }
                })
            })
            .when_some(self.icon.clone(), |row, icon| {
                row.child(icon.size(theme.sizes.medium.icon_size).color(label_color))
            })
            .child(
                div()
                    .flex_1()
                    .text_color(label_color)
                    .child(self.label.clone()),
            )
            .when_some(self.detail.clone(), |row, detail| {
                row.child(
                    div()
                        .flex_none()
                        .typeset(TextRole::Subheadline, cx)
                        .text_color(if selected {
                            theme.colors.primary_foreground
                        } else {
                            theme.colors.muted_foreground
                        })
                        .child(detail),
                )
            })
            .into_any_element()
    }
}

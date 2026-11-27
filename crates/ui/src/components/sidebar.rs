//! Renders rows grouped under section headers, with slots for custom content between the rows.
//!
//! # Examples
//!
//! ```no_run
//! use ui::sidebar::{Sidebar, SidebarEntry};
//!
//! let places = Sidebar::new("places")
//!     .entries(|_, _| {
//!         vec![
//!             SidebarEntry::header("Favorites"),
//!             SidebarEntry::row("documents", "Documents").detail("24 files"),
//!         ]
//!     })
//!     .on_change(|_selected, _window, _cx| println!("selection changed"));
//! ```

use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, App, ElementId, FocusHandle, InteractiveElement as _, IntoElement, KeyDownEvent,
    ParentElement as _, Pixels, RenderOnce, Role, SharedString, StatefulInteractiveElement as _,
    StyleRefinement, Styled as _, Window, div,
};

use crate::components::common::{ElementIdExt as _, SelectableRows as _, StyledExt as _, v_flex};
use crate::components::grain::grain as grain_overlay;
use crate::theme::{ActiveTheme as _, BORDER_WIDTH, SIDEBAR_DEFAULT_WIDTH};

/// The change reporter, fired with the next selected row by pointer and keys.
type ChangeHandler = Rc<dyn Fn(Option<usize>, &mut Window, &mut App)>;
/// The entries builder, run during render.
type EntriesFn = Box<dyn Fn(&mut Window, &mut App) -> Vec<SidebarEntry>>;

/// One entry of a [`Sidebar`], a section header, a selectable row, or custom content.
#[must_use]
pub enum SidebarEntry {
    /// A non-selectable section label.
    Header {
        label: SharedString,
        prefix: Option<AnyElement>,
        suffix: Option<AnyElement>,
    },
    /// A selectable row.
    Row {
        key: ElementId,
        label: SharedString,
        detail: Option<SharedString>,
        prefix: Option<AnyElement>,
        suffix: Option<AnyElement>,
        disabled: bool,
    },
    /// Custom content occupying a slot in the list.
    Custom(AnyElement),
}

impl SidebarEntry {
    /// Creates a section header.
    pub fn header<L>(label: L) -> Self
    where
        L: Into<SharedString>,
    {
        Self::Header {
            label: label.into(),
            prefix: None,
            suffix: None,
        }
    }

    /// Creates a selectable row shown as `label`, identified by `key`.
    pub fn row<K, L>(key: K, label: L) -> Self
    where
        K: Into<ElementId>,
        L: Into<SharedString>,
    {
        Self::Row {
            key: key.into(),
            label: label.into(),
            detail: None,
            prefix: None,
            suffix: None,
            disabled: false,
        }
    }

    /// Wraps custom content occupying a slot in the list.
    pub fn custom<E>(element: E) -> Self
    where
        E: IntoElement,
    {
        Self::Custom(element.into_any_element())
    }

    /// Shows muted trailing text.
    pub fn detail<D>(mut self, detail: D) -> Self
    where
        D: Into<SharedString>,
    {
        if let Self::Row {
            detail: row_detail, ..
        } = &mut self
        {
            *row_detail = Some(detail.into());
        }
        self
    }

    /// Shows an element leading the entry.
    pub fn prefix<E>(mut self, prefix: E) -> Self
    where
        E: IntoElement,
    {
        if let Self::Row { prefix: slot, .. } | Self::Header { prefix: slot, .. } = &mut self {
            *slot = Some(prefix.into_any_element());
        }
        self
    }

    /// Shows an element trailing the entry.
    pub fn suffix<E>(mut self, suffix: E) -> Self
    where
        E: IntoElement,
    {
        if let Self::Row { suffix: slot, .. } | Self::Header { suffix: slot, .. } = &mut self {
            *slot = Some(suffix.into_any_element());
        }
        self
    }

    /// Renders the row dimmed; clicks and keyboard skip it.
    pub fn disabled(mut self, disabled: bool) -> Self {
        if let Self::Row {
            disabled: row_disabled,
            ..
        } = &mut self
        {
            *row_disabled = disabled;
        }
        self
    }

    #[allow(clippy::too_many_arguments)]
    fn render_entry(
        self,
        cx: &App,
        index: usize,
        position_in_set: usize,
        size_of_set: usize,
        selected: bool,
        material: bool,
        on_change: &Option<ChangeHandler>,
        focus: &FocusHandle,
    ) -> AnyElement {
        let theme = cx.theme();

        match self {
            SidebarEntry::Header {
                label,
                prefix,
                suffix,
            } => {
                // The id is required for the role to surface in the a11y tree.
                div()
                    .id(("sidebar-header".into(), label.clone()))
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .pt_2()
                    .pb_1()
                    .text_sm()
                    .font_semibold()
                    .text_color(theme.colors.muted_foreground)
                    .role(Role::Heading)
                    .when_some(prefix, |header, prefix| header.child(prefix))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(label),
                    )
                    .when_some(suffix, |header, suffix| header.child(suffix))
                    .into_any_element()
            }
            SidebarEntry::Custom(element) => element,
            SidebarEntry::Row {
                key,
                label,
                detail,
                prefix,
                suffix,
                disabled: row_disabled,
            } => {
                let label_color = theme.selectable_label_color(selected, row_disabled);

                div()
                    .id((key.clone(), "sidebar-row"))
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .min_h_8()
                    .rounded(theme.radius.sm)
                    .text_color(label_color)
                    .role(Role::ListItem)
                    .aria_selected(selected)
                    .aria_position_in_set(position_in_set)
                    .aria_size_of_set(size_of_set)
                    .map(|row| {
                        if material && !row_disabled {
                            if selected {
                                row.bg(theme.material.selection)
                            } else {
                                row.hover(|row| row.bg(theme.material.hover))
                            }
                        } else {
                            row.selectable_row(cx, selected, row_disabled)
                        }
                    })
                    .when(!row_disabled, |row| {
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
                    .when_some(prefix, |row, prefix| row.child(prefix))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(label),
                    )
                    .when_some(detail, |row, detail| {
                        row.child(
                            div()
                                .flex_none()
                                .text_sm()
                                .text_color(if selected {
                                    theme.colors.primary_foreground
                                } else {
                                    theme.colors.muted_foreground
                                })
                                .child(detail),
                        )
                    })
                    .when_some(suffix, |row, suffix| row.child(suffix))
                    .into_any_element()
            }
        }
    }
}

// SidebarEntry is a data builder, not an element, adopt the combinators explicitly.
impl FluentBuilder for SidebarEntry {}

/// A sidebar of [`SidebarEntry`] rows grouped under section headers.
#[derive(IntoElement)]
#[must_use]
pub struct Sidebar {
    id: ElementId,
    selected: Option<usize>,
    transparent: bool,
    grain: bool,
    border_right: bool,
    padded: bool,
    width: Pixels,
    a11y_label: Option<SharedString>,
    on_change: Option<ChangeHandler>,
    entries: Option<EntriesFn>,
    style: StyleRefinement,
}

impl Sidebar {
    /// Creates a sidebar with nothing selected.
    pub fn new<I>(id: I) -> Self
    where
        I: Into<ElementId>,
    {
        Self {
            id: id.into(),
            selected: None,
            transparent: false,
            grain: false,
            border_right: true,
            padded: true,
            width: SIDEBAR_DEFAULT_WIDTH,
            a11y_label: None,
            on_change: None,
            entries: None,
            style: StyleRefinement::default(),
        }
    }

    /// Sets the current selection.
    pub fn selected(mut self, selected: Option<usize>) -> Self {
        self.selected = selected;
        self
    }

    /// Paints translucent material fills so the window's native backdrop shows through.
    pub fn transparent(mut self, transparent: bool) -> Self {
        self.transparent = transparent;
        self
    }

    /// Paints a hairline border on the sidebar's right edge.
    pub fn border_right(mut self, border_right: bool) -> Self {
        self.border_right = border_right;
        self
    }

    /// Reports the next selected row, from clicks and keys alike.
    pub fn on_change<F>(mut self, handler: F) -> Self
    where
        F: Fn(Option<usize>, &mut Window, &mut App) + 'static,
    {
        self.on_change = Some(Rc::new(handler));
        self
    }

    /// Sets the entries; the closure runs during render and must not mutate entities or notify.
    pub fn entries<F>(mut self, entries: F) -> Self
    where
        F: Fn(&mut Window, &mut App) -> Vec<SidebarEntry> + 'static,
    {
        self.entries = Some(Box::new(entries));
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

impl gpui::Styled for Sidebar {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for Sidebar {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let Self {
            id,
            selected,
            transparent: material,
            grain,
            border_right,
            padded,
            width,
            a11y_label,
            on_change,
            entries,
            style,
        } = self;

        let label = a11y_label.unwrap_or_else(|| SharedString::from(id.to_string()));
        let debug_id = id.clone();

        let entries = match entries {
            Some(build) => build(window, cx),
            None => Vec::new(),
        };

        // Headers and custom entries are never selectable, only enabled rows are.
        let selectable: Vec<usize> = entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| match entry {
                SidebarEntry::Row { disabled, .. } => !disabled,
                SidebarEntry::Header { .. } | SidebarEntry::Custom(_) => false,
            })
            .map(|(index, _)| index)
            .collect();

        let focus = id.focus_handle(window, cx);

        let row_count = entries
            .iter()
            .filter(|entry| matches!(entry, SidebarEntry::Row { .. }))
            .count();

        let mut row_position = 0usize;
        let mut list_children = Vec::new();
        let mut custom_children = Vec::new();

        for (index, entry) in entries.into_iter().enumerate() {
            let is_row = matches!(entry, SidebarEntry::Row { .. });
            let is_custom = matches!(entry, SidebarEntry::Custom(_));

            if is_row {
                row_position += 1;
            }

            let (position_in_set, size_of_set) = if is_row {
                (row_position, row_count)
            } else {
                (0, 0)
            };

            let rendered = entry.render_entry(
                cx,
                index,
                position_in_set,
                size_of_set,
                selected == Some(index),
                material,
                &on_change,
                &focus,
            );

            if is_custom {
                custom_children.push(rendered);
            } else {
                list_children.push(rendered);
            }
        }

        let theme = cx.theme();

        v_flex()
            .id(id.clone())
            .refine_style(&style)
            .debug_selector(move || format!("sidebar-{debug_id}"))
            .h_full()
            .w(width)
            .text_sm()
            .when(border_right, |sidebar| {
                sidebar
                    .border_r(BORDER_WIDTH)
                    .border_color(theme.colors.border)
            })
            .child(
                div()
                    .id((id.clone(), "list"))
                    .flex_none()
                    .track_focus(&focus)
                    .focusable()
                    .tab_stop(true)
                    .role(Role::List)
                    .aria_label(label)
                    .on_key_down({
                        let on_change = on_change.clone();
                        move |event: &KeyDownEvent, window, cx| {
                            if selectable.nav_key(selected, &event.keystroke.key, |next| {
                                if let Some(on_change) = &on_change {
                                    on_change(next, window, cx);
                                }
                            }) {
                                cx.stop_propagation();
                            }
                        }
                    })
                    .child(
                        v_flex()
                            .gap_0p5()
                            .when(padded, |content| content.p_2())
                            .children(list_children),
                    ),
            )
            .when(!custom_children.is_empty(), |sidebar| {
                sidebar.child(
                    v_flex()
                        .flex_1()
                        .min_h_0()
                        .when(padded, |content| content.pl_2())
                        .children(custom_children),
                )
            })
            .when(material && grain, |sidebar| sidebar.child(grain_overlay()))
    }
}

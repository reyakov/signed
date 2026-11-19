//! Renders a menu at the pointer when its target element is right-clicked.
//!
//! # Examples
//!
//! ```no_run
//! use gpui::{IntoElement as _, ParentElement as _, SharedString, div};
//! use ui::context_menu::ContextMenu;
//! use ui::menu::MenuItem;
//!
//! let context = ContextMenu::new("project-context")
//!     .target(|_window, _cx| {
//!         div()
//!             .child(SharedString::from("project.el"))
//!             .into_any_element()
//!     })
//!     .items(|_window, _cx| vec![MenuItem::new("Delete").handler(|_window, _cx| {})]);
//! ```

use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, ElementId, InteractiveElement as _, IntoElement, MouseButton, MouseDownEvent,
    ParentElement as _, RenderOnce, SharedString, StyleRefinement, Window, div,
};

use crate::components::common::StyledExt as _;
use crate::components::menu::{Menu, MenuItem};

/// Builds the element that opens the menu on right-click.
type TargetFn = Box<dyn Fn(&mut Window, &mut App) -> gpui::AnyElement>;
/// Builds the menu's items fresh each render.
type ItemsFn = Box<dyn Fn(&mut Window, &mut App) -> Vec<MenuItem>>;

/// A menu that opens at the pointer when its target is right-clicked.
///
/// Open state is owned internally by design, unlike the controlled [`Menu`].
#[derive(IntoElement)]
#[must_use]
pub struct ContextMenu {
    id: ElementId,
    target: Option<TargetFn>,
    items: Option<ItemsFn>,
    a11y_label: Option<SharedString>,
    style: StyleRefinement,
}

impl ContextMenu {
    /// Creates a context menu over a target set with [`ContextMenu::target`].
    pub fn new<I>(id: I) -> Self
    where
        I: Into<ElementId>,
    {
        Self {
            id: id.into(),
            target: None,
            items: None,
            a11y_label: None,
            style: StyleRefinement::default(),
        }
    }

    /// Sets the element that opens the menu on right-click.
    pub fn target<F>(mut self, target: F) -> Self
    where
        F: Fn(&mut Window, &mut App) -> gpui::AnyElement + 'static,
    {
        self.target = Some(Box::new(target));
        self
    }

    /// Sets the menu's items, built fresh each render.
    pub fn items<F>(mut self, items: F) -> Self
    where
        F: Fn(&mut Window, &mut App) -> Vec<MenuItem> + 'static,
    {
        self.items = Some(Box::new(items));
        self
    }

    /// Overrides the menu's accessibility label.
    pub fn accessibility_label<L>(mut self, label: L) -> Self
    where
        L: Into<SharedString>,
    {
        self.a11y_label = Some(label.into());
        self
    }
}

impl RenderOnce for ContextMenu {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let open = window.with_element_namespace(self.id.clone(), |window| {
            window
                .use_keyed_state("context-open", cx, |_, _| false)
                .clone()
        });

        let at = window.with_element_namespace(self.id.clone(), |window| {
            window
                .use_keyed_state("context-position", cx, |_, _| None)
                .clone()
        });

        let is_open = *open.read(cx);
        let position = *at.read(cx);

        let target = self.target;
        let at_for_target = at.clone();

        let trigger = move |window: &mut Window, cx: &mut App| {
            let base = target
                .as_ref()
                .map(|build| build(window, cx))
                .unwrap_or_else(|| div().into_any_element());

            div()
                .on_mouse_down(MouseButton::Right, {
                    let at = at_for_target.clone();
                    move |event: &MouseDownEvent, _window, cx| {
                        at.update(cx, |at, _| *at = Some(event.position));
                    }
                })
                .child(base)
                .into_any_element()
        };

        let items = move |window: &mut Window, cx: &mut App| {
            self.items
                .as_ref()
                .map(|build| build(window, cx))
                .unwrap_or_default()
        };

        Menu::new(self.id)
            .open(is_open)
            .on_change(move |next, _window, cx| open.update(cx, |open, _| *open = next))
            .open_at(position)
            .trigger_button(MouseButton::Right)
            .trigger(trigger)
            .items(items)
            .refine_style(&self.style)
            .when_some(self.a11y_label.clone(), |menu, label| {
                menu.accessibility_label(label)
            })
    }
}

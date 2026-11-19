//! Renders a button that copies a value to the system clipboard and confirms with a
//! brief check mark.
//!
//! # Examples
//!
//! ```no_run
//! use ui::clipboard::ClipboardButton;
//!
//! let copy = ClipboardButton::new("copy-build", "cargo build --release")
//!     .on_copied(|_cx| println!("copied"));
//! ```

use std::rc::Rc;
use std::time::Duration;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyView, App, ClipboardItem, ElementId, Entity, IntoElement, RenderOnce, SharedString,
    StyleRefinement, Task, Window,
};

use crate::components::button::{Button, ButtonVariant};
use crate::components::common::{ResultLogErr as _, Sizable, StyledExt as _};
use crate::icons::IconRole;
use crate::theme::Size;

/// How long the check mark shows before reverting to the copy icon.
pub const COPIED_DURATION: Duration = Duration::from_secs(1);

/// The tooltip view builder gpui's `.tooltip(...)` expects.
type TooltipBuilder = Rc<dyn Fn(&mut Window, &mut App) -> AnyView>;

/// Runs after the value lands on the clipboard.
type CopiedHandler = Box<dyn Fn(&mut App)>;

/// A quiet icon button that copies a value to the system clipboard.
#[derive(IntoElement)]
#[must_use]
pub struct ClipboardButton {
    id: ElementId,
    value: SharedString,
    size: Size,
    disabled: bool,
    a11y_label: Option<SharedString>,
    tooltip: Option<TooltipBuilder>,
    on_copied: Option<CopiedHandler>,
    style: StyleRefinement,
}

impl ClipboardButton {
    /// Creates a copy button for this value.
    pub fn new<I, V>(id: I, value: V) -> Self
    where
        I: Into<ElementId>,
        V: Into<SharedString>,
    {
        Self {
            id: id.into(),
            value: value.into(),
            size: Size::default(),
            disabled: false,
            a11y_label: None,
            tooltip: None,
            on_copied: None,
            style: StyleRefinement::default(),
        }
    }

    /// Shows a tooltip view next to the button after it is hovered.
    pub fn tooltip<F>(mut self, tooltip: F) -> Self
    where
        F: Fn(&mut Window, &mut App) -> AnyView + 'static,
    {
        self.tooltip = Some(Rc::new(tooltip));
        self
    }

    /// Renders the button inert and dimmed.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Overrides the accessibility label, which defaults to `Copy`.
    pub fn accessibility_label<L>(mut self, label: L) -> Self
    where
        L: Into<SharedString>,
    {
        self.a11y_label = Some(label.into());
        self
    }

    /// Runs after the value lands on the clipboard.
    pub fn on_copied<F>(mut self, handler: F) -> Self
    where
        F: Fn(&mut App) + 'static,
    {
        self.on_copied = Some(Box::new(handler));
        self
    }
}

impl Sizable for ClipboardButton {
    fn apply_size(&mut self, size: Size) {
        self.size = size;
    }
}

/// The transient copied marker keyed per button.
struct CopiedState {
    generation: u64,
    tasks: Vec<Task<()>>,
}

impl RenderOnce for ClipboardButton {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let copied: Entity<CopiedState> =
            window.use_keyed_state((self.id.clone(), "clipboard-copied"), cx, |_, _| {
                CopiedState {
                    generation: 0,
                    tasks: Vec::new(),
                }
            });
        let is_copied = copied.read(cx).generation != 0;

        let selector = if is_copied {
            "clipboard-copied"
        } else {
            "clipboard"
        };

        Button::new((self.id.clone(), "clipboard-button"))
            .icon(if is_copied {
                IconRole::Check
            } else {
                IconRole::Copy
            })
            .variant(ButtonVariant::Ghost)
            .with_size(self.size)
            .disabled(self.disabled)
            .refine_style(&self.style)
            .debug_selector(move || selector.to_string())
            .accessibility_label(self.a11y_label.unwrap_or_else(|| "Copy".into()))
            .when_some(self.tooltip, |button, tooltip| {
                button.tooltip(move |window, cx| tooltip(window, cx))
            })
            .on_click({
                let value = self.value;
                let copied = copied.clone();
                let on_copied = self.on_copied;
                move |_, window, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(value.to_string()));

                    let generation = copied.update(cx, |state, cx| {
                        state.generation += 1;
                        cx.notify();
                        state.generation
                    });

                    let weak = copied.downgrade();
                    let task = window.spawn(cx, async move |cx| {
                        cx.background_executor().timer(COPIED_DURATION).await;
                        cx.update(|_, cx| {
                            weak.update(cx, |state, cx| {
                                if state.generation == generation {
                                    state.generation = 0;
                                    cx.notify();
                                }
                            })
                            .log_err();
                        })
                        .log_err();
                    });

                    copied.update(cx, |state, _| {
                        state.tasks.push(task);
                    });

                    // Invoke the callback after copying
                    if let Some(on_copied) = &on_copied {
                        on_copied(cx);
                    }
                }
            })
    }
}

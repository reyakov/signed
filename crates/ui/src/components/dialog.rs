//! Renders a centred panel over a dim scrim: the platform's modal.
//!
//! Give the dialog a [`DialogHandle`] to share its open state: dismissal
//! through escape, the scrim, or the close button updates every holder of a
//! clone, and they can open or close it in turn.
//!
//! # Examples
//!
//! ```no_run
//! use gpui::{IntoElement, ParentElement as _, SharedString, div};
//! use ui::dialog::Dialog;
//!
//! let confirm = Dialog::new("delete-project")
//!     .title("Delete project")
//!     .content(|_window, _cx| {
//!         div()
//!             .child(SharedString::from("This cannot be undone."))
//!             .into_any_element()
//!     })
//!     .open(true)
//!     .on_change(|open, _window, _cx| println!("dialog open: {open}"));
//! ```

use std::cell::Cell;
use std::rc::Rc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    Animation, AnimationExt as _, AnyElement, App, DefiniteLength, ElementId,
    InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton, ParentElement as _,
    RenderOnce, Role, ScrollHandle, SharedString, StatefulInteractiveElement as _, StyleRefinement,
    Styled as _, Window, canvas, deferred, div, ease_out_quint, relative,
};

use crate::components::button::{Button, ButtonVariant};
use crate::components::common::{DIALOG_LAYER, Sizable, StyledExt as _, v_flex};
use crate::components::grain::grain;
use crate::components::scroll_area::ScrollArea;
use crate::icons::IconRole;
use crate::layout::h_flex;
use crate::theme::{ActiveTheme as _, DIALOG_MIN_WIDTH, TextRole, Typeset as _};

/// Requests the dialog's close: `false` closes.
pub type ChangeHandler = Rc<dyn Fn(bool, &mut Window, &mut App)>;
type ContentFn = Box<dyn Fn(&mut Window, &mut App) -> AnyElement>;
type ActionsFn = Box<dyn Fn(&mut Window, &mut App) -> Vec<AnyElement>>;

/// The shared open state of a dialog; clone it to open or close from anywhere.
#[derive(Clone, Default)]
pub struct DialogHandle {
    open: Rc<Cell<bool>>,
}

impl DialogHandle {
    /// Creates a handle with the given open state.
    pub fn new(open: bool) -> Self {
        Self {
            open: Rc::new(Cell::new(open)),
        }
    }

    /// Whether the dialog is open.
    pub fn is_open(&self) -> bool {
        self.open.get()
    }

    /// Opens the dialog.
    pub fn open(&self, cx: &mut App) {
        self.set_open(true, cx);
    }

    /// Closes the dialog.
    pub fn close(&self, cx: &mut App) {
        self.set_open(false, cx);
    }

    fn set_open(&self, open: bool, cx: &mut App) {
        if self.open.replace(open) != open {
            cx.refresh_windows();
        }
    }
}

/// A centred panel over a dim scrim: the platform's modal.
#[derive(IntoElement)]
#[must_use]
pub struct Dialog {
    id: ElementId,
    open: bool,
    alert: bool,
    grain: bool,
    title: Option<SharedString>,
    a11y_label: Option<SharedString>,
    handle: Option<DialogHandle>,
    on_change: Option<ChangeHandler>,
    close_on_escape: bool,
    close_on_backdrop: bool,
    content: Option<ContentFn>,
    actions: Option<ActionsFn>,
    min_width: Option<DefiniteLength>,
    height: Option<DefiniteLength>,
    style: StyleRefinement,
}

impl Dialog {
    /// Creates a closed dialog.
    pub fn new<I>(id: I) -> Self
    where
        I: Into<ElementId>,
    {
        Self {
            id: id.into(),
            open: false,
            alert: false,
            grain: false,
            title: None,
            a11y_label: None,
            handle: None,
            on_change: None,
            close_on_escape: true,
            close_on_backdrop: true,
            content: None,
            actions: None,
            min_width: None,
            height: None,
            style: StyleRefinement::default(),
        }
    }

    /// Sets the initial open state; a handle overrides it.
    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }

    /// Sets the alert mode.
    pub fn alert(mut self, alert: bool) -> Self {
        self.alert = alert;
        self
    }

    /// Textures the scrim with a film-grain overlay.
    pub fn grain(mut self, grain: bool) -> Self {
        self.grain = grain;
        self
    }

    /// Shares the dialog's open state.
    pub fn handle(mut self, handle: DialogHandle) -> Self {
        self.handle = Some(handle);
        self
    }

    /// Sets whether escape dismisses the dialog.
    pub fn close_on_escape(mut self, close: bool) -> Self {
        self.close_on_escape = close;
        self
    }

    /// Sets whether clicking the scrim dismisses the dialog.
    pub fn close_on_backdrop(mut self, close: bool) -> Self {
        self.close_on_backdrop = close;
        self
    }

    /// Requests close through `on_change` on escape, scrim clicks, and the close button.
    pub fn on_change<F>(mut self, handler: F) -> Self
    where
        F: Fn(bool, &mut Window, &mut App) + 'static,
    {
        self.on_change = Some(Rc::new(handler));
        self
    }

    /// Sets the dialog's title.
    pub fn title<L>(mut self, title: L) -> Self
    where
        L: Into<SharedString>,
    {
        self.title = Some(title.into());
        self
    }

    /// Sets the body content between the title and the actions.
    pub fn content<F>(mut self, content: F) -> Self
    where
        F: Fn(&mut Window, &mut App) -> AnyElement + 'static,
    {
        self.content = Some(Box::new(content));
        self
    }

    /// Sets the footer actions, laid out right-aligned in the given order.
    pub fn actions<F>(mut self, actions: F) -> Self
    where
        F: Fn(&mut Window, &mut App) -> Vec<AnyElement> + 'static,
    {
        self.actions = Some(Box::new(actions));
        self
    }

    /// Overrides the accessibility label of the panel.
    pub fn accessibility_label<L>(mut self, label: L) -> Self
    where
        L: Into<SharedString>,
    {
        self.a11y_label = Some(label.into());
        self
    }

    /// Sets the panel's minimum width, defaults to the standard dialog width.
    pub fn min_width<W>(mut self, width: W) -> Self
    where
        W: Into<DefiniteLength>,
    {
        self.min_width = Some(width.into());
        self
    }

    /// Sets the panel's height, body content taller than the panel scrolls.
    pub fn height<H>(mut self, height: H) -> Self
    where
        H: Into<DefiniteLength>,
    {
        self.height = Some(height.into());
        self
    }
}

impl gpui::Styled for Dialog {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for Dialog {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let root = div().absolute().inset_0();

        let open = self
            .handle
            .as_ref()
            .map_or(self.open, DialogHandle::is_open);

        if !open {
            return root.into_any_element();
        }

        // A handle routes dismissal through the shared state
        let on_change = self.on_change.clone().or_else(|| {
            self.handle.clone().map(|handle| {
                Rc::new(move |open: bool, _window: &mut Window, cx: &mut App| {
                    handle.set_open(open, cx);
                }) as ChangeHandler
            })
        });

        let (scrim_color, border, foreground, surface, radius, shadows, enter_duration) = {
            let theme = cx.theme();
            (
                theme.colors.overlay,
                theme.colors.border,
                theme.colors.foreground,
                theme.colors.surface,
                theme.radius.overlay,
                theme.elevations.dialog.shadows(),
                theme.motion.overlay,
            )
        };

        let role = if self.alert {
            Role::AlertDialog
        } else {
            Role::Dialog
        };

        let label = self
            .a11y_label
            .clone()
            .or_else(|| self.title.clone())
            .unwrap_or_else(|| SharedString::from(self.id.to_string()));

        let alert = self.alert;
        let close_on_escape = self.close_on_escape;
        let close_on_backdrop = self.close_on_backdrop;
        let show_close = !alert && on_change.is_some() && (close_on_escape || close_on_backdrop);

        // Keyboard focus rides the panel across frames.
        let focus = crate::components::common::focus_handle(&self.id, window, cx);

        let content = self.content.map(|build| build(window, cx));
        let actions = self.actions.map(|build| build(window, cx));
        let height = self.height;

        let scroll = window
            .use_keyed_state("dialog-content-scroll", cx, |_, _| ScrollHandle::new())
            .read(cx)
            .clone();

        let panel = v_flex()
            .id("dialog-panel")
            .track_focus(&focus)
            .role(role)
            .aria_label(label)
            .when_some(self.min_width, |panel, width| panel.min_w(width))
            .when(self.min_width.is_none(), |panel| {
                panel.min_w(DIALOG_MIN_WIDTH)
            })
            .max_w(relative(0.9))
            .when_some(height, |panel, height| panel.h(height))
            .bg(surface)
            .text_color(foreground)
            .rounded(radius)
            .border_1()
            .border_color(border)
            .shadow(shadows)
            .overflow_hidden()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_key_down({
                let on_change = on_change.clone();
                move |event: &KeyDownEvent, window, cx| {
                    if event.keystroke.key == "escape"
                        && !alert
                        && close_on_escape
                        && let Some(on_change) = &on_change
                    {
                        on_change(false, window, cx);
                    }
                    cx.stop_propagation();
                }
            })
            .when(self.title.is_some() || show_close, |panel| {
                panel.child(
                    h_flex()
                        .h_12()
                        .px_2()
                        .gap_2()
                        .justify_between()
                        .child(div().size_5())
                        .when_some(self.title.clone(), |row, title| {
                            row.child(
                                div()
                                    .flex_1()
                                    .text_center()
                                    .typeset(TextRole::Headline, cx)
                                    .child(title),
                            )
                        })
                        .when(show_close, |row| {
                            row.child(
                                Button::new("dialog-close")
                                    .icon(IconRole::Close)
                                    .variant(ButtonVariant::Ghost)
                                    .small()
                                    .debug_selector(|| "dialog-close".to_string())
                                    .accessibility_label("Close")
                                    .on_click({
                                        let on_change = on_change.clone();
                                        move |_, window, cx| {
                                            if let Some(on_change) = &on_change {
                                                on_change(false, window, cx);
                                            }
                                        }
                                    }),
                            )
                        }),
                )
            })
            .when_some(content, |panel, content| {
                panel.map(|this| {
                    if height.is_some() {
                        this.child(
                            ScrollArea::new("dialog-content")
                                .handle(scroll)
                                .flex_1()
                                .min_h_0()
                                .child(div().p_2().child(content)),
                        )
                    } else {
                        this.child(div().p_2().child(content))
                    }
                })
            })
            .when_some(actions, |panel, actions| {
                panel.child(
                    h_flex()
                        .px_2()
                        .h_12()
                        .justify_end()
                        .gap_2()
                        .children(actions),
                )
            })
            .child(
                canvas(|_, _, _| {}, {
                    move |_, _, window, cx| {
                        if !focus.contains_focused(window, cx) {
                            window.focus(&focus, cx);
                        }
                    }
                })
                .absolute()
                .inset_0(),
            );

        let scrim = div()
            .absolute()
            .inset_0()
            .bg(scrim_color)
            .occlude()
            .when(self.grain, |scrim| scrim.child(grain()))
            .when(!alert && close_on_backdrop, |scrim| {
                scrim.on_mouse_down(MouseButton::Left, {
                    let on_change = on_change.clone();
                    move |_, _window, cx| {
                        if let Some(on_change) = &on_change {
                            on_change(false, _window, cx);
                        }
                        cx.stop_propagation();
                    }
                })
            });

        deferred(
            root.flex()
                .items_center()
                .justify_center()
                .p_4()
                .refine_style(&self.style)
                .child(scrim.with_animation(
                    "dialog-scrim-enter",
                    Animation::new(enter_duration).with_easing(ease_out_quint()),
                    |scrim, delta| scrim.opacity(delta),
                ))
                .child(panel.with_animation(
                    "dialog-enter",
                    Animation::new(enter_duration).with_easing(ease_out_quint()),
                    |panel, delta| panel.opacity(delta),
                )),
        )
        .with_priority(DIALOG_LAYER)
        .into_any_element()
    }
}

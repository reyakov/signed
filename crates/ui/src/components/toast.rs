//! Toasts: succinct, temporary messages at the bottom center of the window.
//!
//! A [`Toasts`] entity holds the raised toast, and a [`ToastsView`] renders
//! it; mount the view at the window root.
//!
//! # Examples
//!
//! ```no_run
//! use ui::toast::{Toast, ToastAction};
//!
//! let toast = Toast::new("Copied to clipboard")
//!     .action(ToastAction::new("Undo", |_cx| println!("undo")));
//! ```

use std::rc::Rc;
use std::time::Duration;

use gpui::prelude::FluentBuilder;
use gpui::{
    Animation, AnimationExt as _, App, Context, ElementId, Entity, InteractiveElement as _,
    IntoElement, ParentElement as _, RenderOnce, Role, SharedString,
    StatefulInteractiveElement as _, StyleRefinement, Styled as _, Task, Window, deferred, div,
    ease_out_quint, px,
};

use crate::components::button::{Button, ButtonVariant};
use crate::components::common::{
    DismissHandler, ResultLogErr as _, Sizable as _, StyledExt as _, TOAST_LAYER,
};
use crate::icons::{Icon, IconRole};
use crate::theme::{ActiveTheme as _, TOAST_MAX_WIDTH};

/// The action handler of a [`Toast`].
pub type ActionHandler = Rc<dyn Fn(&mut App)>;

/// One user action offered by a toast.
#[derive(Clone)]
pub struct ToastAction {
    label: SharedString,
    handler: ActionHandler,
}

impl ToastAction {
    /// Creates an action with this label.
    pub fn new<L, F>(label: L, handler: F) -> Self
    where
        L: Into<SharedString>,
        F: Fn(&mut App) + 'static,
    {
        Self {
            label: label.into(),
            handler: Rc::new(handler),
        }
    }
}

/// A toast notification.
#[derive(Clone)]
#[must_use]
pub struct Toast {
    message: SharedString,
    icon: Option<Icon>,
    action: Option<ToastAction>,
    duration: Option<Duration>,
    on_dismiss: Option<DismissHandler>,
    style: StyleRefinement,
}

impl Toast {
    /// How long a toast stays up by default.
    pub const DURATION: Duration = Duration::from_secs(5);

    /// Creates a toast with this one-line content.
    pub fn new<L>(message: L) -> Self
    where
        L: Into<SharedString>,
    {
        Self {
            message: message.into(),
            icon: None,
            action: None,
            duration: Some(Self::DURATION),
            on_dismiss: None,
            style: StyleRefinement::default(),
        }
    }

    /// Shows this glyph before the content.
    pub fn icon<I>(mut self, icon: I) -> Self
    where
        I: Into<Icon>,
    {
        self.icon = Some(icon.into());
        self
    }

    /// Offers the user this action.
    pub fn action(mut self, action: ToastAction) -> Self {
        self.action = Some(action);
        self
    }

    /// Sets the time before the toast dismisses itself.
    pub fn duration(mut self, duration: Duration) -> Self {
        self.duration = Some(duration);
        self
    }

    /// Runs when the toast is dismissed by any means, once it is fully gone.
    pub fn on_dismiss<F>(mut self, handler: F) -> Self
    where
        F: Fn(&mut App) + 'static,
    {
        self.on_dismiss = Some(Rc::new(handler));
        self
    }
}

// Toast is a data builder, not an element, adopt the combinators explicitly.
impl FluentBuilder for Toast {}

/// One raised toast, with its identity in the store.
#[derive(Clone)]
pub struct ToastEntry {
    pub id: usize,
    pub toast: Toast,
}

/// The store of raised toasts.
#[derive(Default)]
pub struct Toasts {
    current: Option<ToastEntry>,
    exiting: Option<ToastEntry>,
    next_id: usize,
    /// Pending dismiss and fade timers; held so they die with the store.
    tasks: Vec<Task<()>>,
}

impl Toasts {
    /// Raises a toast, replacing the one on screen. Returns its id.
    pub fn push(&mut self, toast: Toast, cx: &mut Context<Self>) -> usize {
        let id = self.next_id;
        self.next_id += 1;

        let duration = toast.duration;

        if let Some(previous) = self.current.take() {
            self.start_exit(previous, cx);
        }

        self.current = Some(ToastEntry { id, toast });
        cx.notify();

        if let Some(duration) = duration.filter(|duration| !duration.is_zero()) {
            let task = cx.spawn(async move |this, cx| {
                cx.background_executor().timer(duration).await;
                this.update(cx, |toasts, cx| toasts.dismiss(id, cx))
                    .log_err();
            });
            self.tasks.push(task);
        }

        id
    }

    /// Dismisses one toast by id.
    pub fn dismiss(&mut self, id: usize, cx: &mut Context<Self>) {
        if self.exiting.as_ref().is_some_and(|entry| entry.id == id) {
            self.finish_exit(cx);
            return;
        }
        if let Some(entry) = self.current.take_if(|entry| entry.id == id) {
            self.start_exit(entry, cx);
        }
    }

    /// Dismisses the toast on screen.
    pub fn clear(&mut self, cx: &mut Context<Self>) {
        if let Some(entry) = self.current.take() {
            self.start_exit(entry, cx);
        } else {
            self.finish_exit(cx);
        }
    }

    /// The toast now showing, if any.
    pub fn current(&self) -> Option<&ToastEntry> {
        self.current.as_ref()
    }

    /// How many toasts are on screen, including one fading out.
    pub fn count(&self) -> usize {
        usize::from(self.current.is_some()) + usize::from(self.exiting.is_some())
    }

    /// Moves a toast into the fade-out phase.
    fn start_exit(&mut self, entry: ToastEntry, cx: &mut Context<Self>) {
        if let Some(previous) = self.exiting.take()
            && let Some(on_dismiss) = &previous.toast.on_dismiss
        {
            on_dismiss(cx);
        }

        self.exiting = Some(entry);
        cx.notify();

        let fade = cx.theme().motion.overlay;
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(fade).await;
            this.update(cx, |toasts, cx| toasts.finish_exit(cx))
                .log_err();
        });
        self.tasks.push(task);
    }

    fn finish_exit(&mut self, cx: &mut Context<Self>) {
        if let Some(entry) = self.exiting.take() {
            cx.notify();
            if let Some(on_dismiss) = &entry.toast.on_dismiss {
                on_dismiss(cx);
            }
        }
    }
}

/// The floating stack of toasts, bottom center.
#[derive(IntoElement)]
#[must_use]
pub struct ToastsView {
    toasts: Entity<Toasts>,
    id: ElementId,
}

impl ToastsView {
    /// Binds the view to a toast store.
    pub fn new<I>(id: I, toasts: Entity<Toasts>) -> Self
    where
        I: Into<ElementId>,
    {
        Self {
            id: id.into(),
            toasts,
        }
    }
}

impl RenderOnce for ToastsView {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let fade = cx.theme().motion.overlay;

        let exiting = self.toasts.read(cx).exiting.clone();
        let current = self.toasts.read(cx).current.clone();

        deferred(
            div()
                .id(self.id)
                .absolute()
                .left_0()
                .bottom_4()
                .w_full()
                .flex()
                .flex_col()
                .items_center()
                .when_some(exiting, |column, entry| {
                    let id = entry.id;
                    column.child(entry.pill(&self.toasts, cx).with_animation(
                        ("toast-exit", id),
                        Animation::new(fade),
                        |pill, delta| pill.opacity(1. - delta),
                    ))
                })
                .when_some(current, |column, entry| {
                    let id = entry.id;
                    column.child(entry.pill(&self.toasts, cx).with_animation(
                        ("toast-enter", id),
                        Animation::new(fade).with_easing(ease_out_quint()),
                        |pill, delta| pill.opacity(delta),
                    ))
                }),
        )
        .with_priority(TOAST_LAYER)
    }
}

impl ToastEntry {
    /// The rendered pill for this toast.
    fn pill(&self, toasts: &Entity<Toasts>, cx: &App) -> gpui::Stateful<gpui::Div> {
        let theme = cx.theme();
        let surface = theme.colors.surface;
        let border = theme.colors.border;
        let foreground = theme.colors.foreground;
        let radius = theme.radius.full;
        let shadows = theme.elevations.popover.shadows();
        let icon_size = theme.sizes.medium.icon_size;
        let text_size = theme.sizes.medium.text_size;
        let toast = &self.toast;
        let toasts = toasts.clone();
        let id = self.id;

        div()
            .id(("toast", id))
            .debug_selector(|| "toast-pill".to_string())
            .role(Role::Alert)
            .aria_label(toast.message.clone())
            .max_w(TOAST_MAX_WIDTH)
            .flex()
            .items_center()
            .gap_2()
            .py_1p5()
            .px_3()
            .bg(surface)
            .border_1()
            .border_color(border)
            .rounded(radius)
            .shadow(shadows.clone())
            .refine_style(&toast.style)
            .when_some(toast.icon.clone(), |pill, icon| {
                pill.child(
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .child(icon.size(icon_size).color(foreground)),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .truncate()
                    .text_size(text_size)
                    .text_color(foreground)
                    .child(toast.message.clone()),
            )
            .when_some(toast.action.clone(), |pill, action| {
                let toasts = toasts.clone();
                pill.child(
                    div().flex_none().child(
                        Button::new(("toast-action", id))
                            .label(action.label.clone())
                            .variant(ButtonVariant::Primary)
                            .small()
                            .on_click(move |_, _window, cx| {
                                (action.handler)(cx);
                                toasts.update(cx, |toasts, cx| {
                                    toasts.dismiss(id, cx);
                                });
                            }),
                    ),
                )
            })
            .child(
                div().flex_none().child(
                    Button::new(("toast-close", id))
                        .icon(IconRole::Close)
                        .variant(ButtonVariant::Ghost)
                        .small()
                        .accessibility_label("Dismiss")
                        .on_click(move |_, _window, cx| {
                            toasts.update(cx, |toasts, cx| {
                                toasts.dismiss(id, cx);
                            });
                        }),
                ),
            )
    }
}

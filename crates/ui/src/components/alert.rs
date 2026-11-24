//! Renders alert banners and the floating stack they push onto.
//!
//! An [`Alert`] is a value that renders as an [`AlertStyle::Overlay`] banner or
//! an [`AlertStyle::Inline`] callout. An [`Alerts`] entity holds the pushed
//! alerts, and an [`AlertsView`] renders the stack; mount the view at the
//! window root.
//!
//! # Examples
//!
//! ```no_run
//! use ui::alert::Alert;
//!
//! let alert = Alert::new("disk-warning")
//!     .title("Storage almost full")
//!     .message("Delete unused projects to free up space.")
//!     .on_dismiss(|_cx| println!("dismissed"));
//! ```

use std::rc::Rc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    Animation, AnimationExt as _, App, Context, ElementId, Entity, InteractiveElement as _,
    IntoElement, ParentElement as _, RenderOnce, Role, SharedString,
    StatefulInteractiveElement as _, StyleRefinement, Styled as _, Window, deferred, div,
    ease_out_quint, rems,
};

use crate::components::button::{Button, ButtonVariant};
use crate::components::common::{
    DismissHandler, ResultLogErr as _, Sizable as _, StyledExt as _, TOAST_LAYER, floating_panel,
    v_flex,
};
use crate::icons::{Icon, IconRole};
use crate::theme::{ALERT_EDGE_WIDTH, ALERT_STACK_WIDTH, ActiveTheme as _, ENTER_SLIDE_DISTANCE};

/// How alarming an [`Alert`] is; picks the accent color.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AlertSeverity {
    /// The default; uses the info role.
    #[default]
    Info,
    /// Uses the success role.
    Success,
    /// Uses the warning role.
    Warning,
    /// Uses the destructive role.
    Danger,
}

/// How an [`Alert`] presents.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AlertStyle {
    /// A floating banner styled like a popover; the default.
    #[default]
    Overlay,
    /// A tinted callout embedded in the page.
    Inline,
}

/// A static callout banner.
#[derive(Clone, IntoElement)]
#[must_use]
pub struct Alert {
    id: ElementId,
    title: Option<SharedString>,
    message: Option<SharedString>,
    severity: AlertSeverity,
    presentation: AlertStyle,
    icon: Option<Icon>,
    on_dismiss: Option<DismissHandler>,
    style: StyleRefinement,
}

impl Alert {
    /// Creates an alert.
    pub fn new<I>(id: I) -> Self
    where
        I: Into<ElementId>,
    {
        Self {
            id: id.into(),
            title: None,
            message: None,
            severity: AlertSeverity::Info,
            presentation: AlertStyle::default(),
            icon: None,
            on_dismiss: None,
            style: StyleRefinement::default(),
        }
    }

    /// Sets the primary line.
    pub fn title<L>(mut self, title: L) -> Self
    where
        L: Into<SharedString>,
    {
        self.title = Some(title.into());
        self
    }

    /// Sets a secondary line under the title.
    pub fn message<L>(mut self, message: L) -> Self
    where
        L: Into<SharedString>,
    {
        self.message = Some(message.into());
        self
    }

    /// Sets how alarming the alert is.
    pub fn severity(mut self, severity: AlertSeverity) -> Self {
        self.severity = severity;
        self
    }

    /// Sets how the alert presents.
    pub fn style(mut self, style: AlertStyle) -> Self {
        self.presentation = style;
        self
    }

    /// Shows an icon before the text.
    pub fn icon<I>(mut self, icon: I) -> Self
    where
        I: Into<Icon>,
    {
        self.icon = Some(icon.into());
        self
    }

    /// Runs when the user dismisses the alert.
    pub fn on_dismiss<F>(mut self, handler: F) -> Self
    where
        F: Fn(&mut App) + 'static,
    {
        self.on_dismiss = Some(Rc::new(handler));
        self
    }

    fn rekey(mut self, id: usize) -> Self {
        self.id = ElementId::from(id);
        self
    }
}

impl gpui::Styled for Alert {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for Alert {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();

        let ramp = match self.severity {
            AlertSeverity::Info => theme.ramps.info,
            AlertSeverity::Success => theme.ramps.success,
            AlertSeverity::Warning => theme.ramps.warning,
            AlertSeverity::Danger => theme.ramps.destructive,
        };

        let foreground = theme.colors.foreground;
        let muted_foreground = theme.colors.muted_foreground;

        let content = v_flex()
            .flex_1()
            .gap_0p5()
            .when_some(self.title.clone(), |alert, title| {
                alert.child(
                    div()
                        .text_sm()
                        .font_semibold()
                        .text_color(foreground)
                        .child(title),
                )
            })
            .when_some(self.message.clone(), |alert, message| {
                let color = if self.title.is_some() {
                    muted_foreground
                } else {
                    foreground
                };
                alert.child(div().text_sm().text_color(color).child(message))
            });

        let dismiss = self.on_dismiss.clone().map(|on_dismiss| {
            Button::new((self.id.clone(), "alert-close"))
                .icon(IconRole::Close)
                .variant(ButtonVariant::Ghost)
                .xsmall()
                .accessibility_label("Dismiss")
                .on_click(move |_, _window, cx| on_dismiss(cx))
        });

        match self.presentation {
            AlertStyle::Overlay => floating_panel(
                cx,
                theme.radius.overlay,
                div()
                    .id(self.id.clone())
                    .role(Role::Alert)
                    .w_full()
                    .flex()
                    .items_start()
                    .gap_2()
                    .px_3()
                    .py_2(),
                theme.elevations.popover.shadows(),
            )
            .refine_style(&self.style)
            .when_some(self.icon.clone(), |alert, icon_glyph| {
                alert.child(
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .h(window.line_height())
                        .child(icon_glyph.size(rems(1.0)).color(ramp.base)),
                )
            })
            .child(content)
            .when_some(dismiss, |alert, dismiss| alert.child(dismiss)),
            AlertStyle::Inline => div()
                .id(self.id.clone())
                .role(Role::Alert)
                .w_full()
                .flex()
                .overflow_hidden()
                .bg(ramp.subtle)
                .refine_style(&self.style)
                .child(
                    div()
                        .flex_none()
                        .w(ALERT_EDGE_WIDTH)
                        .self_stretch()
                        .bg(ramp.base),
                )
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .items_start()
                        .gap_3()
                        .px_3()
                        .py_2()
                        .when_some(self.icon.clone(), |alert, icon_glyph| {
                            alert.child(
                                div()
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .size_6()
                                    .rounded(theme.radius.full)
                                    .bg(ramp.base)
                                    .child(icon_glyph.size(rems(0.75)).color(ramp.foreground)),
                            )
                        })
                        .child(content),
                )
                .when_some(dismiss, |alert, dismiss| {
                    alert.child(div().flex_none().self_center().pr_3().child(dismiss))
                }),
        }
    }
}

/// One pushed alert, with its identity in the stack.
#[derive(Clone)]
struct AlertEntry {
    /// The id [`Alerts::push`] returned.
    pub id: usize,
    /// The alert as pushed.
    pub alert: Alert,
    on_dismiss: Option<DismissHandler>,
}

/// The stack of pushed alerts.
#[derive(Default)]
pub struct Alerts {
    entries: Vec<AlertEntry>,
    next_id: usize,
}

impl Alerts {
    /// Pushes an alert onto the stack and returns its id.
    pub fn push(&mut self, mut alert: Alert, cx: &mut Context<Self>) -> usize {
        let id = self.next_id;
        self.next_id += 1;

        let on_dismiss = alert.on_dismiss.take();
        let stack = cx.entity().downgrade();

        alert.on_dismiss = Some(Rc::new(move |cx: &mut App| {
            stack
                .update(cx, |alerts, cx| alerts.dismiss(id, cx))
                .log_err();
        }));

        self.entries.push(AlertEntry {
            id,
            alert: alert.rekey(id),
            on_dismiss,
        });

        cx.notify();

        id
    }

    /// Dismisses one alert by id, running its dismiss handler.
    pub fn dismiss(&mut self, id: usize, cx: &mut Context<Self>) {
        if let Some(position) = self.entries.iter().position(|entry| entry.id == id) {
            let entry = self.entries.remove(position);
            cx.notify();
            if let Some(on_dismiss) = &entry.on_dismiss {
                (on_dismiss.clone())(cx);
            }
        }
    }

    /// Dismisses every alert, running each entry's `on_dismiss` handler.
    pub fn clear(&mut self, cx: &mut Context<Self>) {
        if self.entries.is_empty() {
            return;
        }

        let entries = std::mem::take(&mut self.entries);
        cx.notify();

        for entry in entries {
            if let Some(on_dismiss) = entry.on_dismiss {
                on_dismiss(cx);
            }
        }
    }

    /// The stacked alerts, oldest first.
    fn entries(&self) -> &[AlertEntry] {
        &self.entries
    }
}

/// The floating stack of pushed alerts, topmost right.
#[derive(IntoElement)]
#[must_use]
pub struct AlertsView {
    alerts: Entity<Alerts>,
    id: ElementId,
}

impl AlertsView {
    /// Binds the view to an alert stack.
    pub fn new<I>(id: I, alerts: Entity<Alerts>) -> Self
    where
        I: Into<ElementId>,
    {
        Self {
            id: id.into(),
            alerts,
        }
    }
}

impl RenderOnce for AlertsView {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let enter_duration = theme.motion.overlay;

        let entries = self.alerts.read(cx).entries();

        deferred(
            v_flex()
                .id(self.id)
                .absolute()
                .top_4()
                .right_4()
                .w(ALERT_STACK_WIDTH)
                .items_end()
                .gap_2()
                .children(entries.iter().rev().map(|entry| {
                    let id = entry.id;
                    let ease_slide = ease_out_quint();

                    div()
                        .relative()
                        .w_full()
                        .debug_selector(|| "alert-entry".to_string())
                        .child(entry.alert.clone())
                        .with_animation(
                            ("alert-enter", id),
                            Animation::new(enter_duration),
                            move |layer, delta| {
                                layer.left(ENTER_SLIDE_DISTANCE * (1. - ease_slide(delta)))
                            },
                        )
                })),
        )
        .with_priority(TOAST_LAYER)
    }
}

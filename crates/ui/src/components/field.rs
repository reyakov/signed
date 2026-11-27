//! Wraps a control with a label, description, and error text.
//!
//! # Examples
//!
//! ```no_run
//! use gpui::{div, ParentElement};
//! use ui::field::{Field, FieldVariant};
//!
//! let sync = Field::new("sync", "Sync settings")
//!     .description("Keep your work synced across devices")
//!     .variant(FieldVariant::Row)
//!     .child(div());
//! ```

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, ElementId, InteractiveElement as _, IntoElement, ParentElement, RenderOnce,
    Role, SharedString, StatefulInteractiveElement as _, StyleRefinement, Styled, Window, div,
};

use crate::components::common::{StyledExt, h_flex, v_flex};
use crate::theme::ActiveTheme as _;

/// The arrangement of a [`Field`]'s label and control.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FieldVariant {
    /// The label block sits above the control, full width.
    #[default]
    Block,
    /// The label block leads and the control trails, as in a settings row.
    Row,
}

/// A control presented with a label, an optional description, and an error.
#[derive(IntoElement)]
#[must_use]
pub struct Field {
    id: ElementId,
    label: SharedString,
    description: Option<SharedString>,
    error: Option<SharedString>,
    variant: FieldVariant,
    control: Vec<AnyElement>,
    style: StyleRefinement,
}

impl Field {
    /// Creates a field naming its control.
    pub fn new<I, L>(id: I, label: L) -> Self
    where
        I: Into<ElementId>,
        L: Into<SharedString>,
    {
        Self {
            id: id.into(),
            label: label.into(),
            description: None,
            error: None,
            variant: FieldVariant::default(),
            control: Vec::new(),
            style: StyleRefinement::default(),
        }
    }

    /// Sets the helper text under the label.
    pub fn description<D>(mut self, description: D) -> Self
    where
        D: Into<SharedString>,
    {
        self.description = Some(description.into());
        self
    }

    /// Sets the error under the control; `None` clears it.
    pub fn error<E>(mut self, error: Option<E>) -> Self
    where
        E: Into<SharedString>,
    {
        self.error = error.map(Into::into);
        self
    }

    /// Sets the field's arrangement.
    pub fn variant(mut self, variant: FieldVariant) -> Self {
        self.variant = variant;
        self
    }
}

impl ParentElement for Field {
    fn extend(&mut self, children: impl IntoIterator<Item = AnyElement>) {
        self.control.extend(children);
    }
}

impl Styled for Field {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for Field {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let error_color = theme.ramps.destructive.base;
        let row = self.variant == FieldVariant::Row;
        let id = self.id.clone();

        div()
            .id(self.id)
            .w_full()
            .role(Role::Group)
            .aria_label(self.label.clone())
            .when_some(self.description.clone(), |field, description| {
                field.aria_description(description)
            })
            .refine_style(&self.style)
            .child(
                v_flex()
                    .gap_1()
                    .when(!row, |field| field.gap_2())
                    .child(
                        div()
                            .flex()
                            .when(row, |field| {
                                field.flex_row().items_center().justify_between().gap_4()
                            })
                            .when(!row, |field| field.flex_col().gap_2())
                            .child(
                                div()
                                    .when(row, |label| label.flex_1().min_w_0())
                                    .when(!row, |label| label.flex().flex_col().gap_0p5())
                                    .child(
                                        div()
                                            .debug_selector({
                                                let id = id.clone();
                                                move || format!("{id}-label")
                                            })
                                            .text_sm()
                                            .when(self.description.is_some(), |title| {
                                                title.font_semibold()
                                            })
                                            .child(self.label.clone()),
                                    )
                                    .when(!row, |label| {
                                        label.when_some(
                                            self.description.clone(),
                                            |slot, description| {
                                                slot.child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(theme.colors.muted_foreground)
                                                        .child(description),
                                                )
                                            },
                                        )
                                    }),
                            )
                            .child(
                                div().when(row, |control| control.flex_none()).child(
                                    h_flex()
                                        .items_center()
                                        .gap_2()
                                        .debug_selector({
                                            let id = id.clone();
                                            move || format!("{id}-control")
                                        })
                                        .children(self.control),
                                ),
                            ),
                    )
                    .when(row, |field| {
                        field.when_some(self.description.clone(), |field, description| {
                            field.child(
                                div()
                                    .text_xs()
                                    .text_color(theme.colors.muted_foreground)
                                    .child(description),
                            )
                        })
                    })
                    .when_some(self.error, |field, error| {
                        field.child(
                            div()
                                .debug_selector({
                                    let id = id.clone();
                                    move || format!("{id}-error")
                                })
                                .text_xs()
                                .text_color(error_color)
                                .child(error),
                        )
                    }),
            )
    }
}

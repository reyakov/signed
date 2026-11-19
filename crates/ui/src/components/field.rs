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
use crate::theme::{ActiveTheme as _, TextRole, Typeset as _};

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
        let metrics = theme.sizes.medium;
        let error_color = theme.ramps.destructive.base;
        let id = self.id.clone();

        let title = div()
            .debug_selector({
                let id = id.clone();
                move || format!("{id}-label")
            })
            .when(self.description.is_some(), |title| {
                title.typeset(TextRole::Headline, cx)
            })
            .when(self.description.is_none(), |title| {
                title.typeset(TextRole::Callout, cx)
            })
            .child(self.label.clone());

        let control = h_flex()
            .items_center()
            .gap_2()
            .debug_selector({
                let id = id.clone();
                move || format!("{id}-control")
            })
            .children(self.control);

        let error = self.error.map(|error| {
            div()
                .debug_selector({
                    let id = id.clone();
                    move || format!("{id}-error")
                })
                .text_size(metrics.caption_size)
                .text_color(error_color)
                .child(error)
        });

        let description_text = |cx: &App| {
            div()
                .text_size(metrics.caption_size)
                .text_color(cx.theme().colors.muted_foreground)
        };

        let content = if self.variant == FieldVariant::Row {
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    h_flex()
                        .items_center()
                        .justify_between()
                        .gap_4()
                        .child(title.flex_1().min_w_0())
                        .child(control.flex_none()),
                )
                .when_some(self.description.clone(), |field, description| {
                    field.child(description_text(cx).child(description))
                })
                .when_some(error, |field, error| field.child(error))
        } else {
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    v_flex()
                        .gap_0p5()
                        .child(title)
                        .when_some(self.description.clone(), |slot, description| {
                            slot.child(description_text(cx).child(description))
                        }),
                )
                .child(control)
                .when_some(error, |field, error| field.child(error))
        };

        div()
            .id(self.id)
            .w_full()
            .role(Role::Group)
            .aria_label(self.label.clone())
            .when_some(self.description, |field, description| {
                field.aria_description(description)
            })
            .refine_style(&self.style)
            .child(content)
    }
}

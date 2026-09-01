//! Reusable settings UI: rows and blocks for building a settings dialog, plus
//! a labeled dropdown option.

use gpui::prelude::*;
use gpui::{App, SharedString, div};
use gpui_component::searchable_list::SearchableListItem;
use gpui_component::{ActiveTheme, StyledExt, h_flex, v_flex};

/// A dropdown option with a display label and a stored value.
///
/// Renders the `label` in the trigger and the menu, while `value` is what a
/// [`gpui_component::select::SelectState`] reports as the selection.
#[derive(Clone)]
pub struct SelectOption {
    value: SharedString,
    label: SharedString,
}

impl SelectOption {
    /// Create an option with the given stored `value` and display `label`.
    pub fn new(value: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
        }
    }

    /// The stored value of this option.
    pub fn value(&self) -> &SharedString {
        &self.value
    }

    /// The display label of this option.
    pub fn label(&self) -> &SharedString {
        &self.label
    }
}

impl SearchableListItem for SelectOption {
    type Value = SharedString;

    fn title(&self) -> SharedString {
        self.label.clone()
    }

    fn value(&self) -> &Self::Value {
        &self.value
    }
}

/// A settings row: label + description on the left, control on the right.
pub fn setting_row(
    cx: &App,
    title: impl Into<SharedString>,
    description: impl Into<SharedString>,
    control: impl IntoElement,
) -> impl IntoElement {
    let title = title.into();
    let description = description.into();

    h_flex()
        .w_full()
        .justify_between()
        .items_center()
        .gap_4()
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .child(div().text_sm().font_semibold().child(title))
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(description),
                ),
        )
        .child(
            h_flex()
                .w_40()
                .flex_shrink_0()
                .justify_end()
                .items_center()
                .child(control),
        )
}

/// A full-width settings block: title + subtitle in one header, `gap_3`
/// between the header and the control below.
pub fn setting_block(
    cx: &App,
    title: impl Into<SharedString>,
    description: impl Into<SharedString>,
    control: impl IntoElement,
) -> impl IntoElement {
    let title = title.into();
    let description = description.into();
    v_flex()
        .w_full()
        .gap_3()
        .child(
            v_flex()
                .w_full()
                .child(div().text_sm().font_semibold().child(title))
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(description),
                ),
        )
        .child(control)
}

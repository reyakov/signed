use assets::CustomIconName;
use gpui::prelude::*;
use gpui::{AnyElement, App, SharedString, div};
use gpui_component::combobox::{Caret, ComboboxTriggerContext};
use gpui_component::searchable_list::SearchableVec;
use gpui_component::{ActiveTheme, Icon, Sizable, h_flex};

/// The kind icon, the selection or placeholder, and the caret. `Combobox`
/// replaces its default trigger entirely, the only way to show an icon inside it.
pub fn ref_selector_trigger(
    ctx: &ComboboxTriggerContext<SearchableVec<SharedString>>,
    icon: CustomIconName,
    cx: &App,
) -> AnyElement {
    let muted = cx.theme().muted_foreground;

    h_flex()
        .w_full()
        .min_w_0()
        .gap_1()
        .items_center()
        .child(Icon::new(icon).small().flex_shrink_0())
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .text_ellipsis()
                .whitespace_nowrap()
                .when(ctx.selection().is_empty(), |this| this.text_color(muted))
                .child(
                    ctx.selection()
                        .first()
                        .map(|(_, item)| item.clone())
                        .or_else(|| ctx.placeholder().cloned())
                        .unwrap_or_default(),
                ),
        )
        .child(Caret::new(ctx.size()).text_color(muted))
        .into_any_element()
}

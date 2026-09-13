use gpui::prelude::*;
use gpui::{
    Anchor, AnyElement, App, DismissEvent, ElementId, Entity, Focusable, SharedString,
    StyleRefinement, Window, px,
};
use gpui_base::{Button as BaseButton, Popover, Selectable, StyledExt};
use gpui_component::menu::PopupMenu;
use gpui_component::{ActiveTheme, Icon, IconName, Sizable, h_flex};

/// A split dropdown button built on `gpui_base::Popover`.
/// An action element with a separate caret trigger that opens a [`PopupMenu`].
/// The action and the caret are caller-supplied elements, so the look stays in the app.
/// This component only owns the popover wiring.
#[derive(IntoElement)]
pub struct DropdownButton {
    id: ElementId,
    style: StyleRefinement,
    anchor: Anchor,
    action: Option<AnyElement>,
    caret: Option<CaretBuilder>,
    menu: Option<MenuBuilder>,
}

type MenuBuilder =
    Box<dyn Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static>;
type CaretBuilder = Box<dyn FnOnce(bool, &Window, &App) -> AnyElement>;

impl DropdownButton {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            style: StyleRefinement::default(),
            anchor: Anchor::TopRight,
            action: None,
            caret: None,
            menu: None,
        }
    }

    /// The action half of the button.
    /// It keeps its own icon, label, tooltip and click handler.
    pub fn action(mut self, action: impl IntoElement + 'static) -> Self {
        self.action = Some(action.into_any_element());
        self
    }

    /// The menu built by `builder`.
    /// Matches gpui-component's `DropdownButton::dropdown_menu` signature.
    /// Existing menu code keeps working.
    pub fn dropdown_menu(
        mut self,
        builder: impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static,
    ) -> Self {
        self.menu = Some(Box::new(builder));
        self
    }

    /// Which corner of the caret the menu anchors to.
    /// Defaults to [`Anchor::TopRight`], lining the menu's right edge up with the caret's.
    #[allow(dead_code)] // API knob, current call sites use the default anchor.
    pub fn anchor(mut self, anchor: impl Into<Anchor>) -> Self {
        self.anchor = anchor.into();
        self
    }
}

impl Styled for DropdownButton {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

/// Holds the [`PopupMenu`] entity of one popover between renders.
/// Dismissal drops it, so the menu is rebuilt with fresh items on the next open.
#[derive(Default)]
struct DropdownMenuState {
    menu: Option<Entity<PopupMenu>>,
}

impl RenderOnce for DropdownButton {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        debug_assert!(
            self.menu.is_some(),
            "a DropdownButton needs a `dropdown_menu`"
        );

        // The popover needs its own id.
        // The container and the popover both register keyed state on this window.
        let popover_id = SharedString::from(format!("{}-popover", self.id));
        let anchor = self.anchor;
        let menu_state =
            window.use_keyed_state(popover_id.clone(), cx, |_, _| DropdownMenuState::default());

        let caret = self.caret.unwrap_or_else(|| {
            let id = popover_id.clone();
            Box::new(move |is_open, _, cx| {
                let caret = default_caret(id.clone(), cx);
                let selected = caret.is_selected();
                caret.selected(selected || is_open).into_any_element()
            })
        });

        h_flex()
            .id(self.id)
            .refine_style(&self.style)
            .gap_0p5()
            .when_some(self.action, |this, action| this.child(action))
            .when_some(self.menu, |this, builder| {
                this.child(
                    Popover::new(popover_id)
                        .anchor(anchor)
                        // The menu dismisses itself on outside click or Escape.
                        // The subscription below closes the popover along with it.
                        .overlay_closable(false)
                        .trigger_with(caret)
                        .content(
                            move |_, window, cx| match menu_state.read(cx).menu.clone() {
                                Some(menu) => menu,
                                None => {
                                    let menu = PopupMenu::build(window, cx, |menu, window, cx| {
                                        builder(menu, window, cx)
                                    });
                                    menu_state
                                        .update(cx, |state, _| state.menu = Some(menu.clone()));
                                    menu.focus_handle(cx).focus(window, cx);

                                    let popover_state = cx.entity();
                                    window
                                        .subscribe(&menu, cx, {
                                            let menu_state = menu_state.clone();
                                            move |_, _: &DismissEvent, window, cx| {
                                                popover_state.update(cx, |state, cx| {
                                                    state.dismiss(window, cx);
                                                });
                                                menu_state.update(cx, |state, _| {
                                                    state.menu = None;
                                                });
                                            }
                                        })
                                        .detach();

                                    menu.clone()
                                }
                            },
                        ),
                )
            })
    }
}

/// The default caret, a chevron button the height of a medium button.
/// It is tinted by the theme and styled for hover and menu-open states.
fn default_caret(id: impl Into<ElementId>, cx: &App) -> BaseButton {
    BaseButton::new(id)
        .h(px(32.))
        .px_1p5()
        .text_color(cx.theme().muted_foreground)
        .hover(|style| style.bg(cx.theme().secondary_hover))
        .styles(|this| {
            this.selected(|style| style.bg(cx.theme().secondary_active))
                .disabled(|style| style.opacity(0.5))
        })
        .child(Icon::new(IconName::ChevronDown).xsmall())
}

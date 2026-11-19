use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, Context, Entity, FontWeight, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Window, div,
};
use ui::button::{Button, ButtonVariant};
use ui::common::v_flex;
use ui::layout::panel::Panel;

use crate::identity::import::ImportDialog;
use crate::identity::onboarding::OnboardingDialog;

pub struct SignedOutView {
    onboarding: Option<Entity<OnboardingDialog>>,
    import: Option<Entity<ImportDialog>>,
}

impl SignedOutView {
    pub fn new() -> Self {
        Self {
            onboarding: None,
            import: None,
        }
    }

    fn open_onboarding(&mut self, cx: &mut Context<Self>) {
        self.onboarding = Some(OnboardingDialog::new(cx));
        cx.notify();
    }

    fn open_import(&mut self, cx: &mut Context<Self>) {
        self.import = Some(ImportDialog::new(cx));
        cx.notify();
    }
}

impl Panel for SignedOutView {
    fn title(&self, _cx: &App) -> SharedString {
        SharedString::from("Signed")
    }
}

impl Render for SignedOutView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_4()
            .child(
                div()
                    .text_2xl()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Signed"),
            )
            .child(
                v_flex()
                    .gap_1()
                    .w_48()
                    .child(
                        Button::new("onboarding")
                            .label("Join now")
                            .on_click(cx.listener(|this, _, _, cx| this.open_onboarding(cx))),
                    )
                    .child(
                        Button::new("import")
                            .label("Import identity")
                            .variant(ButtonVariant::Secondary)
                            .on_click(cx.listener(|this, _, _, cx| this.open_import(cx))),
                    ),
            )
            .when_some(self.onboarding.clone(), |el, view| el.child(view))
            .when_some(self.import.clone(), |el, view| el.child(view))
    }
}

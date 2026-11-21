use std::time::{SystemTime, UNIX_EPOCH};

use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, Context, Entity, IntoElement, ObjectFit, ParentElement as _, Render, SharedString,
    Styled as _, StyledImage as _, Window, div, img,
};
use ui::button::{Button, ButtonVariant};
use ui::common::v_flex;
use ui::layout::panel::Panel;
use ui::theme::ActiveTheme as _;

use crate::identity::import::ImportDialog;
use crate::identity::onboarding::OnboardingDialog;

pub struct SignedOutView {
    banner: SharedString,
    onboarding: Option<Entity<OnboardingDialog>>,
    import: Option<Entity<ImportDialog>>,
}

impl SignedOutView {
    pub fn new() -> Self {
        Self {
            banner: pick_banner(),
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

/// One of the three banner photos, picked per process launch.
fn pick_banner() -> SharedString {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|now| now.subsec_nanos())
        .unwrap_or(0);
    format!("backgrounds/banner{}.jpg", nanos % 3 + 1).into()
}

impl Render for SignedOutView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let colors = theme.colors;

        v_flex()
            .size_full()
            .child(
                v_flex()
                    .size_full()
                    .relative()
                    .bg(colors.background)
                    .rounded(theme.radius.overlay)
                    .overflow_hidden()
                    .child(
                        div().absolute().inset_0().child(
                            img(self.banner.clone())
                                .size_full()
                                .object_fit(ObjectFit::Cover)
                                .rounded(theme.radius.overlay),
                        ),
                    )
                    .child(
                        v_flex()
                            .size_full()
                            .justify_end()
                            .p_4()
                            .gap_6()
                            .child(img("backgrounds/headline.png").max_w_48())
                            .child(
                                v_flex()
                                    .gap_2()
                                    .w_full()
                                    .child(
                                        Button::new("join")
                                            .label("Join now")
                                            .variant(ButtonVariant::Primary)
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.open_onboarding(cx)
                                            })),
                                    )
                                    .child(
                                        Button::new("import")
                                            .label("Import identity")
                                            .variant(ButtonVariant::Secondary)
                                            .on_click(
                                                cx.listener(|this, _, _, cx| this.open_import(cx)),
                                            ),
                                    ),
                            ),
                    ),
            )
            .when_some(self.onboarding.clone(), |el, view| el.child(view))
            .when_some(self.import.clone(), |el, view| el.child(view))
    }
}

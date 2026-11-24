pub mod helpers;
mod placeholder;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, Styled as _, Subscription, WeakEntity, Window, div,
};
use helpers::apply_appearance_mode;
use settings::{AppearanceMode, SettingsStore};
use signed_state::{Backend, BackendEvent};
use ui::common::{h_flex, v_flex};
use ui::icons::IconRole;
use ui::layout::panel::PanelStack;
use ui::layout::root::RootContext as _;
use ui::layout::titlebar::Titlebar;
use ui::toast::Toast;

use crate::app::placeholder::PlaceholderView;
use crate::explore::ExploreView;
use crate::identity::passphrase::PassphraseDialog;
use crate::shared::log_dropped;
use crate::titlebar::{
    CreateRepository, REPOSITORY_KEY_CONTEXT, SwitchRepository, TitlebarView,
    bind_repository_shortcuts,
};

pub struct AppRoot {
    panels: Entity<PanelStack>,
    titlebar: Entity<TitlebarView>,
    appearance: AppearanceMode,
    _subscriptions: Vec<Subscription>,
}

impl AppRoot {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        bind_repository_shortcuts(cx);

        let app = cx.entity().downgrade();
        let panels = cx.new(|cx| PanelStack::new("signed-panel", cx));
        let titlebar = cx.new(|cx| TitlebarView::new(app.clone(), window, cx));
        let empty = cx.new(|_| PlaceholderView::new("Select a repository"));
        panels.update(cx, |panels, cx| panels.set_empty(empty, None, cx));

        let backend = Backend::global(cx);
        let settings = SettingsStore::global(cx);
        let mut subscriptions = Vec::new();

        subscriptions.push(cx.subscribe_in(
            &backend,
            window,
            |this, _backend, event, window, cx| match event {
                BackendEvent::PassphraseRequired => {
                    this.open_passphrase(cx);
                }
                BackendEvent::Error(message) => {
                    cx.toast(Toast::new(message.clone()).icon(IconRole::Warning));
                }
                BackendEvent::SignerChanged => {
                    let app = cx.entity().downgrade();
                    window.defer(cx, move |window, cx| {
                        Self::reset_to_explore(&app, window, cx)
                    });
                }
                _ => {}
            },
        ));

        subscriptions.push(cx.observe(&settings, |this, store, cx| {
            let appearance = store.read(cx).settings().appearance;
            if appearance != this.appearance {
                this.appearance = appearance;
                apply_appearance_mode(appearance, cx);
            }
        }));

        window.defer(cx, move |window, cx| {
            if backend.read(cx).passphrase_required() {
                log_dropped(app.update(cx, |this, cx| this.open_passphrase(cx)));
            }
            Self::reset_to_explore(&app, window, cx);
        });

        let appearance = settings.read(cx).settings().appearance;

        Self {
            panels,
            titlebar,
            appearance,
            _subscriptions: subscriptions,
        }
    }

    fn open_passphrase(&mut self, cx: &mut Context<Self>) {
        let dialog = PassphraseDialog::new(cx);
        cx.open_dialog(dialog);
    }

    /// The panel stack: the main stack, the split stack, and the divider between them.
    pub fn panels(&self) -> &Entity<PanelStack> {
        &self.panels
    }

    /// Empties the stack and opens it on the explore panel, where every session starts.
    fn reset_to_explore(app: &WeakEntity<Self>, window: &mut Window, cx: &mut App) {
        let result = app.update(cx, |this, cx| {
            this.panels.update(cx, |panels, cx| panels.clear(cx))
        });
        log_dropped(result);
        ExploreView::open(app, window, cx);
    }
}

impl Render for AppRoot {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let repo_tabs = PanelStack::main_tabs(self.panels.clone(), cx);
        let titlebar_view = self.titlebar.read(cx);

        div()
            .relative()
            .size_full()
            .key_context(REPOSITORY_KEY_CONTEXT)
            .on_action(cx.listener(|this, action: &SwitchRepository, window, cx| {
                this.titlebar.update(cx, |titlebar, cx| {
                    titlebar.activate_shortcut(action.index, window, cx);
                });
            }))
            .on_action(cx.listener(|this, _: &CreateRepository, window, cx| {
                this.titlebar.update(cx, |titlebar, cx| {
                    titlebar.create_repository(window, cx);
                });
            }))
            .child(
                v_flex()
                    .size_full()
                    .overflow_hidden()
                    .child(
                        Titlebar::new("signed-titlebar")
                            .prefix(
                                h_flex()
                                    .gap_1()
                                    .child(titlebar_view.render_new_repository())
                                    .child(titlebar_view.render_explore())
                                    .child(titlebar_view.repository_switcher()),
                            )
                            .tab_bar(
                                h_flex()
                                    .gap_1()
                                    .when_some(repo_tabs, |bar, tabs| bar.child(tabs)),
                            )
                            .when_some(titlebar_view.render_user_menu(cx), |titlebar, menu| {
                                titlebar.suffix(menu)
                            }),
                    )
                    .child(self.panels.clone()),
            )
    }
}

pub mod helpers;
mod placeholder;
mod sign_in;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, Styled as _, Subscription, Window, div,
};
use helpers::apply_appearance;
use settings::SettingsStore;
use signed_state::{Backend, BackendEvent};
use ui::common::h_flex;
use ui::icons::IconRole;
use ui::layout::panel::PanelRow;
use ui::layout::shell::Shell;
use ui::layout::titlebar::Titlebar;
use ui::theme::ActiveTheme as _;
use ui::toast::{Toast, Toasts};

use crate::app::placeholder::PlaceholderView;
use crate::app::sign_in::SignedOutView;
use crate::explore::ExploreView;
use crate::identity::passphrase::PassphraseDialog;
use crate::shared::log_dropped;
use crate::titlebar::{
    CreateRepository, REPOSITORY_KEY_CONTEXT, SwitchRepository, TitlebarView,
    bind_repository_shortcuts,
};
use crate::workspace::Workspace;

pub struct AppRoot {
    workspace: Entity<Workspace>,
    titlebar: Entity<TitlebarView>,
    toasts: Entity<Toasts>,
    sign_in: Entity<SignedOutView>,
    empty: Entity<PlaceholderView>,
    passphrase: Option<Entity<PassphraseDialog>>,
    _subscriptions: Vec<Subscription>,
}

impl AppRoot {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        bind_repository_shortcuts(cx);

        let workspace = cx.new(|_| Workspace::new());
        let titlebar = cx.new(|cx| TitlebarView::new(workspace.downgrade(), window, cx));

        let toasts = cx.new(|_| Toasts::default());
        let sign_in = cx.new(|_| SignedOutView::new());
        let empty = cx.new(|_| PlaceholderView::new("Select a repository"));

        let backend = Backend::global(cx);
        let settings = SettingsStore::global(cx);
        let mut subscriptions = Vec::new();

        subscriptions.push(cx.subscribe_in(
            &backend,
            window,
            |this, backend, event, window, cx| match event {
                BackendEvent::PassphraseRequired => {
                    this.open_passphrase(cx);
                }
                BackendEvent::Error(message) => {
                    this.toasts.update(cx, |toasts, cx| {
                        toasts.push(Toast::new(message.clone()).icon(IconRole::Warning), cx);
                    });
                }
                BackendEvent::SignerChanged => {
                    this.workspace.update(cx, |workspace, cx| {
                        workspace.clear(cx);
                    });
                    if backend.read(cx).current_user().is_some() {
                        ExploreView::open(&this.workspace.downgrade(), window, cx);
                    }
                }
                _ => {}
            },
        ));

        subscriptions.push(cx.observe(&settings, |_this, store, cx| {
            let settings = store.read(cx).settings().clone();
            apply_appearance(&settings, cx);
        }));

        let app = cx.entity().downgrade();
        window.defer(cx, move |_window, cx| {
            if backend.read(cx).passphrase_required() {
                log_dropped(app.update(cx, |this, cx| this.open_passphrase(cx)));
            }
        });

        Self {
            workspace,
            titlebar,
            toasts,
            sign_in,
            empty,
            passphrase: None,
            _subscriptions: subscriptions,
        }
    }

    fn open_passphrase(&mut self, cx: &mut Context<Self>) {
        self.passphrase = Some(PassphraseDialog::new(cx));
        cx.notify();
    }
}

impl Render for AppRoot {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let backend = Backend::global(cx);
        let signed_in = backend.read(cx).current_user().is_some();

        let titlebar_view = self.titlebar.read(cx);
        let workspace = self.workspace.read(cx);
        let main = workspace.active_main();

        let prefix = h_flex()
            .gap_1()
            .child(titlebar_view.render_new_repository())
            .child(titlebar_view.render_explore())
            .child(titlebar_view.repository_switcher());

        let tab_bar = h_flex()
            .gap_1()
            .when_some(titlebar_view.render_repo_tabs(workspace), |bar, tabs| {
                bar.child(tabs)
            });

        div()
            .relative()
            .size_full()
            .font_family(".SystemUIFont")
            .text_color(cx.theme().colors.foreground)
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
                Shell::new("signed-shell")
                    .titlebar(
                        Titlebar::new("signed-titlebar")
                            .prefix(prefix)
                            .tab_bar(tab_bar)
                            .when_some(titlebar_view.render_user_menu(cx), |titlebar, menu| {
                                titlebar.suffix(menu)
                            }),
                    )
                    .panel(
                        PanelRow::new("signed-panel")
                            .when_some(main.as_ref(), |row, main| {
                                row.main(main.clone()).when(workspace.has_details(), |row| {
                                    row.split(self.workspace.clone())
                                })
                            })
                            .when(!main.is_some() && signed_in, |row| {
                                row.main(self.empty.clone())
                            })
                            .when(!main.is_some() && !signed_in, |row| {
                                row.main(self.sign_in.clone())
                            }),
                    )
                    .toasts(self.toasts.clone()),
            )
            .child(titlebar_view.palette())
            .children(titlebar_view.dialogs())
            .when_some(self.passphrase.clone(), |el, view| el.child(view))
    }
}

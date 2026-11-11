use gpui::{
    App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _,
    Subscription, Window, div,
};
use gpui_native_kit::icons::IconRole;
use gpui_native_kit::layout::dock::{Dock, DockState};
use gpui_native_kit::layout::shell::{LayoutState, Shell, SidebarToggles};
use gpui_native_kit::theme::{ActiveTheme as _, ThemeVariant, follow_system, set_mode_override};
use gpui_native_kit::toast::Toast;
use settings::{AppearanceMode, Settings, SettingsStore};
use signed_state::{Backend, BackendEvent};

use crate::dialogs::{self, DialogHost};
use crate::views::placeholder::PlaceholderView;
use crate::views::sidebar::SidebarView;

pub struct AppRoot {
    layout: Entity<LayoutState>,
    dock: Entity<DockState>,
    sidebar: Entity<SidebarView>,
    dialogs: Entity<DialogHost>,
    _subscriptions: Vec<Subscription>,
}

impl AppRoot {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let dock = DockState::new(cx);
        let layout = LayoutState::new(cx);

        layout.update(cx, |layout, cx| {
            layout.set_sidebar_toggles(SidebarToggles::Hidden, cx);
        });

        dock.update(cx, |dock, cx| {
            dock.register_panel("welcome", |_, cx| {
                cx.new(|_| PlaceholderView::new("Welcome"))
            });
            dock.register_panel("profile", |_, cx| {
                cx.new(|_| PlaceholderView::new("Profile"))
            });
            dock.register_panel("relays", |_, cx| cx.new(|_| PlaceholderView::new("Relays")));
            dock.open_panel("welcome", window, cx);
        });

        let sidebar = cx.new(|cx| SidebarView::new(dock.downgrade(), cx));
        let dialogs = cx.new(DialogHost::new);
        DialogHost::set_global(dialogs.clone(), cx);

        let backend = Backend::global(cx);
        let settings = SettingsStore::global(cx);

        let mut subscriptions = Vec::new();

        subscriptions.push(
            cx.subscribe(&backend, |this, _backend, event, cx| match event {
                BackendEvent::PassphraseRequired => dialogs::passphrase::open(cx),
                BackendEvent::Error(message) => {
                    this.layout.update(cx, |layout, cx| {
                        layout.push_toast(Toast::new(message.clone()).icon(IconRole::Warning), cx);
                    });
                }
                _ => {}
            }),
        );

        subscriptions.push(cx.observe(&settings, |_this, store, cx| {
            let settings = store.read(cx).settings().clone();
            apply_appearance(&settings, cx);
        }));

        window.defer(cx, move |_window, cx| {
            if backend.read(cx).passphrase_required() {
                dialogs::passphrase::open(cx);
            }
        });

        Self {
            layout,
            dock,
            sidebar,
            dialogs,
            _subscriptions: subscriptions,
        }
    }
}

impl Render for AppRoot {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .size_full()
            .font_family(".SystemUIFont")
            .text_color(theme.colors.foreground)
            .child(
                Shell::new("signed-shell", self.layout.clone())
                    .left_sidebar(self.sidebar.read(cx).build(cx))
                    .center(
                        Dock::new("signed-dock", self.dock.clone())
                            .window_controls(true)
                            .traffic_light_pad(!self.layout.read(cx).is_left_sidebar_visible())
                            .sidebar_toggles(self.layout.clone()),
                    ),
            )
            .child(self.dialogs.clone())
    }
}

pub fn apply_appearance(settings: &Settings, cx: &mut App) {
    match settings.appearance {
        AppearanceMode::System => follow_system(cx),
        AppearanceMode::Light => set_mode_override(Some(ThemeVariant::Light), cx),
        AppearanceMode::Dark => set_mode_override(Some(ThemeVariant::Dark), cx),
    }
}

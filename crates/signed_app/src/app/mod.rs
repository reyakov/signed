pub mod helpers;
mod placeholder;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, Styled as _, Subscription, Window, div,
};
use gpui_native_kit::common::h_flex;
use gpui_native_kit::icons::IconRole;
use gpui_native_kit::layout::dock::{Dock, DockSidebar, DockState};
use gpui_native_kit::layout::shell::{LayoutState, Shell, SidebarToggles};
use gpui_native_kit::layout::window_controls::{TRAFFIC_LIGHT_PAD, titlebar_strip};
use gpui_native_kit::theme::ActiveTheme as _;
use gpui_native_kit::toast::Toast;
use helpers::apply_appearance;
use settings::SettingsStore;
use signed_state::{Backend, BackendEvent};

use crate::app::placeholder::PlaceholderView;
use crate::identity;
use crate::shared::dialogs::DialogHost;
use crate::sidebar::SidebarView;

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
            layout.set_sidebar_toggles(SidebarToggles::Tabbar, cx);
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
                BackendEvent::PassphraseRequired => identity::helpers::open_passphrase(cx),
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

        subscriptions.push(cx.observe(&dock, |this, dock, cx| {
            let Some(panel) = dock.read(cx).active_panel() else {
                return;
            };
            let is_repo = panel.starts_with("repo:") || panel.starts_with("repo-local:");

            this.layout.update(cx, |layout, cx| {
                if is_repo && !layout.is_right_sidebar_visible() {
                    layout.set_right_sidebar_visible(true, cx);
                } else if !is_repo && layout.is_right_sidebar_visible() {
                    layout.set_right_sidebar_visible(false, cx);
                }
            });
        }));

        window.defer(cx, move |_window, cx| {
            if backend.read(cx).passphrase_required() {
                identity::helpers::open_passphrase(cx);
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.theme();
        let layout = self.layout.read(cx);
        let left_visible = layout.is_left_sidebar_visible();
        let sidebar_width = layout.left_sidebar_width();

        let titlebar = if left_visible {
            self.sidebar.read(cx).render_titlebar(cx)
        } else {
            None
        };

        let titlebar = titlebar.map(|row| {
            let strip = h_flex()
                .id("signed-sidebar-titlebar")
                .size_full()
                .when(cfg!(target_os = "macos"), |strip| {
                    strip.pl(TRAFFIC_LIGHT_PAD)
                })
                .when(cfg!(not(target_os = "macos")), |strip| strip.pl_2())
                .pr_2();

            titlebar_strip(strip, "signed-sidebar-titlebar-drag", window, cx)
                .child(row)
                .into_any_element()
        });

        div()
            .relative()
            .size_full()
            .font_family(".SystemUIFont")
            .text_color(theme.colors.foreground)
            .child(
                Shell::new("signed-shell", self.layout.clone())
                    .left_sidebar(self.sidebar.read(cx).build(cx))
                    .right_sidebar(DockSidebar::new("signed-dock-sidebar", self.dock.clone()))
                    .center(
                        Dock::new("signed-dock", self.dock.clone())
                            .window_controls(true)
                            .traffic_light_pad(!left_visible)
                            .sidebar_toggles(self.layout.clone()),
                    ),
            )
            .when_some(titlebar, |root, titlebar| {
                root.child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .w(sidebar_width)
                        .h(theme.titlebar_height)
                        .child(titlebar),
                )
            })
            .child(self.dialogs.clone())
    }
}

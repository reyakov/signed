use dock::{DockArea, DockEvent, DockLayout, DockPlacement, SignedDockSkin, panel_handle};
use gpui::prelude::*;
use gpui::{Context, Entity, KeyBinding, Render, Subscription, Window, actions, div, px};
use gpui_component::{Root, StyledExt, Theme};
use gpui_fps::{FpsMonitor, FpsOverlay};
use signed_state::{Backend, BackendEvent};

use crate::views::SidebarPanel;
use crate::views::sidebar::passphrase_dialog;

actions!(workspace, [ToggleMonitor]);

pub struct Workspace {
    dock: Entity<DockArea>,
    fps: Entity<FpsMonitor>,
    /// Debug HUD, toggled with `cmd-shift-f`.
    show_fps: bool,
    _subscriptions: Vec<Subscription>,
    _passphrase_subscription: Subscription,
}

impl Workspace {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let fps = cx.new(|cx| FpsMonitor::new(window, cx).continuous(false));
        cx.bind_keys([KeyBinding::new("cmd-shift-f", ToggleMonitor, None)]);

        let dock = cx.new(|cx| {
            let skin = SignedDockSkin::new(cx);
            DockArea::new("dock", Some(1), window, cx).with_renderer(skin)
        });
        let weak_dock = dock.downgrade();

        let sidebar = cx.new(|cx| SidebarPanel::new(weak_dock.clone(), cx));
        let weak_sidebar = sidebar.downgrade();

        dock.update(cx, |dock_area, cx| {
            dock_area.set_dock(
                DockPlacement::Left,
                DockLayout::tabs().panel_view(panel_handle(sidebar), cx),
                window,
                cx,
            );
            dock_area.set_dock_size(DockPlacement::Left, px(240.), window, cx);
        });

        let mut subscriptions = vec![];

        // A bottom or right dock whose last panel was dragged away is removed entirely.
        let dock_for_pruning = dock.clone();
        subscriptions.push(cx.subscribe_in(
            &dock,
            window,
            move |_, _, event: &DockEvent, window, cx| {
                if !matches!(event, DockEvent::LayoutChanged) {
                    return;
                }
                let dock = dock_for_pruning.clone();
                cx.spawn_in(window, async move |_, window| {
                    dock.update_in(window, |area, window, cx| {
                        for placement in [DockPlacement::Bottom, DockPlacement::Right] {
                            if area.is_empty(placement, cx) {
                                area.remove_dock(placement, window, cx);
                            }
                        }
                    })
                    .ok();
                })
                .detach();
            },
        ));

        subscriptions.push(cx.observe_window_appearance(window, |_this, window, cx| {
            Theme::sync_system_appearance(Some(window), cx);
        }));

        let backend = Backend::global(cx);

        // Ask for the passphrase when the stored identity is NIP-49 encrypted.
        let passphrase_subscription =
            window.subscribe(&backend, cx, |_backend, event, window, cx| {
                if matches!(event, BackendEvent::PassphraseRequired) {
                    passphrase_dialog::open(window, cx);
                }
            });

        // The event may have fired before this window existed.
        // Fall back to the backend state in that case.
        if backend.read(cx).passphrase_required() {
            passphrase_dialog::open(window, cx);
        }

        // Open the explore panel after the sidebar has been initialized.
        cx.defer_in(window, move |_, window, cx| {
            weak_sidebar
                .update(cx, |this, cx| {
                    this.open_explore(window, cx);
                })
                .ok();
        });

        Self {
            dock,
            show_fps: cfg!(debug_assertions),
            fps,
            _subscriptions: subscriptions,
            _passphrase_subscription: passphrase_subscription,
        }
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let dialog_layer = Root::render_dialog_layer(window, cx);
        let notification_layer = Root::render_notification_layer(window, cx);

        div()
            .id("workspace")
            .on_action(
                cx.listener(|this: &mut Self, _ev: &ToggleMonitor, _window, cx| {
                    this.show_fps = !this.show_fps;
                    cx.notify();
                }),
            )
            .v_flex()
            .size_full()
            .relative()
            .child(self.dock.clone())
            // Notifications
            .children(notification_layer)
            // Modals
            .children(dialog_layer)
            // On top of everything, so it stays readable while debugging.
            .when(self.show_fps, |this| this.child(FpsOverlay::new(&self.fps)))
    }
}

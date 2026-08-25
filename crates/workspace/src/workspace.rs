use dock::{DockArea, DockEvent, DockLayout, DockPlacement, SignedDockSkin, panel_handle};
use gpui::prelude::*;
use gpui::{Context, Entity, Render, Subscription, Window, div, px};
use gpui_component::{Root, StyledExt, Theme};
use signed_state::{Backend, BackendEvent};

use crate::image_cache::{MAX_IMAGES, image_cache};
use crate::views::SidebarPanel;
use crate::views::sidebar::passphrase_dialog;

/// Root view of the app: dock area (whose center tab bar doubles as the
/// window title bar), overlays.
pub struct Workspace {
    dock: Entity<DockArea>,
    _subscriptions: Vec<Subscription>,
    _passphrase_subscription: Subscription,
}

impl Workspace {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
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

        let backend = Backend::global(cx);

        let mut subscriptions = vec![];

        // A bottom/right dock whose last panel was dragged away is removed
        // entirely: base keeps the emptied region, which would otherwise
        // linger as a bare strip. Deferred, because the event arrives while
        // the area is mid-update.
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

        // Ask for the passphrase when the stored identity is NIP-49
        // encrypted. Subscribed via the window, since opening a dialog
        // needs one.
        let passphrase_subscription =
            window.subscribe(&backend, cx, |_backend, event, window, cx| {
                if matches!(event, BackendEvent::PassphraseRequired) {
                    passphrase_dialog::open(window, cx);
                }
            });

        // The event may have fired before this window existed (the backend
        // is initialized before the first window opens); fall back to the
        // backend state in that case.
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
            .image_cache(image_cache("workspace", MAX_IMAGES))
            .id("workspace")
            .v_flex()
            .size_full()
            .child(self.dock.clone())
            // Notifications
            .children(notification_layer)
            // Modals
            .children(dialog_layer)
    }
}

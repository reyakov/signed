use dock::{DockArea, DockEvent, DockLayout, DockPlacement, SignedDockSkin, panel_handle};
use gpui::prelude::*;
use gpui::{Context, Entity, Render, Subscription, Window, div, px};
use gpui_component::{Root, StyledExt, Theme};
use settings::{AppearanceMode, SettingsStore};
use signed_state::{Backend, BackendEvent};

use crate::views::SidebarPanel;
use crate::views::sidebar::passphrase_dialog;

pub struct Workspace {
    dock: Entity<DockArea>,
    _subscriptions: Vec<Subscription>,
}

impl Workspace {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let backend = Backend::global(cx);
        let settings = SettingsStore::global(cx);

        let dock = cx.new(|cx| {
            let skin = SignedDockSkin::new(cx);
            DockArea::new("dock", Some(1), window, cx).with_renderer(skin)
        });
        let weak_dock = dock.downgrade();

        let sidebar = cx.new(|cx| SidebarPanel::new(weak_dock.clone(), cx));
        let weak_sidebar = sidebar.downgrade();

        let mut subscriptions = vec![];

        if settings.read(cx).settings().appearance == AppearanceMode::System {
            subscriptions.push(cx.observe_window_appearance(window, |_this, window, cx| {
                Theme::sync_system_appearance(Some(window), cx);
            }));
        }

        // A bottom or right dock whose last panel was dragged away is removed entirely.
        subscriptions.push(cx.subscribe_in(
            &dock,
            window,
            move |_, _, event: &DockEvent, window, cx| {
                if !matches!(event, DockEvent::LayoutChanged) {
                    return;
                }
                let weak = weak_dock.clone();
                cx.spawn_in(window, async move |_, window| {
                    weak.update_in(window, |area, window, cx| {
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

        // Ask for the passphrase when the stored identity is NIP-49 encrypted.
        subscriptions.push(cx.subscribe_in(
            &backend,
            window,
            |_this, _state, event, window, cx| {
                if matches!(event, BackendEvent::PassphraseRequired) {
                    passphrase_dialog::open(window, cx);
                }
            },
        ));

        cx.defer_in(window, move |this, window, cx| {
            // The event may have fired before this window existed.
            // Fall back to the backend state in that case.
            if backend.read(cx).passphrase_required() {
                passphrase_dialog::open(window, cx);
            }

            this.dock.update(cx, |dock_area, cx| {
                dock_area.set_dock(
                    DockPlacement::Left,
                    DockLayout::tabs().panel_view(panel_handle(sidebar), cx),
                    window,
                    cx,
                );
                dock_area.set_dock_size(DockPlacement::Left, px(240.), window, cx);
            });

            weak_sidebar
                .update(cx, |this, cx| {
                    this.open_explore(window, cx);
                })
                .ok();
        });

        Self {
            dock,
            _subscriptions: subscriptions,
        }
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let dialog_layer = Root::render_dialog_layer(window, cx);
        let notification_layer = Root::render_notification_layer(window, cx);

        div()
            .id("workspace")
            .v_flex()
            .size_full()
            .relative()
            .child(self.dock.clone())
            .children(notification_layer)
            .children(dialog_layer)
    }
}

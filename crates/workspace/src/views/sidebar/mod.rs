use assets::CustomIconName;
use dock::{
    BasePanel, DockArea, DockPlacement, Panel, PanelEvent, TAB_BAR_HEIGHT, panel_handle,
    title_bar_drag_handlers,
};
use gpui::prelude::*;
use gpui::{
    App, ClickEvent, Context, ElementId, EventEmitter, FocusHandle, Focusable, Render,
    SharedString, StyleRefinement, Subscription, WeakEntity, Window, div, px,
};
use gpui_component::avatar::Avatar;
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::InputState;
use gpui_component::{ActiveTheme, Icon, IconName, Sizable, StyledExt, h_flex, v_flex};
use signed_state::{Backend, BackendEvent, Profile, ProfileStore};

use super::RepoListView;
use crate::image_cache::{MAX_IMAGES, image_cache};

mod import_dialog;
mod onboarding_dialog;
pub(crate) mod passphrase_dialog;

use self::onboarding_dialog::OnboardingState;

/// Left-dock panel with navigation entries. Entries open content panels in
/// the dock area.
pub struct SidebarPanel {
    focus_handle: FocusHandle,
    dock_area: WeakEntity<DockArea>,
    explore: Option<WeakEntity<RepoListView>>,
    logged_in: bool,
    _subscription: Subscription,
}

impl SidebarPanel {
    pub fn new(dock_area: WeakEntity<DockArea>, cx: &mut Context<Self>) -> Self {
        let backend = Backend::global(cx);
        let logged_in = backend.read(cx).current_user().is_some();

        let subscription = cx.subscribe(&backend, |this, backend, event, cx| {
            match event {
                BackendEvent::SignerChanged => {
                    this.logged_in = backend.read(cx).current_user().is_some();
                }
                BackendEvent::SignerRequired => {
                    this.logged_in = false;
                }
                _ => return,
            }
            cx.notify();
        });

        Self {
            focus_handle: cx.focus_handle(),
            dock_area,
            explore: None,
            logged_in,
            _subscription: subscription,
        }
    }

    /// Open the Explore (repository list) panel in the center of the dock
    /// area. No-op if it's already open.
    pub fn open_explore(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .explore
            .as_ref()
            .and_then(WeakEntity::upgrade)
            .is_some()
        {
            return;
        }

        let panel = cx.new(|cx| RepoListView::new(self.dock_area.clone(), window, cx));
        self.explore = Some(panel.downgrade());

        let _ = self.dock_area.update(cx, |dock_area, cx| {
            dock_area.add_panel_view(panel_handle(panel), DockPlacement::Center, None, window, cx);
        });
    }

    /// Show the Onboarding dialog.
    fn open_onboarding(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name_input = cx.new(|cx| InputState::new(window, cx).placeholder("Enter desired name"));
        let pass_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Passphrase to protect your keys")
                .masked(true)
        });
        let repass_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Repeat passphrase")
                .masked(true)
        });
        let state = cx.new(|_| OnboardingState::default());

        onboarding_dialog::open(name_input, pass_input, repass_input, state, window, cx);
    }

    /// Show the Import Identity dialog.
    fn open_import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        import_dialog::open(window, cx);
    }

    /// Render the user avatar and name in the sidebar, wrapped in the window titlebar drag area.
    fn render_user(
        &self,
        profile: &Profile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let name = profile.name();
        let picture = profile.picture();

        title_bar_drag_handlers(
            h_flex()
                .id("user")
                .h(TAB_BAR_HEIGHT)
                .when(cfg!(target_os = "macos"), |this| this.pl(px(80.)))
                .child(
                    div().child(
                        Button::new("user").text().dropdown_caret(true).child(
                            h_flex()
                                .gap_1()
                                .child(
                                    Avatar::new()
                                        .name(name.clone())
                                        .when_some(picture, |this, url| this.src(url))
                                        .rounded(cx.theme().radius)
                                        .small(),
                                )
                                .child(div().text_xs().font_semibold().child(name)),
                        ),
                    ),
                ),
            window,
            cx,
        )
    }
}

impl BasePanel for SidebarPanel {
    fn panel_name(&self) -> &'static str {
        "sidebar"
    }

    fn closable(&self, _cx: &App) -> bool {
        false
    }
}

impl Panel for SidebarPanel {
    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

impl EventEmitter<PanelEvent> for SidebarPanel {}

impl Focusable for SidebarPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for SidebarPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.logged_in {
            return v_flex()
                .p_4()
                .size_full()
                .items_center()
                .justify_center()
                .gap_2()
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("Sign in to continue"),
                )
                .child(
                    Button::new("onboarding")
                        .label("Join now")
                        .primary()
                        .w_full()
                        .on_click(
                            cx.listener(|this, _ev, window, cx| this.open_onboarding(window, cx)),
                        ),
                )
                .child(
                    Button::new("import-identity")
                        .label("Import identity")
                        .secondary()
                        .w_full()
                        .on_click(
                            cx.listener(|this, _ev, window, cx| this.open_import(window, cx)),
                        ),
                );
        }

        let backend = Backend::global(cx);
        let profile_store = ProfileStore::global(cx);

        let profile = backend
            .read(cx)
            .current_user()
            .map(|public_key| profile_store.read(cx).get(&public_key));

        v_flex()
            .size_full()
            .justify_between()
            .image_cache(image_cache("sidebar", MAX_IMAGES))
            .bg(cx.theme().sidebar)
            .text_color(cx.theme().sidebar_foreground)
            .child(
                div()
                    .flex_1()
                    .when_some(profile.as_ref(), |this, profile| {
                        this.child(self.render_user(profile, window, cx))
                    })
                    .child(
                        v_flex()
                            .px_2()
                            .gap_1()
                            .items_start()
                            .justify_start()
                            .child(NavItem::new("inbox", "Inbox", IconName::Inbox).on_click(
                                cx.listener(|this, _ev, window, cx| this.open_explore(window, cx)),
                            ))
                            .child(NavItem::new("explore", "Browse", IconName::Globe).on_click(
                                cx.listener(|this, _ev, window, cx| this.open_explore(window, cx)),
                            ))
                            .child(NavItem::new("search", "Search", IconName::Search).on_click(
                                cx.listener(|this, _ev, window, cx| this.open_explore(window, cx)),
                            ))
                            .child(
                                v_flex().w_full().child(
                                    h_flex()
                                        .h_10()
                                        .w_full()
                                        .justify_between()
                                        .items_center()
                                        .child(
                                            h_flex()
                                                .px_2()
                                                .gap_2()
                                                .text_color(cx.theme().muted_foreground)
                                                .child(Icon::new(CustomIconName::Filter).small())
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .font_semibold()
                                                        .child("All Repositories"),
                                                ),
                                        )
                                        .child(
                                            Button::new("add").icon(IconName::Plus).small().ghost(),
                                        ),
                                ),
                            ),
                    ),
            )
            .child(
                v_flex()
                    .p_2()
                    .flex_shrink_0()
                    .gap_1()
                    .items_start()
                    .justify_start()
                    .child(NavItem::new("guide", "Guide", IconName::Info).on_click(
                        cx.listener(|this, _ev, window, cx| this.open_explore(window, cx)),
                    ))
                    .child(
                        NavItem::new("settings", "Settings", IconName::Settings).on_click(
                            cx.listener(|this, _ev, window, cx| this.open_explore(window, cx)),
                        ),
                    ),
            )
    }
}

/// A single navigation entry in the sidebar: an icon and label with a hover
/// highlight and an optional click handler.
#[allow(clippy::type_complexity)]
#[derive(IntoElement)]
struct NavItem {
    id: ElementId,
    style: StyleRefinement,
    icon: IconName,
    label: SharedString,
    on_click: Option<Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>>,
}

impl NavItem {
    fn new<I, L>(id: I, label: L, icon: IconName) -> Self
    where
        I: Into<ElementId>,
        L: Into<SharedString>,
    {
        Self {
            id: id.into(),
            icon,
            label: label.into(),
            style: StyleRefinement::default(),
            on_click: None,
        }
    }

    fn on_click(mut self, listener: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Box::new(listener));
        self
    }
}

impl RenderOnce for NavItem {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        h_flex()
            .id(self.id)
            .refine_style(&self.style)
            .px_2()
            .py_1()
            .w_full()
            .gap_2()
            .rounded(cx.theme().radius)
            .child(Icon::new(self.icon).small())
            .child(div().text_sm().child(self.label))
            .hover(|this| this.bg(cx.theme().list_hover))
            .when_some(self.on_click, |this, listener| this.on_click(listener))
    }
}

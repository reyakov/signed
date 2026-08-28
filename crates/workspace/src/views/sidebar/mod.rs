use std::ops::Range;

use assets::CustomIconName;
use dock::{
    BasePanel, DockArea, DockPlacement, Panel, PanelEvent, TAB_BAR_HEIGHT, panel_handle,
    title_bar_drag_handlers,
};
use gpui::prelude::*;
use gpui::{
    AnyElement, App, ClickEvent, Context, ElementId, Entity, EventEmitter, FocusHandle, Focusable,
    Render, SharedString, StyleRefinement, Subscription, WeakEntity, Window, div, px, uniform_list,
};
use gpui_component::avatar::Avatar;
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::InputState;
use gpui_component::{ActiveTheme, Icon, IconName, Sizable, StyledExt, h_flex, v_flex};
use signed_core::Announcement;
use signed_state::{Backend, BackendEvent, Profile, ProfileStore, RepoListStore};

use super::{RepoDetailView, RepoListView};
use crate::image_cache::{MAX_IMAGES, image_cache};
use crate::pixel_avatar::PixelAvatar;

mod create_repo_dialog;
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
    /// Repositories announced by the current user, listed under
    /// "All Repositories". Recreated when the signer changes.
    my_repos: Option<Entity<RepoListStore>>,
    /// Observes the current user's repo store so the list re-renders.
    my_repos_subscription: Option<Subscription>,
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
                    this.refresh_my_repos(cx);
                }
                BackendEvent::SignerRequired => {
                    this.logged_in = false;
                    this.my_repos = None;
                    this.my_repos_subscription = None;
                }
                _ => return,
            }
            cx.notify();
        });

        let mut panel = Self {
            focus_handle: cx.focus_handle(),
            dock_area,
            explore: None,
            my_repos: None,
            my_repos_subscription: None,
            logged_in,
            _subscription: subscription,
        };

        if logged_in {
            panel.refresh_my_repos(cx);
        }

        panel
    }

    /// (Re)create the store listing the current user's repositories.
    fn refresh_my_repos(&mut self, cx: &mut Context<Self>) {
        self.my_repos_subscription = None;

        let author = Backend::global(cx).read(cx).current_user();
        self.my_repos = author.map(|author| cx.new(|cx| RepoListStore::new(Some(author), cx)));

        if let Some(store) = self.my_repos.as_ref() {
            self.my_repos_subscription = Some(cx.observe(store, |_, _, cx| cx.notify()));
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

    /// Show the Create Repository dialog.
    fn open_create_repo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        create_repo_dialog::open(self.dock_area.clone(), window, cx);
    }

    /// Open a repository's detail view in the dock's center.
    fn open_repo(
        &mut self,
        announcement: &Announcement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let detail = cx.new(|cx| {
            RepoDetailView::new(self.dock_area.clone(), announcement.clone(), window, cx)
        });

        let _ = self.dock_area.update(cx, |dock_area, cx| {
            dock_area.add_panel_view(
                panel_handle(detail),
                DockPlacement::Center,
                None,
                window,
                cx,
            );
        });
    }

    /// The "All Repositories" section: header with the create button and
    /// the current user's repositories below it, lazily rendered through a
    /// [`uniform_list`].
    fn render_my_repos(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let store = self.my_repos.as_ref();

        v_flex()
            .px_2()
            .flex_1()
            .min_h_0()
            .w_full()
            .child(
                h_flex()
                    .h_10()
                    .w_full()
                    .flex_shrink_0()
                    .justify_between()
                    .items_center()
                    .child(
                        h_flex()
                            .px_2()
                            .gap_2()
                            .text_color(cx.theme().muted_foreground)
                            .child(Icon::new(CustomIconName::Filter).small())
                            .child(div().text_xs().font_semibold().child("All Repositories")),
                    )
                    .child(
                        Button::new("add")
                            .icon(IconName::Plus)
                            .small()
                            .ghost()
                            .on_click(cx.listener(|this, _ev, window, cx| {
                                this.open_create_repo(window, cx);
                            })),
                    ),
            )
            .when_some(store, |builder, store| {
                let announcements = store.read(cx).announcements.clone();

                if announcements.is_empty() {
                    builder.child(
                        div()
                            .flex_1()
                            .px_2()
                            .py_1()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("No repositories yet"),
                    )
                } else {
                    builder.child(
                        uniform_list(
                            "my-repos-list",
                            announcements.len(),
                            cx.processor(move |this, range: Range<usize>, _window, cx| {
                                range
                                    .map(|ix| {
                                        this.render_repo_row(&announcements[ix], cx)
                                            .into_any_element()
                                    })
                                    .collect()
                            }),
                        )
                        .flex_1()
                        .min_h_0(),
                    )
                }
            })
    }

    /// One repository row in the sidebar, styled like the nav items: a
    /// deterministic pixel avatar and the repo name.
    fn render_repo_row(
        &self,
        announcement: &Announcement,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let name = announcement
            .name
            .clone()
            .unwrap_or_else(|| SharedString::from(announcement.id.clone()));
        let avatar = PixelAvatar::new(format!("{}:{}", announcement.owner, announcement.id));
        let announcement = announcement.clone();

        NavItem::new(format!("my-repo:{}", announcement.id), name, avatar).on_click(
            cx.listener(move |this, _ev, window, cx| this.open_repo(&announcement, window, cx)),
        )
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
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .when_some(profile.as_ref(), |this, profile| {
                        this.child(self.render_user(profile, window, cx))
                    })
                    .child(
                        v_flex()
                            .px_2()
                            .gap_1()
                            .items_start()
                            .justify_start()
                            .child(
                                NavItem::new("inbox", "Inbox", Icon::new(IconName::Inbox).small())
                                    .on_click(cx.listener(|this, _ev, window, cx| {
                                        this.open_explore(window, cx)
                                    })),
                            )
                            .child(
                                NavItem::new(
                                    "explore",
                                    "Browse",
                                    Icon::new(IconName::Globe).small(),
                                )
                                .on_click(cx.listener(
                                    |this, _ev, window, cx| this.open_explore(window, cx),
                                )),
                            )
                            .child(
                                NavItem::new(
                                    "search",
                                    "Search",
                                    Icon::new(IconName::Search).small(),
                                )
                                .on_click(cx.listener(
                                    |this, _ev, window, cx| this.open_explore(window, cx),
                                )),
                            ),
                    )
                    .child(self.render_my_repos(cx)),
            )
            .child(
                v_flex()
                    .p_2()
                    .flex_shrink_0()
                    .gap_1()
                    .items_start()
                    .justify_start()
                    .child(
                        NavItem::new("guide", "Guide", Icon::new(IconName::Info).small()).on_click(
                            cx.listener(|this, _ev, window, cx| this.open_explore(window, cx)),
                        ),
                    )
                    .child(
                        NavItem::new(
                            "settings",
                            "Settings",
                            Icon::new(IconName::Settings).small(),
                        )
                        .on_click(
                            cx.listener(|this, _ev, window, cx| this.open_explore(window, cx)),
                        ),
                    ),
            )
    }
}

/// A single navigation entry in the sidebar: an arbitrary leading element
/// (an icon, avatar, ...) and a text label with a hover highlight and an
/// optional click handler.
#[allow(clippy::type_complexity)]
#[derive(IntoElement)]
struct NavItem {
    id: ElementId,
    style: StyleRefinement,
    icon: AnyElement,
    label: SharedString,
    on_click: Option<Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>>,
}

impl NavItem {
    fn new<I, L, N>(id: I, label: L, icon: N) -> Self
    where
        I: Into<ElementId>,
        L: Into<SharedString>,
        N: IntoElement,
    {
        Self {
            id: id.into(),
            icon: icon.into_any_element(),
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
            .child(self.icon)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_sm()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(self.label),
            )
            .hover(|this| this.bg(cx.theme().list_hover))
            .when_some(self.on_click, |this, listener| this.on_click(listener))
    }
}

use std::collections::HashSet;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use assets::CustomIconName;
use dock::{
    BasePanel, DockArea, DockPlacement, Panel, PanelEvent, TAB_BAR_HEIGHT, panel_handle,
    title_bar_drag_handlers,
};
use gpui::prelude::*;
use gpui::{
    AnyElement, App, ClickEvent, Context, Div, ElementId, Entity, EventEmitter, FocusHandle,
    Focusable, ObjectFit, Render, SharedString, StyleRefinement, Subscription, WeakEntity, Window,
    div, img, px, uniform_list,
};
use gpui_base::Button as BaseButton;
use gpui_component::avatar::Avatar;
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::InputState;
use gpui_component::{ActiveTheme, Icon, IconName, Sizable, StyledExt, h_flex, v_flex};
use signed_core::{Announcement, identifier_from_name};
use signed_state::{Backend, BackendEvent, LocalReposStore, Profile, ProfileStore, RepoListStore};

use super::{RepoDetailView, RepoListView};
use crate::image_cache::{MAX_IMAGES, image_cache};
use crate::pixel_avatar::PixelAvatar;

mod create_repo_dialog;
pub(crate) mod grasp_servers;
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
    /// Repositories announced by the current user, listed under
    /// "All Repositories". Recreated when the signer changes.
    my_repos: Option<Entity<RepoListStore>>,
    /// Observes the current user's repo store so the list re-renders.
    my_repos_subscription: Option<Subscription>,
    /// Banner artwork shown behind the sign-in screen,
    /// picked at random from the bundled `backgrounds/` assets.
    banner: SharedString,
    /// Observes the local-repository scan so new discoveries re-render.
    _local_repos_subscription: Subscription,
    _subscription: Subscription,
}

impl SidebarPanel {
    pub fn new(dock_area: WeakEntity<DockArea>, cx: &mut Context<Self>) -> Self {
        let local_repos_store = LocalReposStore::global(cx);
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
                    this.banner = pick_banner();
                    this.my_repos = None;
                    this.my_repos_subscription = None;
                }
                _ => return,
            }
            cx.notify();
        });

        let local_repos_subscription = cx.observe(&local_repos_store, |_, _, cx| {
            cx.notify();
        });

        let mut panel = Self {
            focus_handle: cx.focus_handle(),
            dock_area,
            logged_in,
            explore: None,
            my_repos: None,
            my_repos_subscription: None,
            banner: pick_banner(),
            _local_repos_subscription: local_repos_subscription,
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

    /// Open a local repository's detail view in the dock's center; the
    /// detail view offers to publish it to NIP-34.
    fn open_local_repo(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let detail =
            cx.new(|cx| RepoDetailView::new_local(self.dock_area.clone(), path, window, cx));

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
    /// [`uniform_list`], followed by the local git repositories discovered
    /// by the startup scan.
    fn render_my_repos(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let store = self.my_repos.as_ref();
        let local = LocalReposStore::global(cx);
        let local_repos = local.read(cx).repos.clone();
        let scanning = local.read(cx).scanning;

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
                        h_flex()
                            .gap_1()
                            .child(
                                Button::new("rescan")
                                    .icon(CustomIconName::Refresh)
                                    .small()
                                    .ghost()
                                    .tooltip("Rescan for local repositories")
                                    .on_click(cx.listener(|_this, _ev, _window, cx| {
                                        let local_repos = LocalReposStore::global(cx);
                                        local_repos.update(cx, |store, cx| store.rescan(cx));
                                    })),
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
                    ),
            )
            .when_some(store, |builder, store| {
                let announcements = store.read(cx).announcements.clone();
                // Local repositories that have already been published to
                // NIP-34 are listed among the user's repositories above;
                // hide them from the local section (matched by the
                // identifier derived from the directory name, like the
                // init dialog's default name).
                let announced_ids: HashSet<String> =
                    announcements.iter().map(|a| a.id.clone()).collect();
                let local_repos: Vec<PathBuf> = local_repos
                    .iter()
                    .filter(|path| {
                        let Some(name) = path.file_name() else {
                            return true;
                        };
                        !announced_ids.contains(&identifier_from_name(&name.to_string_lossy()))
                    })
                    .cloned()
                    .collect();
                // One merged list: the user's NIP-34 repositories first,
                // then the local repositories discovered by the scan.
                let total = announcements.len() + local_repos.len();

                if total == 0 {
                    builder.child(
                        div()
                            .flex_1()
                            .px_2()
                            .py_1()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(if scanning {
                                "Scanning for local repositories…"
                            } else {
                                "No repositories yet"
                            }),
                    )
                } else {
                    builder.child(
                        uniform_list(
                            "repos",
                            total,
                            cx.processor(move |this, range: Range<usize>, _window, cx| {
                                range
                                    .map(|ix| {
                                        this.render_repo_row_at(
                                            &announcements,
                                            &local_repos,
                                            ix,
                                            cx,
                                        )
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

    /// One row of the merged sidebar list: a NIP-34 repository or a local
    /// repository.
    fn render_repo_row_at(
        &self,
        announcements: &[Announcement],
        local_repos: &[PathBuf],
        ix: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if ix < announcements.len() {
            return self
                .render_repo_row(&announcements[ix], cx)
                .into_any_element();
        }

        let local_ix = ix - announcements.len();
        let path = &local_repos[local_ix];

        self.render_local_row(path, cx).into_any_element()
    }

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

    /// One local repository row: a deterministic pixel avatar seeded from
    /// the path, the directory name, and a warning suffix marking it as
    /// not yet set up for NIP-34. Clicking it opens the repository's
    /// detail view, which offers to initialize it.
    fn render_local_row(&self, path: &Path, cx: &mut Context<Self>) -> impl IntoElement {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        let path = path.to_path_buf();

        NavItem::new(
            format!("local-repo:{}", path.display()),
            name,
            PixelAvatar::new(path.to_string_lossy()),
        )
        .suffix(
            Icon::new(IconName::TriangleAlert)
                .small()
                .text_color(cx.theme().warning),
        )
        .on_click(cx.listener(move |this, _ev, window, cx| {
            this.open_local_repo(path.clone(), window, cx);
        }))
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

    /// Sign-in placeholder shown while logged out: the banner artwork fills the
    /// panel behind a scrim that ends in a solid black band, keeping the CTA
    /// buttons readable on a clean dark surface in both themes.
    fn render_sign_in(&self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        v_flex()
            .size_full()
            .relative()
            .bg(cx.theme().sidebar)
            .text_color(cx.theme().sidebar_foreground)
            .child(title_bar_drag_handlers(
                div()
                    .id("onboarding-drag")
                    .absolute()
                    .h_12()
                    .w_full()
                    .top_0()
                    .left_0(),
                window,
                cx,
            ))
            .child(
                div().absolute().inset_0().child(
                    img(self.banner.clone())
                        .size_full()
                        .object_fit(ObjectFit::Cover),
                ),
            )
            .child(
                v_flex()
                    .size_full()
                    .justify_end()
                    .p_4()
                    .mb_4()
                    .gap_4()
                    .child(img("backgrounds/headline.png").max_w_48())
                    .child(
                        v_flex()
                            .gap_1()
                            .w_full()
                            .child(
                                BaseButton::new("onboarding")
                                    .h_flex()
                                    .h_8()
                                    .px_2()
                                    .bg(cx.theme().primary)
                                    .hover(|this| this.bg(cx.theme().primary_hover))
                                    .active(|this| this.bg(cx.theme().primary_active))
                                    .text_color(cx.theme().primary_foreground)
                                    .child(div().text_sm().font_semibold().child("Join now"))
                                    .on_click(cx.listener(|this, _ev, window, cx| {
                                        this.open_onboarding(window, cx)
                                    })),
                            )
                            .child(
                                BaseButton::new("onboarding")
                                    .h_flex()
                                    .h_8()
                                    .px_2()
                                    .text_color(gpui::white())
                                    .bg(gpui::white().opacity(0.1))
                                    .hover(|this| this.bg(gpui::white().opacity(0.2)))
                                    .active(|this| this.bg(gpui::white().opacity(0.4)))
                                    .child(div().text_sm().child("Import identity"))
                                    .on_click(cx.listener(|this, _ev, window, cx| {
                                        this.open_import(window, cx)
                                    })),
                            ),
                    ),
            )
    }
}

fn pick_banner() -> SharedString {
    let num = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before unix epoch")
        .subsec_nanos()
        % 3
        + 1;
    format!("backgrounds/banner{num}.jpg").into()
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
            return self.render_sign_in(window, cx);
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
/// (an icon, avatar, ...) and a text label with a hover highlight,
/// an optional trailing suffix (e.g. a status icon) and an optional click handler.
#[allow(clippy::type_complexity)]
#[derive(IntoElement)]
struct NavItem {
    id: ElementId,
    style: StyleRefinement,
    icon: AnyElement,
    label: SharedString,
    /// Trailing element rendered at the right edge of the row, after the (ellipsized) label.
    suffix: Option<AnyElement>,
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
            suffix: None,
            on_click: None,
        }
    }

    /// A trailing element rendered at the right edge of the row
    fn suffix(mut self, suffix: impl IntoElement) -> Self {
        self.suffix = Some(suffix.into_any_element());
        self
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
            .when_some(self.suffix, |this, suffix| {
                this.child(div().flex_shrink_0().child(suffix))
            })
            .hover(|this| this.bg(cx.theme().list_hover))
            .when_some(self.on_click, |this, listener| this.on_click(listener))
    }
}

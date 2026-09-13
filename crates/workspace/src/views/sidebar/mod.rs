use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use assets::CustomIconName;
use dock::{
    BasePanel, DockArea, Panel, PanelEvent, TAB_BAR_HEIGHT, add_center_panel, panel_handle,
};
use gpui::prelude::*;
use gpui::{
    AnyElement, App, Context, Div, EventEmitter, FocusHandle, Focusable, ObjectFit, Render,
    SharedString, Subscription, WeakEntity, Window, div, img, px, relative, uniform_list, white,
};
use gpui_base::Button as BaseButton;
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::InputState;
use gpui_component::{ActiveTheme, Icon, IconName, Sizable, StyledExt, h_flex, v_flex};
use signed_core::{Announcement, RepoAddr, identifier_from_name};
use signed_state::{
    Backend, BackendEvent, CheckoutsStore, LocalReposStore, Profile, ProfileStore, RepoListStore,
};
use signed_ui::{NavItem, PixelAvatar, UserAvatar, title_bar_drag_handlers};

use super::{InboxView, RepoDetailView, RepoListView, open_repo_panel};

mod create_repo_dialog;
pub(crate) mod grasp_servers;
mod import_dialog;
mod onboarding_dialog;
pub(crate) mod passphrase_dialog;
mod settings_dialog;

use self::onboarding_dialog::OnboardingState;

pub struct SidebarPanel {
    focus_handle: FocusHandle,
    dock_area: WeakEntity<DockArea>,
    inbox: Option<WeakEntity<InboxView>>,
    explore: Option<WeakEntity<RepoListView>>,
    banner: SharedString,
    /// The signed-in user's announced repositories, newest first.
    announcements: Arc<Vec<Announcement>>,
    /// Local repositories found by the scan that are not announced yet.
    local_repos: Arc<Vec<PathBuf>>,
    scanning: bool,
    /// Unpushed commit counts per announced repository, shown as row badges.
    unpushed: HashMap<RepoAddr, usize>,
    _subscriptions: Vec<Subscription>,
}

impl SidebarPanel {
    pub fn new(dock_area: WeakEntity<DockArea>, cx: &mut Context<Self>) -> Self {
        let backend = Backend::global(cx);
        let repos = RepoListStore::global(cx);
        let local = LocalReposStore::global(cx);
        let checkouts = CheckoutsStore::global(cx);

        let mut subscriptions = Vec::new();

        // Identity changes swap the whole sidebar between the sign-in screen and the signed-in content.
        subscriptions.push(cx.subscribe(&backend, |this, _backend, event, cx| {
            let signer_changed = matches!(event, BackendEvent::SignerChanged);
            let signer_required = matches!(event, BackendEvent::SignerRequired);

            if !signer_changed && !signer_required {
                return;
            }

            if signer_required {
                this.banner = pick_banner();
                cx.notify();
            }

            if this.refresh(cx) || signer_required {
                cx.notify();
            }
        }));

        subscriptions.push(cx.observe(&repos, |this, _repos, cx| {
            if this.refresh(cx) {
                cx.notify();
            }
        }));

        subscriptions.push(cx.observe(&local, |this, _local, cx| {
            if this.refresh(cx) {
                cx.notify();
            }
        }));

        // Push statuses are recomputed in the background, so only the badge counts change.
        subscriptions.push(cx.observe(&checkouts, |this, _checkouts, cx| {
            if this.refresh_unpushed(cx) {
                cx.notify();
            }
        }));

        Self {
            focus_handle: cx.focus_handle(),
            dock_area,
            inbox: None,
            explore: None,
            banner: pick_banner(),
            announcements: Arc::new(Vec::new()),
            local_repos: Arc::new(Vec::new()),
            scanning: false,
            unpushed: HashMap::new(),
            _subscriptions: subscriptions,
        }
    }

    fn refresh(&mut self, cx: &mut Context<Self>) -> bool {
        let backend = Backend::global(cx);
        let user = backend.read(cx).current_user();

        let repo_list = RepoListStore::global(cx);
        let announcements = user
            .as_ref()
            .map(|user| repo_list.read(cx).announcements_of(user))
            .unwrap_or_default();

        // Drop a scanned repository once the user announces it, so it is not listed twice.
        let local = LocalReposStore::global(cx);
        let scanning = local.read(cx).scanning;

        let local_repos = {
            let ids: HashSet<String> = announcements.iter().map(|a| a.id.clone()).collect();
            local
                .read(cx)
                .repos
                .iter()
                .filter(|path| {
                    let Some(name) = path.file_name() else {
                        return true;
                    };
                    !ids.contains(&identifier_from_name(&name.to_string_lossy()))
                })
                .cloned()
                .collect()
        };

        let announcements_changed = *self.announcements != announcements;
        let local_changed = *self.local_repos != local_repos;
        let scanning_changed = self.scanning != scanning;

        self.announcements = Arc::new(announcements);
        self.local_repos = Arc::new(local_repos);
        self.scanning = scanning;

        if announcements_changed {
            self.request_push_watches(cx);
            self.unpushed.clear();
        }

        announcements_changed || local_changed || scanning_changed
    }

    fn refresh_unpushed(&mut self, cx: &mut Context<Self>) -> bool {
        let checkouts = CheckoutsStore::global(cx);
        let mut unpushed = HashMap::with_capacity(self.announcements.len());

        for announcement in self.announcements.iter() {
            let addr = announcement.addr();
            let count = checkouts.read(cx).unpushed(&addr);
            if count > 0 {
                unpushed.insert(addr, count);
            }
        }

        if unpushed == self.unpushed {
            return false;
        }

        self.unpushed = unpushed;
        true
    }

    fn request_push_watches(&self, cx: &mut Context<Self>) {
        let checkouts = CheckoutsStore::global(cx);
        checkouts.update(cx, |checkouts, cx| {
            for announcement in self.announcements.iter() {
                checkouts.request_push_statuses(&announcement.addr(), cx);
            }
        });
    }

    pub fn open_inbox(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.inbox.as_ref().and_then(WeakEntity::upgrade).is_some() {
            return;
        }

        let panel = cx.new(|cx| InboxView::new(self.dock_area.clone(), cx));
        self.inbox = Some(panel.downgrade());

        self.dock_area
            .update(cx, |dock_area, cx| {
                add_center_panel(dock_area, panel_handle(panel), window, cx);
            })
            .ok();
    }

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

        self.dock_area
            .update(cx, |dock_area, cx| {
                add_center_panel(dock_area, panel_handle(panel), window, cx);
            })
            .ok();
    }

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

    fn open_create_repo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        create_repo_dialog::open(self.dock_area.clone(), window, cx);
    }

    fn open_repo(
        &mut self,
        announcement: &Announcement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        open_repo_panel(
            &self.dock_area,
            &announcement.addr(),
            Some(announcement),
            window,
            &mut *cx,
        );
    }

    /// The detail view offers to publish it to NIP-34.
    fn open_local_repo(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let detail =
            cx.new(|cx| RepoDetailView::new_local(self.dock_area.clone(), path, window, cx));

        self.dock_area
            .update(cx, |dock_area, cx| {
                add_center_panel(dock_area, panel_handle(detail), window, cx);
            })
            .ok();
    }

    fn render_repos(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let announcements = self.announcements.clone();
        let local_repos = self.local_repos.clone();
        let scanning = self.scanning;

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
            .map(|this| {
                // The merged list: NIP-34 repositories first, then discovered local repositories.
                let total = announcements.len() + local_repos.len();

                if total == 0 {
                    this.child(
                        div()
                            .flex_1()
                            .px_2()
                            .py_1()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .map(|this| {
                                if scanning {
                                    this.child("Scanning for local repositories…")
                                } else {
                                    this.child("No repositories yet")
                                }
                            }),
                    )
                } else {
                    this.child(
                        uniform_list(
                            "repos",
                            total,
                            cx.processor(move |this, range: Range<usize>, _, cx| {
                                range
                                    .map(|ix| {
                                        this.render_repo_at(&announcements, &local_repos, ix, cx)
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

    /// Renders row `ix` of the merged list: an announced repository or a local one.
    fn render_repo_at(
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
        let name = announcement.name().map(SharedString::from);
        let avatar = PixelAvatar::new(format!("{}:{}", announcement.owner, announcement.id));
        let announcement = announcement.clone();

        let unpushed = self
            .unpushed
            .get(&announcement.addr())
            .copied()
            .unwrap_or(0);

        let mut row = NavItem::new(format!("repo:{}", announcement.id), name, avatar);

        if unpushed > 0 {
            row = row.suffix(
                v_flex()
                    .flex_shrink_0()
                    .size_4()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .line_height(relative(1.))
                    .bg(cx.theme().red_light)
                    .text_color(white())
                    .text_size(px(8.))
                    .child(SharedString::from(unpushed.to_string())),
            );
        }

        row.on_click(
            cx.listener(move |this, _ev, window, cx| this.open_repo(&announcement, window, cx)),
        )
    }

    /// A local repository that is not yet set up for NIP-34, marked with a warning.
    fn render_local_row(&self, path: &Path, cx: &mut Context<Self>) -> impl IntoElement {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or("Untitled".into());
        let path = path.to_path_buf();
        let avatar = PixelAvatar::new(path.to_string_lossy());

        NavItem::new(format!("local-repo:{}", path.display()), name, avatar)
            .suffix(
                Icon::new(IconName::TriangleAlert)
                    .small()
                    .text_color(cx.theme().warning),
            )
            .on_click(cx.listener(move |this, _ev, window, cx| {
                this.open_local_repo(path.clone(), window, cx);
            }))
    }

    fn open_import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        import_dialog::open(window, cx);
    }

    /// The user avatar and name, wired into the titlebar drag area.
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
                                .child(UserAvatar::new(name.clone()).picture(picture))
                                .child(div().text_xs().font_semibold().child(name)),
                        ),
                    ),
                ),
            window,
            cx,
        )
    }

    /// Shown while no identity is signed in.
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
        let backend = Backend::global(cx);
        let profile_store = ProfileStore::global(cx);

        let profile = backend
            .read(cx)
            .current_user()
            .map(|public_key| profile_store.read(cx).get(&public_key));

        if profile.is_none() {
            return self.render_sign_in(window, cx);
        }

        v_flex()
            .image_cache(gpui::retain_all("sidebar"))
            .size_full()
            .justify_between()
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
                                        this.open_inbox(window, cx)
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
                    .child(self.render_repos(cx)),
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
                        .on_click(cx.listener(|_, _ev, window, cx| {
                            settings_dialog::open(window, cx);
                        })),
                    ),
            )
    }
}

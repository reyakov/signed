use std::collections::HashMap;
use std::sync::Arc;

use assets::CustomIconName;
use gpui::{
    AnyElement, App, AppContext as _, Context, FontWeight, IntoElement, ObjectFit,
    ParentElement as _, SharedString, Styled as _, StyledImage as _, Subscription, WeakEntity,
    Window, div, img, px,
};
use gpui_native_kit::avatar::Avatar;
use gpui_native_kit::badge::{Badge, BadgeVariant};
use gpui_native_kit::button::{Button, ButtonVariant};
use gpui_native_kit::common::{Sizable as _, h_flex, v_flex};
use gpui_native_kit::data::sidebar::{Sidebar, SidebarEntry};
use gpui_native_kit::icons::{Icon, IconRole};
use gpui_native_kit::layout::dock::DockState;
use gpui_native_kit::menu::{Menu, MenuItem};
use gpui_native_kit::theme::ActiveTheme as _;
use gpui_native_kit::tooltip::Tooltip;
use metadata::{Profile, ProfileDirectory};
use nostr::prelude::PublicKey;
use signed_core::{Announcement, RepoAddr};
use signed_state::{
    Backend, BackendEvent, CheckoutsStore, LocalReposStore, Nip34Kind, RepoListStore,
    ResolvedLocalRepo, SyncStatusDirectory,
};

mod helpers;

use crate::explore::ExploreView;
use crate::identity;
use crate::repo::dialogs;
use helpers::{local_avatar, local_platform, pick_banner};

pub struct SidebarView {
    weak: WeakEntity<Self>,
    dock: WeakEntity<DockState>,
    banner: SharedString,
    scanning: bool,
    /// User's announced repositories.
    announcements: Arc<Vec<Announcement>>,
    /// Local repositories found by the scan that are not announced yet.
    local_repos: Arc<Vec<ResolvedLocalRepo>>,
    /// Unpushed commit counts per announced repository.
    unpushed: HashMap<RepoAddr, usize>,
    /// Profiles rendered by the sidebar.
    directory: gpui::Entity<ProfileDirectory>,
    /// Sync statuses for the announced repositories.
    sync_directory: gpui::Entity<SyncStatusDirectory>,
    user_menu_open: bool,
    _subscriptions: Vec<Subscription>,
}

impl SidebarView {
    pub fn new(dock: WeakEntity<DockState>, cx: &mut Context<Self>) -> Self {
        let backend = Backend::global(cx);
        let repos = RepoListStore::global(cx);
        let local = LocalReposStore::global(cx);
        let checkouts = CheckoutsStore::global(cx);

        let mut subscriptions = Vec::new();

        // Identity changes swap the whole sidebar between the sign-in screen and the signed-in content.
        subscriptions.push(cx.subscribe(&backend, |this, _backend, event, cx| {
            let signer_changed = matches!(event, BackendEvent::SignerChanged);
            let signer_required = matches!(event, BackendEvent::SignerRequired);

            if signer_required {
                this.banner = pick_banner();
                cx.notify();
            }

            if this.refresh(cx) || signer_changed {
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
            weak: cx.entity().downgrade(),
            dock,
            banner: pick_banner(),
            announcements: Arc::new(Vec::new()),
            local_repos: Arc::new(Vec::new()),
            scanning: false,
            unpushed: HashMap::new(),
            directory: ProfileDirectory::new(cx),
            sync_directory: SyncStatusDirectory::new(cx),
            user_menu_open: false,
            _subscriptions: subscriptions,
        }
    }

    fn refresh(&mut self, cx: &mut Context<Self>) -> bool {
        let backend = Backend::global(cx);
        let user = backend.read(cx).current_user();

        let (announcements, local_repos, scanning) = {
            let repo_list = RepoListStore::global(cx).read(cx);

            let announcements = user
                .as_ref()
                .map(|user| repo_list.announcements_of(user))
                .unwrap_or_default();

            let local = LocalReposStore::global(cx).read(cx);
            let local_repos =
                LocalReposStore::resolve(&local.repos, &repo_list.announcements, &announcements);

            (announcements, local_repos, local.scanning)
        };

        let announcements_changed = *self.announcements != announcements;
        let local_changed = *self.local_repos != local_repos;
        let scanning_changed = self.scanning != scanning;

        self.announcements = Arc::new(announcements);
        self.local_repos = Arc::new(local_repos);
        self.scanning = scanning;

        let mut profiles: Vec<PublicKey> = user.into_iter().collect();
        let owners: Vec<PublicKey> = self
            .local_repos
            .iter()
            .filter_map(|entry| {
                entry
                    .nip34
                    .as_ref()
                    .and_then(|binding| binding.owner)
                    .filter(|owner| !profiles.contains(owner))
            })
            .collect();

        profiles.extend(owners);

        self.directory.update(cx, |directory, cx| {
            directory.track(profiles, cx);
        });

        let addrs: Vec<RepoAddr> = self
            .announcements
            .iter()
            .map(|announcement| announcement.addr())
            .collect();

        self.sync_directory.update(cx, |directory, cx| {
            directory.track(addrs, cx);
        });

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

    /// Renders the sidebar for the current identity state.
    pub fn build(&self, cx: &App) -> Sidebar {
        let backend = Backend::global(cx);

        let profile = backend
            .read(cx)
            .current_user()
            .map(|public_key| self.directory.read(cx).profile(&public_key, cx));

        let Some(profile) = profile else {
            return Sidebar::new("signed-sidebar").content(self.render_sign_in(cx));
        };

        let weak = self.weak.clone();
        let announcements = self.announcements.clone();
        let local_repos = self.local_repos.clone();
        let unpushed = self.unpushed.clone();
        let directory = self.directory.clone();
        let sync_directory = self.sync_directory.clone();

        Sidebar::new("signed-sidebar")
            .transparent(true)
            .header(self.render_user(&profile, self.scanning, cx))
            .entries(move |_, cx| {
                let muted = cx.theme().colors.muted_foreground;
                let mut entries = vec![
                    SidebarEntry::row("browse", "Browse").prefix(
                        Icon::named(CustomIconName::Globe)
                            .size(px(14.))
                            .color(muted),
                    ),
                    SidebarEntry::row("search", "Search")
                        .prefix(Icon::named(IconRole::Search).size(px(14.)).color(muted)),
                    SidebarEntry::header("All Repositories"),
                ];

                for announcement in announcements.iter() {
                    let owner = directory.read(cx).profile(&announcement.owner, cx);
                    let name = announcement
                        .name
                        .as_deref()
                        .map(SharedString::from)
                        .unwrap_or_else(|| SharedString::from(announcement.id.clone()));
                    let addr = announcement.addr();
                    let mut row = SidebarEntry::row(
                        SharedString::from(format!("repo:{}", announcement.id)),
                        name,
                    )
                    .prefix(
                        Avatar::new(announcement.owner)
                            .name(owner.name())
                            .picture(owner.picture())
                            .small(),
                    );

                    let count = unpushed.get(&addr).copied().unwrap_or(0);
                    let unsynced = sync_directory.read(cx).unsynced(&addr, cx);

                    if count > 0 || unsynced > 0 {
                        let mut badges = h_flex().gap_1();
                        if count > 0 {
                            badges = badges.child(
                                Badge::new(
                                    SharedString::from(format!("unpushed:{addr}")),
                                    count.to_string(),
                                )
                                .variant(BadgeVariant::Danger),
                            );
                        }
                        if unsynced > 0 {
                            badges = badges.child(
                                Badge::new(
                                    SharedString::from(format!("unsynced:{addr}")),
                                    unsynced.to_string(),
                                )
                                .variant(BadgeVariant::Accent),
                            );
                        }
                        row = row.suffix(badges);
                    }

                    entries.push(row);
                }

                for entry in local_repos.iter() {
                    let suffix: AnyElement = match entry.nip34.as_ref() {
                        Some(binding) => {
                            let (label, note) = match binding.kind {
                                Nip34Kind::Initialized => {
                                    let platform = local_platform(binding).unwrap_or("Grasp");
                                    (platform, "Initialized for NIP-34")
                                }
                                Nip34Kind::Cloned => ("Cloned", "Cloned but not initialized"),
                                Nip34Kind::ToolingOnly => ("Tooling", "Grasp tooling only"),
                            };
                            Badge::new(
                                SharedString::from(format!("local-badge:{}", entry.path.display())),
                                label,
                            )
                            .tooltip(Tooltip::text(note))
                            .into_any_element()
                        }
                        None => Badge::new(
                            SharedString::from(format!(
                                "local-unpublished:{}",
                                entry.path.display()
                            )),
                            "Unpublished",
                        )
                        .variant(BadgeVariant::Warning)
                        .into_any_element(),
                    };

                    entries.push(
                        SidebarEntry::row(
                            SharedString::from(format!("local-repo:{}", entry.path.display())),
                            entry.name(),
                        )
                        .prefix(local_avatar(entry, &directory, cx))
                        .suffix(suffix),
                    );
                }

                entries
            })
            .on_change(move |index, window, cx| {
                if let Err(error) = weak.update(cx, |this, cx| this.activate(index, window, cx)) {
                    log::warn!("failed to activate sidebar row: {error}");
                }
            })
    }

    fn activate(&mut self, index: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = index else {
            return;
        };

        match index {
            0 | 1 => {
                self.open_explore(window, cx);
                return;
            }
            _ => {}
        }

        let Some(row) = index.checked_sub(3) else {
            return;
        };

        if row < self.announcements.len() {
            let announcement = self.announcements[row].clone();
            let addr = announcement.addr();
            crate::repo::actions::open_repo_panel(
                &self.dock,
                &addr,
                Some(&announcement),
                window,
                cx,
            );
            return;
        }

        let local_ix = row - self.announcements.len();
        if let Some(entry) = self.local_repos.get(local_ix).cloned() {
            self.open_local_entry(entry, window, cx);
        }
    }

    /// A local repository opens as the announced repository when its binding matches one.
    fn open_local_entry(
        &mut self,
        entry: ResolvedLocalRepo,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ResolvedLocalRepo {
            path,
            nip34,
            announcement,
        } = entry;

        if let Some(announcement) = announcement {
            let id = SharedString::from(format!("repo:{}", announcement.addr()));
            let dock = self.dock.clone();
            let dock_for_build = dock.clone();
            crate::shared::panels::open(
                &dock,
                id,
                move |window, cx| {
                    cx.new(|cx| {
                        crate::repo::RepoDetailView::new_local_announced(
                            dock_for_build.clone(),
                            announcement.clone(),
                            path.clone(),
                            window,
                            cx,
                        )
                    })
                },
                window,
                cx,
            );
            return;
        }

        let id = SharedString::from(format!("repo-local:{}", path.display()));
        let dock = self.dock.clone();
        let dock_for_build = dock.clone();
        crate::shared::panels::open(
            &dock,
            id,
            move |window, cx| {
                cx.new(|cx| {
                    crate::repo::RepoDetailView::new_local(
                        dock_for_build.clone(),
                        path.clone(),
                        nip34.clone(),
                        window,
                        cx,
                    )
                })
            },
            window,
            cx,
        );
    }

    fn open_explore(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let dock = self.dock.clone();
        let dock_for_build = dock.clone();
        crate::shared::panels::open(
            &dock,
            "explore",
            move |_window, cx| cx.new(|cx| ExploreView::new(dock_for_build.clone(), cx)),
            window,
            cx,
        );
    }

    fn open_panel(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Err(error) = self
            .dock
            .update(cx, |dock, cx| dock.open_panel(id, window, cx))
        {
            log::warn!("failed to open panel {id}: {error}");
        }
    }

    fn render_user(&self, profile: &Profile, scanning: bool, cx: &App) -> impl IntoElement {
        let name = profile.name();
        let picture = profile.picture();
        let public_key = profile.public_key();
        let weak = self.weak.clone();
        let weak_for_open = self.weak.clone();
        let muted = cx.theme().colors.muted_foreground;
        let dock = self.dock.clone();

        let menu = Menu::new("user-menu")
            .open(self.user_menu_open)
            .on_change({
                let weak = weak_for_open.clone();
                move |open, _, cx| {
                    if let Err(error) = weak.update(cx, |this, cx| {
                        this.user_menu_open = open;
                        cx.notify();
                    }) {
                        log::warn!("failed to update user menu: {error}");
                    }
                }
            })
            .trigger({
                let name = name.clone();
                let picture = picture.clone();
                move |_, _| {
                    h_flex()
                        .gap_1()
                        .items_center()
                        .child(
                            Avatar::new(public_key)
                                .name(name.clone())
                                .picture(picture.clone()),
                        )
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(name.clone()),
                        )
                        .child(
                            Icon::named(IconRole::ChevronDown)
                                .size(px(12.))
                                .color(muted),
                        )
                        .into_any_element()
                }
            })
            .items(move |_, _| {
                let profile_weak = weak.clone();
                let relays_weak = weak.clone();

                vec![
                    MenuItem::new("View Profile").handler(move |window, cx| {
                        if let Err(error) = profile_weak.update(cx, |this, cx| {
                            this.open_panel("profile", window, cx);
                        }) {
                            log::warn!("failed to open profile: {error}");
                        }
                    }),
                    MenuItem::new("View Relays").handler(move |window, cx| {
                        if let Err(error) = relays_weak.update(cx, |this, cx| {
                            this.open_panel("relays", window, cx);
                        }) {
                            log::warn!("failed to open relays: {error}");
                        }
                    }),
                    MenuItem::separator(),
                    MenuItem::new("Exit")
                        .destructive(true)
                        .handler(|_, cx| identity::helpers::open_exit(cx)),
                ]
            });

        h_flex().w_full().justify_between().child(menu).child(
            h_flex()
                .gap_1()
                .child(
                    Button::new("rescan-repos")
                        .icon(Icon::named(IconRole::Loader))
                        .variant(ButtonVariant::Ghost)
                        .loading(scanning)
                        .disabled(scanning)
                        .tooltip(Tooltip::text("Rescan for local repositories"))
                        .on_click(move |_, _, cx| {
                            let local_repos = LocalReposStore::global(cx);
                            local_repos.update(cx, |store, cx| store.rescan(cx));
                        }),
                )
                .child(
                    Button::new("add-repo")
                        .icon(Icon::named(IconRole::Plus))
                        .variant(ButtonVariant::Ghost)
                        .tooltip(Tooltip::text("Create repository"))
                        .on_click({
                            let dock = dock.clone();
                            move |_, window, cx| {
                                dialogs::create::open(dock.clone(), window, cx);
                            }
                        }),
                ),
        )
    }

    /// Shown while no identity is signed in.
    fn render_sign_in(&self, _cx: &App) -> impl IntoElement {
        let weak = self.weak.clone();
        let import_weak = self.weak.clone();

        v_flex()
            .size_full()
            .relative()
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
                    .gap_4()
                    .child(img("backgrounds/headline.png").max_w_48())
                    .child(
                        v_flex()
                            .gap_1()
                            .w_full()
                            .child(Button::new("onboarding").label("Join now").on_click(
                                move |_, window, cx| {
                                    if let Err(error) = weak.update(cx, |this, cx| {
                                        this.open_panel("welcome", window, cx);
                                        identity::helpers::open_onboarding(cx);
                                    }) {
                                        log::warn!("failed to open onboarding: {error}");
                                    }
                                },
                            ))
                            .child(
                                Button::new("import")
                                    .label("Import identity")
                                    .variant(ButtonVariant::Secondary)
                                    .on_click(move |_, _, cx| {
                                        if let Err(error) = import_weak
                                            .update(cx, |_, cx| identity::helpers::open_import(cx))
                                        {
                                            log::warn!("failed to open import: {error}");
                                        }
                                    }),
                            ),
                    ),
            )
    }
}

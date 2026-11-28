use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    App, AppContext as _, Context, Entity, IntoElement, KeyBinding, KeyBindingContextPredicate,
    ParentElement as _, SharedString, Subscription, WeakEntity, Window,
};
use metadata::{Profile, ProfileDirectory};
use nostr::prelude::PublicKey;
use repository_switcher::RepositoryEntry;
pub(crate) use repository_switcher::RepositorySwitcher;
use signed_core::Announcement;
use signed_state::{Backend, BackendEvent, LocalReposStore, RepoListStore, ResolvedLocalRepo};
use ui::avatar::Avatar;
use ui::button::{Button, ButtonVariant};
use ui::command_palette::{Command, CommandPalette, CommandPaletteState};
use ui::icons::{Icon, IconRole};
use ui::layout::panel::PanelSlot;
use ui::layout::root::RootContext as _;
use ui::menu::{Menu, MenuItem};

use crate::app::AppRoot;
use crate::identity::exit::ExitDialog;
use crate::repo::RepoDetailView;
use crate::repo::dialogs::create::CreateRepoDialog;
use crate::settings::SettingsDialog;
use crate::shared::labels::{AnnouncementExt as _, ResolvedLocalRepoExt as _};
use crate::shared::log_dropped;

mod repository_switcher;

/// The key context the repository shortcuts bind against.
pub(crate) const REPOSITORY_KEY_CONTEXT: &str = "Signed";
const CREATE_KEYSTROKE: &str = "secondary-n";
const REPOSITORY_KEYSTROKES: [&str; 9] = [
    "secondary-1",
    "secondary-2",
    "secondary-3",
    "secondary-4",
    "secondary-5",
    "secondary-6",
    "secondary-7",
    "secondary-8",
    "secondary-9",
];

/// Opens the repository at this index in the combined announced/local list.
#[derive(Clone, PartialEq, gpui::Action)]
#[action(namespace = signed, no_json)]
pub(crate) struct SwitchRepository {
    pub index: usize,
}

/// Opens the create-repository dialog.
#[derive(Clone, PartialEq, gpui::Action)]
#[action(namespace = signed, no_json)]
pub(crate) struct CreateRepository;

pub struct TitlebarView {
    weak: WeakEntity<Self>,
    app: WeakEntity<AppRoot>,
    /// User's announced repositories.
    announcements: Arc<Vec<Announcement>>,
    /// Local repositories found by the scan that are not announced yet.
    local_repos: Arc<Vec<ResolvedLocalRepo>>,
    /// Profiles rendered by the repository switcher.
    directory: Entity<ProfileDirectory>,
    repository_switcher: Entity<RepositorySwitcher>,
    user_menu_open: bool,
    _subscriptions: Vec<Subscription>,
}

impl TitlebarView {
    pub fn new(app: WeakEntity<AppRoot>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let backend = Backend::global(cx);
        let repos = RepoListStore::global(cx);
        let local = LocalReposStore::global(cx);
        let directory = ProfileDirectory::new(cx);
        let weak = cx.weak_entity();

        let palette = Self::new_palette(weak.clone(), window, cx);
        cx.open_palette(palette);

        let mut subscriptions = Vec::new();

        let repository_switcher = cx.new(|cx| {
            RepositorySwitcher::new(
                directory.clone(),
                {
                    let weak = weak.clone();
                    move |index, window, cx| {
                        log_dropped(weak.update(cx, |this, cx| {
                            this.activate(index, window, cx);
                        }));
                    }
                },
                {
                    let weak = weak.clone();
                    move |window, cx| {
                        log_dropped(weak.update(cx, |this, cx| {
                            this.create_repository(window, cx);
                        }));
                    }
                },
                |cx| {
                    let local_repos = LocalReposStore::global(cx);
                    local_repos.update(cx, |store, cx| {
                        store.rescan(cx);
                    });
                },
                cx,
            )
        });

        subscriptions.push(cx.subscribe(&backend, |this, _backend, event, cx| {
            // Push progress and sync events cannot change the titlebar's data.
            if !matches!(
                event,
                BackendEvent::SignerChanged | BackendEvent::SignerRequired
            ) {
                return;
            }

            if this.refresh(cx) {
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

        cx.defer_in(window, move |this, _window, cx| {
            if this.refresh(cx) {
                cx.notify();
            }
        });

        Self {
            weak,
            app,
            announcements: Arc::new(Vec::new()),
            local_repos: Arc::new(Vec::new()),
            directory,
            repository_switcher,
            user_menu_open: false,
            _subscriptions: subscriptions,
        }
    }

    fn new_palette(
        weak: WeakEntity<Self>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<CommandPaletteState> {
        cx.new(|cx| {
            let matcher = |query: &str, commands: &[Command]| {
                let needle = query.to_lowercase();
                commands
                    .iter()
                    .filter(|command| command.label.to_lowercase().contains(&needle))
                    .cloned()
                    .collect()
            };

            CommandPaletteState::new(
                CommandPalette::new("command-palette", Self::palette_commands(), matcher)
                    .on_execute(move |command, window, cx| match command.id.as_ref() {
                        "create-repo" => {
                            log_dropped(weak.update(cx, |this, cx| this.open_create(window, cx)));
                        }
                        "rescan-repos" => {
                            let local_repos = LocalReposStore::global(cx);
                            local_repos.update(cx, |store, cx| store.rescan(cx));
                        }
                        "open-settings" => {
                            log_dropped(weak.update(cx, |this, cx| this.open_settings(cx)));
                        }
                        "exit" => {
                            log_dropped(weak.update(cx, |this, cx| this.open_exit(cx)));
                        }
                        _ => {}
                    }),
                window,
                cx,
            )
        })
    }

    fn palette_commands() -> Vec<Command> {
        vec![
            Command::new("create-repo", "Create repository").icon(Icon::named(IconRole::Plus)),
            Command::new("rescan-repos", "Rescan for local repositories")
                .icon(Icon::named(IconRole::Refresh)),
            Command::new("open-settings", "Settings").icon(Icon::named(IconRole::Settings)),
            Command::new("exit", "Exit"),
        ]
    }

    fn refresh(&mut self, cx: &mut Context<Self>) -> bool {
        let backend = Backend::global(cx);
        let user = backend.read(cx).current_user();

        let (announcements, local_repos) = {
            let repo_list = RepoListStore::global(cx).read(cx);

            let announcements = user
                .as_ref()
                .map(|user| repo_list.announcements_of(user))
                .unwrap_or_default();

            let local = LocalReposStore::global(cx).read(cx);
            let local_repos =
                LocalReposStore::resolve(&local.repos, &repo_list.announcements, &announcements);

            (announcements, local_repos)
        };

        let announcements_changed = *self.announcements != announcements;
        let local_changed = *self.local_repos != local_repos;

        self.announcements = Arc::new(announcements);
        self.local_repos = Arc::new(local_repos);

        let mut profiles: Vec<PublicKey> = user.into_iter().collect();
        profiles.extend(
            self.local_repos
                .iter()
                .filter_map(|entry| {
                    entry
                        .nip34
                        .as_ref()
                        .and_then(|binding| binding.owner)
                        .filter(|owner| !profiles.contains(owner))
                })
                .collect::<Vec<PublicKey>>(),
        );

        self.directory.update(cx, |directory, cx| {
            directory.track(profiles, cx);
        });

        if announcements_changed || local_changed {
            let entries = self.repository_entries();
            self.repository_switcher.update(cx, |switcher, cx| {
                switcher.set_entries(entries, cx);
            });
        }

        announcements_changed || local_changed
    }

    fn repository_entries(&self) -> Vec<RepositoryEntry> {
        let mut entries = Vec::with_capacity(self.announcements.len() + self.local_repos.len());

        for (index, announcement) in self.announcements.iter().enumerate() {
            let label = announcement
                .name
                .as_deref()
                .map(SharedString::from)
                .unwrap_or_else(|| SharedString::from(announcement.id.clone()));

            entries.push(RepositoryEntry {
                index,
                key: announcement.panel_key(),
                label_lower: SharedString::from(label.to_lowercase()),
                label,
                owner: Some(announcement.owner),
            });
        }

        for (local_index, entry) in self.local_repos.iter().enumerate() {
            let owner = entry.nip34.as_ref().and_then(|binding| binding.owner);
            let label = entry.name();

            entries.push(RepositoryEntry {
                index: self.announcements.len() + local_index,
                key: entry.panel_key(),
                label_lower: SharedString::from(label.to_lowercase()),
                label,
                owner,
            });
        }

        entries
    }

    pub(crate) fn repository_switcher(&self) -> Entity<RepositorySwitcher> {
        self.repository_switcher.clone()
    }

    pub(crate) fn activate_shortcut(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.repository_switcher.update(cx, |switcher, cx| {
            switcher.close(cx);
        });
        self.activate(index, window, cx);
    }

    pub(crate) fn create_repository(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.repository_switcher.update(cx, |switcher, cx| {
            switcher.close(cx);
        });
        self.open_create(window, cx);
    }

    fn open_settings(&mut self, cx: &mut Context<Self>) {
        let dialog = SettingsDialog::new(cx);
        cx.open_dialog(dialog);
    }

    fn open_exit(&mut self, cx: &mut Context<Self>) {
        let dialog = ExitDialog::new(cx);
        cx.open_dialog(dialog);
    }

    pub(crate) fn open_create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let view = CreateRepoDialog::new(self.app.clone(), window, cx);
        cx.open_dialog(view);
    }

    fn activate(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(announcement) = self.announcements.get(index) {
            let addr = announcement.addr();
            let result = self.app.update(cx, |root, cx| {
                root.open_repo_panel(&addr, Some(announcement), window, cx);
            });
            log_dropped(result);
            return;
        }

        let Some(entry) = index
            .checked_sub(self.announcements.len())
            .and_then(|local| self.local_repos.get(local))
            .cloned()
        else {
            return;
        };

        self.open_local_entry(entry, window, cx);
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
            let workspace = self.app.clone();
            let workspace_for_build = workspace.clone();

            log_dropped(workspace.update(cx, |root, cx| {
                root.panels().clone().update(cx, |panels, cx| {
                    panels.open(
                        PanelSlot::Main,
                        id,
                        move |window, cx| {
                            cx.new(|cx| {
                                RepoDetailView::new_local_announced(
                                    workspace_for_build.clone(),
                                    announcement.clone(),
                                    path.clone(),
                                    window,
                                    cx,
                                )
                            })
                        },
                        window,
                        cx,
                    )
                })
            }));
            return;
        }

        let id = SharedString::from(format!("repo-local:{}", path.display()));
        let workspace = self.app.clone();
        let workspace_for_build = workspace.clone();

        log_dropped(workspace.update(cx, |root, cx| {
            root.panels().clone().update(cx, |panels, cx| {
                panels.open(
                    PanelSlot::Main,
                    id,
                    move |window, cx| {
                        cx.new(|cx| {
                            RepoDetailView::new_local(
                                workspace_for_build.clone(),
                                path.clone(),
                                nip34.clone(),
                                window,
                                cx,
                            )
                        })
                    },
                    window,
                    cx,
                )
            })
        }));
    }

    pub fn render_user_menu(&self, cx: &App) -> Option<impl IntoElement> {
        let profile = Backend::global(cx)
            .read(cx)
            .current_user()
            .map(|public_key| self.directory.read(cx).profile(&public_key, cx))?;

        Some(self.render_user(&profile, cx).into_any_element())
    }

    fn render_user(&self, profile: &Profile, _cx: &App) -> impl IntoElement {
        let name = profile.name();
        let picture = profile.picture();
        let public_key = profile.public_key();
        let weak = self.weak.clone();

        Menu::new("user-menu")
            .open(self.user_menu_open)
            .on_change({
                let weak = weak.clone();
                move |open, _, cx| {
                    log_dropped(weak.update(cx, |this, cx| {
                        this.user_menu_open = open;
                        cx.notify();
                    }));
                }
            })
            .trigger({
                let name = name.clone();
                let picture = picture.clone();
                move |_, _| {
                    Button::new("user-menu-trigger")
                        .variant(ButtonVariant::Ghost)
                        .fully_rounded(true)
                        .padded(false)
                        .accessibility_label(name.clone())
                        .child(
                            Avatar::new(public_key)
                                .name(name.clone())
                                .picture(picture.clone())
                                .fully_rounded(true),
                        )
                        .into_any_element()
                }
            })
            .items(move |_, _| {
                vec![
                    MenuItem::new("Settings").handler({
                        let weak = weak.clone();
                        move |_, cx| {
                            log_dropped(weak.update(cx, |this, cx| this.open_settings(cx)));
                        }
                    }),
                    MenuItem::separator(),
                    MenuItem::new("Exit").destructive(true).handler({
                        let weak = weak.clone();
                        move |_, cx| {
                            log_dropped(weak.update(cx, |this, cx| this.open_exit(cx)));
                        }
                    }),
                ]
            })
    }
}

impl TitlebarView {
    /// Binds the repository shortcuts that the shell and the switcher share.
    pub(crate) fn bind_keys(cx: &mut App) {
        let context = match KeyBindingContextPredicate::parse(REPOSITORY_KEY_CONTEXT) {
            Ok(context) => Some(Rc::new(context)),
            Err(error) => {
                log::error!("failed to parse repository key context: {error}");
                return;
            }
        };

        let bindings = REPOSITORY_KEYSTROKES
            .into_iter()
            .enumerate()
            .map(|(index, keystroke)| {
                KeyBinding::load(
                    keystroke,
                    Box::new(SwitchRepository { index }),
                    context.clone(),
                    false,
                    None,
                    cx,
                )
            })
            .chain(std::iter::once(KeyBinding::load(
                CREATE_KEYSTROKE,
                Box::new(CreateRepository),
                context.clone(),
                false,
                None,
                cx,
            )))
            .collect::<Result<Vec<_>, _>>();

        let bindings = match bindings {
            Ok(bindings) => bindings,
            Err(error) => {
                log::error!("failed to parse repository shortcut keystrokes: {error}");
                return;
            }
        };

        cx.bind_keys(bindings);
    }
}

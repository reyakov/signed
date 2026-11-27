mod placeholder;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyView, App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Subscription, WeakEntity, Window, div,
};
use nostr::prelude::EventId;
use settings::{AppearanceMode, SettingsStore};
use signed_core::{Announcement, RepoAddr};
use signed_state::{Backend, BackendEvent, RepoStore};
use ui::common::{h_flex, v_flex};
use ui::icons::IconRole;
use ui::layout::panel::{PanelSlot, PanelStack};
use ui::layout::root::{Root, RootContext as _};
use ui::layout::titlebar::Titlebar;
use ui::theme::{ThemeEngine, ThemeVariant};
use ui::toast::Toast;

use crate::app::placeholder::PlaceholderView;
use crate::explore::ExploreView;
use crate::identity::import::ImportDialog;
use crate::identity::onboarding::OnboardingDialog;
use crate::identity::passphrase::PassphraseDialog;
use crate::issues::IssuesView;
use crate::issues::detail::IssueDetailView;
use crate::pull_requests::{PullRequestDetailView, PullRequestsView};
use crate::repo::RepoDetailView;
use crate::shared::log_dropped;
use crate::titlebar::{CreateRepository, REPOSITORY_KEY_CONTEXT, SwitchRepository, TitlebarView};

/// An item of a repository to open from outside its detail panel.
pub enum RepoItem {
    Issue(EventId),
    PullRequest(EventId),
    Patch,
}

/// The issue and pull-request tab views a repository panel embeds.
pub struct RepoTabs {
    pub issues: AnyView,
    pub pulls: AnyView,
    /// Keep the tab totals flowing into the repository panel.
    pub(crate) _subscriptions: Vec<Subscription>,
}

pub struct AppRoot {
    panels: Entity<PanelStack>,
    titlebar: Entity<TitlebarView>,
    appearance: AppearanceMode,
    _subscriptions: Vec<Subscription>,
}

impl AppRoot {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        TitlebarView::bind_keys(cx);

        let app = cx.weak_entity();
        let panels = cx.new(|cx| PanelStack::new("signed-panel", cx));
        let titlebar = cx.new(|cx| TitlebarView::new(app.clone(), window, cx));

        let empty = cx.new(|_| PlaceholderView::new("Select a repository"));
        panels.update(cx, |panels, cx| {
            panels.set_empty(empty, None, cx);
        });

        let backend = Backend::global(cx);
        let settings = SettingsStore::global(cx);

        let mut subscriptions = Vec::new();

        subscriptions.push(cx.subscribe_in(
            &backend,
            window,
            |this, _backend, event, window, cx| {
                match event {
                    BackendEvent::PassphraseRequired => {
                        this.open_passphrase(cx);
                    }
                    BackendEvent::Error(message) => {
                        cx.toast(Toast::new(message).icon(IconRole::Warning));
                    }
                    BackendEvent::SignerChanged => {
                        cx.defer_in(window, |this, window, cx| {
                            this.reset_to_explore(window, cx);
                        });
                    }
                    _ => {}
                };
            },
        ));

        subscriptions.push(cx.observe(&settings, |this, store, cx| {
            let appearance = store.read(cx).settings().appearance;
            if appearance != this.appearance {
                this.appearance = appearance;
                Self::apply_appearance_mode(appearance, cx);
            }
        }));

        let backend_deferred = backend.clone();
        cx.defer_in(window, move |this, window, cx| {
            if backend_deferred.read(cx).passphrase_required() {
                this.open_passphrase(cx);
            }
            this.reset_to_explore(window, cx);
        });

        Self {
            panels,
            titlebar,
            appearance: settings.read(cx).settings().appearance,
            _subscriptions: subscriptions,
        }
    }

    fn open_passphrase(&mut self, cx: &mut Context<Self>) {
        let dialog = PassphraseDialog::new(cx);
        cx.open_dialog(dialog);
    }

    /// Builds the window root with the app shell inside it.
    pub fn mount(window: &mut Window, cx: &mut App) -> Entity<Root> {
        let root = cx.new(|cx| Root::new(window, cx));
        let content = cx.new(|cx| AppRoot::new(window, cx));
        root.update(cx, |root, cx| {
            root.set_content(content, cx);
        });
        root
    }

    /// Applies one appearance mode to the kit theme.
    pub fn apply_appearance_mode(mode: AppearanceMode, cx: &mut App) {
        match mode {
            AppearanceMode::System => ThemeEngine::follow_system(cx),
            AppearanceMode::Light => ThemeEngine::set_mode_override(Some(ThemeVariant::Light), cx),
            AppearanceMode::Dark => ThemeEngine::set_mode_override(Some(ThemeVariant::Dark), cx),
        }
    }

    /// The panel stack: the main stack, the split stack, and the divider between them.
    pub fn panels(&self) -> &Entity<PanelStack> {
        &self.panels
    }

    /// Empties the stack and opens it on the explore panel, where every session starts.
    fn reset_to_explore(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.panels.update(cx, |panels, cx| panels.clear(cx));
        ExploreView::open(&self.panels, cx.weak_entity(), window, cx);
    }

    /// Builds the tab views for the repository served by `store`, observing their totals into `this`.
    pub(crate) fn repo_tabs(
        store: &Entity<RepoStore>,
        workspace: &WeakEntity<Self>,
        cx: &mut Context<RepoDetailView>,
    ) -> RepoTabs {
        let issues = cx.new(|cx| IssuesView::new(workspace.clone(), store.clone(), cx));
        let pulls = cx.new(|cx| PullRequestsView::new(workspace.clone(), store.clone(), cx));

        let subscriptions = vec![
            cx.observe(&issues, |this, view, cx| {
                this.issue_count = view.read(cx).total_count()
            }),
            cx.observe(&pulls, |this, view, cx| {
                this.pull_count = view.read(cx).total_count()
            }),
        ];

        RepoTabs {
            issues: issues.into(),
            pulls: pulls.into(),
            _subscriptions: subscriptions,
        }
    }

    /// Opens a repository as the main panel, keyed by its address.
    pub(crate) fn open_repo_panel(
        &mut self,
        addr: &RepoAddr,
        hint: Option<&Announcement>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = SharedString::from(format!("repo:{addr}"));
        let addr = addr.clone();
        let hint = hint.cloned();
        let workspace = cx.weak_entity();

        self.panels.update(cx, |panels, cx| {
            panels.open(
                PanelSlot::Main,
                id,
                move |window, cx| {
                    let addr = addr.clone();
                    let hint = hint.clone();
                    cx.new(|cx| RepoDetailView::new(workspace.clone(), addr, hint, window, cx))
                },
                window,
                cx,
            );
        });
    }

    /// Opens an issue or pull request using a store built here.
    pub(crate) fn open_repo_item(
        &mut self,
        addr: &RepoAddr,
        hint: Option<&Announcement>,
        item: RepoItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match item {
            RepoItem::Issue(issue_id) => {
                let store = cx.new(|cx| RepoStore::new(addr.clone(), hint.cloned(), cx));
                IssueDetailView::open(&self.panels, store, issue_id, window, cx);
            }
            RepoItem::PullRequest(pr_id) => {
                let store = cx.new(|cx| RepoStore::new(addr.clone(), hint.cloned(), cx));
                PullRequestDetailView::open(
                    &self.panels,
                    &cx.weak_entity(),
                    store,
                    pr_id,
                    window,
                    cx,
                );
            }
            // Opening a patch series directly is not a feature.
            RepoItem::Patch => {}
        }
    }

    /// Opens the dialog that imports an identity from a secret.
    pub(crate) fn open_import_dialog(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let dialog = ImportDialog::new(cx);
        cx.open_dialog(dialog);
    }

    /// Opens the dialog that creates an identity.
    pub(crate) fn open_onboarding_dialog(&mut self, cx: &mut Context<Self>) {
        let dialog = OnboardingDialog::new(cx);
        cx.open_dialog(dialog);
    }
}

impl Render for AppRoot {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let repo_tabs = PanelStack::main_tabs(self.panels.clone(), cx);
        let titlebar_view = self.titlebar.read(cx);

        div()
            .relative()
            .size_full()
            .key_context(REPOSITORY_KEY_CONTEXT)
            .on_action(cx.listener(|this, action: &SwitchRepository, window, cx| {
                let titlebar = this.titlebar.downgrade();
                let index = action.index;
                window.defer(cx, move |window, cx| {
                    log_dropped(titlebar.update(cx, |titlebar, cx| {
                        titlebar.activate_shortcut(index, window, cx);
                    }));
                });
            }))
            .on_action(cx.listener(|this, _: &CreateRepository, window, cx| {
                this.titlebar.update(cx, |titlebar, cx| {
                    titlebar.create_repository(window, cx);
                });
            }))
            .child(
                v_flex()
                    .size_full()
                    .overflow_hidden()
                    .child(
                        Titlebar::new("signed-titlebar")
                            .prefix(
                                h_flex()
                                    .gap_1()
                                    .child(titlebar_view.render_new_repository())
                                    .child(titlebar_view.render_explore())
                                    .child(titlebar_view.repository_switcher()),
                            )
                            .tab_bar(
                                h_flex()
                                    .gap_1()
                                    .when_some(repo_tabs, |bar, tabs| bar.child(tabs)),
                            )
                            .when_some(titlebar_view.render_user_menu(cx), |titlebar, menu| {
                                titlebar.suffix(menu)
                            }),
                    )
                    .child(self.panels.clone()),
            )
    }
}

use std::path::{Path, PathBuf};

use anyhow::Error;
use assets::CustomIconName;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, ClipboardItem, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
    px,
};
use gpui_native_kit::avatar::Avatar;
use gpui_native_kit::clipboard::ClipboardButton;
use gpui_native_kit::combobox::ComboboxState;
use gpui_native_kit::common::{Sizable as _, h_flex, v_flex};
use gpui_native_kit::icons::Icon;
use gpui_native_kit::menu::MenuItem;
use gpui_native_kit::theme::ActiveTheme as _;
use metadata::ProfileDirectory;
use nostr::nips::nip19::Nip19Coordinate;
use nostr::prelude::{RelayUrl, ToBech32};
use signed_core::Announcement;
use signed_git::{DiffHunk, DiffLine, DiffLineKind, FileCommit, FileDiff, Repo};
use signed_state::{Nip34Binding, RepoListStore, RepoStore};
use utils::{middle_truncate, relative_time_secs};

use super::RepoDetailView;
use crate::shared::tree::{TreeItemSeed, build_tree_items};

const GUTTER_WIDTH: f32 = 44.;
pub const DIFF_ROW_HEIGHT: f32 = 20.;
pub const COMMIT_ROW_HEIGHT: f32 = 56.;

#[derive(Clone, Copy)]
pub(super) enum DiffRow {
    Hunk {
        old_start: u32,
        old_lines: u32,
        new_start: u32,
        new_lines: u32,
    },
    Line {
        hunk: usize,
        line: usize,
    },
}

pub(super) struct RepoData {
    pub(super) tree: Vec<TreeItemSeed>,
    pub(super) entries: Vec<PathBuf>,
    pub(super) readme_path: Option<PathBuf>,
    pub(super) readme: Option<Vec<u8>>,
    pub(super) worktree: Option<PathBuf>,
    pub(super) branches: Vec<String>,
    pub(super) tags: Vec<String>,
    pub(super) current_branch: Option<String>,
    pub(super) head_commit: Option<FileCommit>,
}

#[derive(Clone)]
pub(super) struct ShareTargets {
    /// NIP-19 `naddr1...` of the announcement, with its announced relays.
    pub(super) naddr: String,
    /// Hex ID of the announcement event itself.
    event_id: String,
    /// NIP-34 coordinate `30617:<pubkey>:<repo-id>`.
    coordinate: String,
    gitworkshop: String,
    ditto: String,
}

impl ShareTargets {
    pub(super) fn from_announcement(announcement: &Announcement) -> Self {
        let addr = announcement.addr();
        let coordinate = addr.to_string();
        let naddr = Nip19Coordinate::new(addr.into(), announcement.relays.iter().cloned())
            .to_bech32()
            .unwrap_or_else(|_| coordinate.clone());

        Self {
            naddr: naddr.clone(),
            event_id: announcement
                .event_id
                .to_bech32()
                .unwrap_or_else(|_| announcement.event_id.to_string()),
            coordinate,
            gitworkshop: format!("https://gitworkshop.dev/{naddr}"),
            ditto: format!("https://ditto.pub/{naddr}"),
        }
    }

    pub(super) fn menu(&self) -> Vec<MenuItem> {
        let items = [
            ("GitWorkshop", truncate_naddr_link(&self.gitworkshop, 4)),
            ("Ditto", truncate_naddr_link(&self.ditto, 4)),
            ("Event ID", middle_truncate(&self.event_id, 10, 10)),
            ("Coordinate", middle_truncate(&self.coordinate, 10, 10)),
        ];

        let values = [
            self.gitworkshop.clone(),
            self.ditto.clone(),
            self.event_id.clone(),
            self.coordinate.clone(),
        ];

        items
            .into_iter()
            .zip(values)
            .map(|((label, detail), value)| {
                MenuItem::new(SharedString::from(format!("{label}  {detail}"))).handler(
                    move |_, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(value.clone()));
                    },
                )
            })
            .collect()
    }
}

pub(super) fn clone_group(label: &str, id: &'static str, command: &SharedString) -> AnyElement {
    v_flex()
        .gap_1()
        .child(div().text_xs().child(SharedString::from(label)))
        .child(ClipboardButton::new(id, command.clone()))
        .into_any_element()
}

pub(super) fn nostr_clone_url(announcement: &Announcement, nip05: Option<&str>) -> SharedString {
    let owner = announcement.owner;
    let user = nip05
        .map(str::to_owned)
        .unwrap_or_else(|| owner.to_bech32().unwrap_or_else(|_| owner.to_string()));

    let mut url = format!("nostr://{user}");
    if let Some(hint) = announcement.relays.first().and_then(RelayUrl::domain) {
        url.push('/');
        url.push_str(hint);
    }
    url.push('/');
    url.push_str(&announcement.id);

    SharedString::from(url)
}

pub(super) fn fork_row(
    announcement: &Announcement,
    cx: &mut Context<RepoDetailView>,
) -> Option<AnyElement> {
    let upstream = announcement.upstream.as_ref()?;

    let (label, clickable) = match &upstream.addr {
        Some(addr) => {
            let name = RepoListStore::global(cx)
                .read(cx)
                .announcements
                .iter()
                .find(|a| a.addr() == *addr)
                .map(|a| {
                    a.name
                        .as_deref()
                        .map(SharedString::from)
                        .unwrap_or_else(|| SharedString::from(a.id.clone()))
                })
                .unwrap_or_else(|| SharedString::from(addr.identifier().to_owned()));
            (SharedString::from(format!("Forked from {name}")), true)
        }
        None => (SharedString::from(upstream.display().as_str()), false),
    };

    let row = h_flex()
        .gap_1()
        .min_w_0()
        .text_sm()
        .text_color(cx.theme().colors.muted_foreground)
        .child(Icon::named(CustomIconName::GitBranch).size(px(14.)))
        .child(div().whitespace_nowrap().text_ellipsis().child(label));

    Some(if clickable {
        row.id("fork-upstream")
            .cursor_pointer()
            .on_click(cx.listener(|this, _, window, cx| this.open_upstream(window, cx)))
            .into_any_element()
    } else {
        row.into_any_element()
    })
}

pub(super) fn truncate_naddr_link(url: &str, tail: usize) -> String {
    let Some(end) = url.find("naddr1").map(|i| i + "naddr1".len()) else {
        return url.to_string();
    };
    if url.len() - end <= tail + 3 {
        return url.to_string();
    }
    format!("{}...{}", &url[..end], &url[url.len() - tail..])
}

pub(super) fn load_repo_data(repo: &Repo) -> Result<RepoData, Error> {
    let snapshot = repo.snapshot()?;
    let worktree = repo.workdir().map(Path::to_path_buf);
    let tree = build_tree_items(&snapshot.entries);

    Ok(RepoData {
        tree,
        entries: snapshot.entries,
        readme_path: snapshot.readme_path,
        readme: snapshot.readme,
        worktree,
        branches: snapshot.branches,
        tags: snapshot.tags,
        current_branch: snapshot.current_branch,
        head_commit: snapshot.head_commit,
    })
}

/// The announcement's name or ID for announced repositories, the directory name for local ones.
pub fn repo_display_name(store: &RepoStore) -> SharedString {
    if store.addr().is_none() {
        return store
            .path
            .as_ref()
            .map(|path| {
                SharedString::from(
                    path.file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_else(|| path.display().to_string()),
                )
            })
            .unwrap_or_default();
    }

    store
        .announcement
        .as_ref()
        .map(|announcement| {
            announcement
                .name
                .as_deref()
                .map(SharedString::from)
                .unwrap_or_else(|| SharedString::from(announcement.id.clone()))
        })
        .unwrap_or_default()
}

/// The owner and identifier a local repository is already bound to.
pub(super) fn bound_repo_label(
    binding: &Nip34Binding,
    directory: &Entity<ProfileDirectory>,
    cx: &App,
) -> AnyElement {
    let profile = binding.owner.map(|pk| directory.read(cx).profile(&pk, cx));

    h_flex()
        .flex_shrink_0()
        .gap_2()
        .text_sm()
        .child("Initialized by")
        .text_color(cx.theme().colors.muted_foreground)
        .when_some(profile, |this, profile| {
            let name = profile.name();
            this.child(
                h_flex()
                    .gap_1()
                    .text_color(cx.theme().colors.foreground)
                    .child(
                        Avatar::new(profile.public_key())
                            .name(name.clone())
                            .picture(profile.picture())
                            .small(),
                    )
                    .child(name),
            )
        })
        .when_some(binding.identifier.as_deref(), |this, ident| {
            this.child(
                div()
                    .text_xs()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(SharedString::from(format!("/{ident}"))),
            )
        })
        .into_any_element()
}

/// Computes a source hash from the given text.
pub(super) fn source_hash(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

pub(super) fn preview_spinner() -> AnyElement {
    v_flex()
        .size_full()
        .items_center()
        .justify_center()
        .child(gpui_native_kit::spinner::Spinner::new(
            "repo-preview-spinner",
        ))
        .into_any_element()
}

/// The kit grammar for a path; only four languages are supported.
pub(super) fn code_language(path: &str) -> gpui_native_kit::editor::highlight::Language {
    use gpui_native_kit::editor::highlight::Language;
    match Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .as_deref()
    {
        Some("rs") => Language::Rust,
        Some("ts" | "mts" | "cts" | "tsx") => Language::TypeScript,
        Some("js" | "mjs" | "cjs" | "jsx") => Language::JavaScript,
        Some("py") => Language::Python,
        _ => Language::Plain,
    }
}

/// Whether a file path has a markdown extension.
pub(super) fn is_markdown_path(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            matches!(
                ext.to_ascii_lowercase().as_str(),
                "md" | "markdown" | "mdown" | "mkdn"
            )
        })
}

pub(super) fn diff_rows(file: &FileDiff) -> Vec<DiffRow> {
    let mut rows = Vec::new();
    for (hunk_ix, hunk) in file.hunks.iter().enumerate() {
        rows.push(DiffRow::Hunk {
            old_start: hunk.old_start,
            old_lines: hunk.old_lines,
            new_start: hunk.new_start,
            new_lines: hunk.new_lines,
        });
        rows.extend((0..hunk.lines.len()).map(|line| DiffRow::Line {
            hunk: hunk_ix,
            line,
        }));
    }
    rows
}

pub(super) fn render_diff_row(hunks: &[DiffHunk], row: DiffRow, cx: &App) -> AnyElement {
    match row {
        DiffRow::Hunk {
            old_start,
            old_lines,
            new_start,
            new_lines,
        } => div()
            .px_2()
            .w_full()
            .h(px(DIFF_ROW_HEIGHT))
            .font_family(cx.theme().mono_font)
            .text_xs()
            .bg(cx.theme().colors.muted)
            .border_y_1()
            .border_color(cx.theme().colors.border)
            .text_color(cx.theme().colors.muted_foreground)
            .child(SharedString::from(format!(
                "@@ -{},{} +{},{} @@",
                old_start, old_lines, new_start, new_lines
            )))
            .into_any_element(),
        DiffRow::Line { hunk, line } => render_diff_line(&hunks[hunk].lines[line], cx),
    }
}

fn render_diff_line(line: &DiffLine, cx: &App) -> AnyElement {
    let bg = match line.kind {
        DiffLineKind::Addition => Some(cx.theme().ramps.success.base.opacity(0.2)),
        DiffLineKind::Deletion => Some(cx.theme().ramps.destructive.base.opacity(0.2)),
        DiffLineKind::Context => None,
    };
    let gutter = cx.theme().colors.muted_foreground;

    h_flex()
        .w_full()
        .h(px(DIFF_ROW_HEIGHT))
        .font_family(cx.theme().mono_font)
        .text_xs()
        .when_some(bg, |this, bg| this.bg(bg))
        .child(
            div()
                .w(px(GUTTER_WIDTH))
                .flex_none()
                .pr_2()
                .text_right()
                .text_color(gutter)
                .child(line.old.map(|n| n.to_string()).unwrap_or_default()),
        )
        .child(
            div()
                .w(px(GUTTER_WIDTH))
                .flex_none()
                .pr_2()
                .text_right()
                .text_color(gutter)
                .child(line.new.map(|n| n.to_string()).unwrap_or_default()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_color(cx.theme().colors.foreground)
                .child(line.text.clone()),
        )
        .into_any_element()
}

pub(crate) fn commit_row(
    ix: usize,
    commit: &FileCommit,
    on_click: impl Fn(&mut Window, &mut gpui::App) + 'static,
    cx: &gpui::App,
) -> gpui::AnyElement {
    h_flex()
        .id(ix)
        .px_4()
        .h(px(COMMIT_ROW_HEIGHT))
        .w_full()
        .gap_3()
        .border_b_1()
        .border_color(cx.theme().colors.border)
        .hover(|this| this.bg(cx.theme().colors.muted_hover))
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_1()
                .justify_center()
                .child(
                    h_flex()
                        .gap_2()
                        .overflow_hidden()
                        .child(
                            div()
                                .font_family(cx.theme().mono_font)
                                .text_xs()
                                .text_color(cx.theme().colors.muted_foreground)
                                .child(commit.id.clone()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_sm()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .child(commit.summary.clone()),
                        ),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .text_xs()
                        .text_color(cx.theme().colors.muted_foreground)
                        .child(commit.author.clone())
                        .child(relative_time_secs(commit.time)),
                ),
        )
        .on_click(move |_, window, cx| on_click(window, cx))
        .into_any_element()
}

pub(super) fn sync_selector(
    select: &Entity<ComboboxState<SharedString>>,
    cached: &mut Vec<SharedString>,
    items: Vec<SharedString>,
    selected: Option<SharedString>,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    let items_changed = *cached != items;

    let selection_changed = selected
        .as_ref()
        .is_some_and(|value| select.read(cx).selected() != Some(value));

    if !items_changed && !selection_changed {
        return false;
    }

    select.update(cx, |state, cx| {
        if items_changed {
            state.set_options(
                items
                    .iter()
                    .map(|item| (item.clone(), item.clone()))
                    .collect(),
                cx,
            );
        }
        if let Some(value) = selected
            && (items_changed || selection_changed)
        {
            state.set_selected(Some(value), window, cx);
        }
    });
    *cached = items;

    true
}

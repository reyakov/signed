use std::path::{Path, PathBuf};

use anyhow::Error;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use metadata::ProfileDirectory;
use nostr::prelude::{RelayUrl, ToBech32};
use signed_core::{Announcement, RepoAddr};
use signed_git::{DiffHunk, DiffLine, DiffLineKind, FileCommit, FileDiff, Repo, WorktreeSnapshot};
use signed_state::{Nip34Binding, RepoListStore, RepoStore};
use ui::avatar::Avatar;
use ui::clipboard::CopyRow;
use ui::combobox::ComboboxState;
use ui::common::{Sizable as _, StyledExt as _, h_flex, v_flex};
use ui::icons::{Icon, IconRole};
use ui::theme::ActiveTheme as _;

use super::RepoDetailView;
use ui::tree::TreeItemSeed;

const GUTTER_WIDTH: f32 = 44.;
pub const DIFF_ROW_HEIGHT: f32 = 20.;

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

impl DiffRow {
    /// Renders one diff row against the file's hunks.
    pub(super) fn render(self, hunks: &[DiffHunk], wrap: bool, cx: &App) -> AnyElement {
        match self {
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
            DiffRow::Line { hunk, line } => {
                match hunks.get(hunk).and_then(|h| h.lines.get(line)) {
                    Some(line) => render_diff_line(line, wrap, cx),
                    // A stale row index renders as a blank row rather than panicking.
                    None => div().w_full().h(px(DIFF_ROW_HEIGHT)).into_any_element(),
                }
            }
        }
    }
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

/// Sorted relative paths of a worktree snapshot.
pub(crate) fn sorted_worktree_paths(entries: &[PathBuf]) -> Vec<String> {
    let mut paths: Vec<String> = entries
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect();
    paths.sort();
    paths
}

/// The announcement's name, falling back to its id.
pub fn announcement_display_name(announcement: &Announcement) -> SharedString {
    announcement
        .name
        .as_deref()
        .map(SharedString::from)
        .unwrap_or_else(|| SharedString::from(announcement.id.clone()))
}

/// The first eight hex characters of an object id, for row labels.
pub fn short_id(hex: &str) -> SharedString {
    SharedString::from(hex.get(..8).unwrap_or(hex).to_owned())
}

/// The co-author names credited in a commit body, deduplicated in order.
pub(super) fn co_authors(description: &str) -> Vec<SharedString> {
    let mut names: Vec<SharedString> = Vec::new();
    for line in description.lines() {
        let trimmed = line.trim();
        if !trimmed.to_lowercase().starts_with("co-authored-by:") {
            continue;
        }
        let name = trimmed
            .split_once(':')
            .map(|(_, value)| value.trim())
            .unwrap_or_default()
            .split('<')
            .next()
            .unwrap_or_default()
            .trim();
        if !name.is_empty() && !names.iter().any(|existing| existing.as_ref() == name) {
            names.push(SharedString::from(name.to_owned()));
        }
    }
    names
}

/// Snapshots the worktree and derives the explorer's tree rows and paths.
pub(super) fn snapshot_worktree(
    worktree: PathBuf,
) -> Result<(WorktreeSnapshot, Vec<TreeItemSeed>, Vec<String>), Error> {
    let snapshot = Repo::open(&worktree)?.snapshot()?;
    let tree = TreeItemSeed::from_paths(&snapshot.entries);
    let paths = sorted_worktree_paths(&snapshot.entries);
    Ok((snapshot, tree, paths))
}

pub(super) fn clone_group(label: &str, id: SharedString, command: &SharedString) -> AnyElement {
    v_flex()
        .gap_1()
        .min_w_0()
        .child(
            div()
                .text_xs()
                .font_semibold()
                .child(SharedString::from(label)),
        )
        .child(CopyRow::new(id, command.clone()))
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

/// The display name of the announcement addressing `addr`, falling back to its identifier.
pub(crate) fn upstream_display_name(
    addr: &RepoAddr,
    announcements: &[Announcement],
) -> SharedString {
    announcements
        .iter()
        .find(|a| a.addr() == *addr)
        .map(|a| {
            a.name
                .as_deref()
                .map(SharedString::from)
                .unwrap_or_else(|| SharedString::from(a.id.clone()))
        })
        .unwrap_or_else(|| SharedString::from(addr.identifier().to_owned()))
}

pub(super) fn fork_row(
    announcement: &Announcement,
    cx: &mut Context<RepoDetailView>,
) -> Option<AnyElement> {
    let upstream = announcement.upstream.as_ref()?;

    let (label, clickable) = match &upstream.addr {
        Some(addr) => {
            let name =
                upstream_display_name(addr, &RepoListStore::global(cx).read(cx).announcements);
            (SharedString::from(format!("Forked from {name}")), true)
        }
        None => (SharedString::from(upstream.display().as_str()), false),
    };

    let row = h_flex()
        .gap_1()
        .min_w_0()
        .text_base()
        .text_color(cx.theme().colors.muted_foreground)
        .child(Icon::named(IconRole::GitBranch).small())
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

pub(super) fn load_repo_data(repo: &Repo) -> Result<RepoData, Error> {
    let snapshot = repo.snapshot()?;
    let worktree = repo.workdir().map(Path::to_path_buf);
    let tree = TreeItemSeed::from_paths(&snapshot.entries);

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
        .map(announcement_display_name)
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
        .text_base()
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
                    .font_medium()
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

/// The kit grammar for a path; only four languages are supported.
pub(super) fn code_language(path: &str) -> ui::highlight::Language {
    use ui::highlight::Language;
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

fn render_diff_line(line: &DiffLine, wrap: bool, cx: &App) -> AnyElement {
    let bg = match line.kind {
        DiffLineKind::Addition => Some(cx.theme().ramps.success.base.opacity(0.2)),
        DiffLineKind::Deletion => Some(cx.theme().ramps.destructive.base.opacity(0.2)),
        DiffLineKind::Context => None,
    };
    let gutter = cx.theme().colors.muted_foreground;

    h_flex()
        .w_full()
        .when(wrap, |row| row.min_h(px(DIFF_ROW_HEIGHT)))
        .when(!wrap, |row| row.h(px(DIFF_ROW_HEIGHT)))
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
                .text_color(cx.theme().colors.foreground)
                .when(!wrap, |content| {
                    content.overflow_hidden().whitespace_nowrap()
                })
                .child(line.text.clone()),
        )
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

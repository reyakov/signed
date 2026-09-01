use std::collections::HashMap;
use std::path::{Path, PathBuf};

use assets::CustomIconName;
use gpui::prelude::*;
use gpui::{
    Anchor, AnyElement, App, ClipboardItem, DismissEvent, ElementId, Entity, Focusable,
    SharedString, StyleRefinement, Window, div, px,
};
use gpui_base::{Button as BaseButton, Popover, Selectable, StyledExt};
use gpui_component::clipboard::Clipboard;
use gpui_component::list::ListItem;
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use gpui_component::tooltip::Tooltip;
use gpui_component::tree::{TreeEntry, TreeItem};
use gpui_component::{ActiveTheme, Icon, IconName, Sizable, h_flex, v_flex};
use nostr::nips::nip19::{Nip19Coordinate, ToBech32};
use signed_core::{Announcement, RepoStatus};
use signed_git::{DiffHunk, DiffLine, DiffLineKind, FileDiff};

/// A `Send` file-tree node: the tree is built on a background thread and
/// converted into [`TreeItem`]s (which hold `Rc` state,
/// so they cannot cross threads) on the main thread.
pub(super) struct TreeItemSeed {
    /// Path of the node, relative to the worktree root.
    id: String,
    /// File or directory name.
    label: String,
    children: Vec<TreeItemSeed>,
}

/// Convert tree seeds into [`TreeItem`]s, expanding every folder
/// when `expand_folders` is set.
///
/// The commit diff explorer shows only changed files,
/// which is typically a handful of paths, so its folders start expanded;
/// the worktree explorer starts collapsed instead.
pub(super) fn tree_items(seeds: Vec<TreeItemSeed>, expand_folders: bool) -> Vec<TreeItem> {
    fn convert(seed: TreeItemSeed, expand_folders: bool) -> TreeItem {
        let mut item = TreeItem::new(seed.id, seed.label);
        if expand_folders && !seed.children.is_empty() {
            item = item.expanded(true);
        }
        item.children = seed
            .children
            .into_iter()
            .map(|seed| convert(seed, expand_folders))
            .collect();
        item
    }

    seeds
        .into_iter()
        .map(|seed| convert(seed, expand_folders))
        .collect()
}

/// One row of a file tree: icon + name, indented by depth.
/// Clicking a file runs `on_click`; folders expand/collapse via the tree itself.
pub(super) fn tree_row<F>(ix: usize, entry: &TreeEntry, selected: bool, on_click: F) -> ListItem
where
    F: Fn(&mut Window, &mut App) + 'static,
{
    let item = entry.item();
    let is_folder = entry.is_folder();

    let icon = if is_folder {
        if entry.is_expanded() {
            IconName::FolderOpen
        } else {
            IconName::FolderClosed
        }
    } else {
        IconName::File
    };

    ListItem::new(ix)
        .pl(px(8.) + px(14.) * entry.depth() as f32)
        .selected(selected)
        .child(
            h_flex()
                .gap_2()
                .overflow_hidden()
                .child(Icon::new(icon).small())
                .child(div().text_sm().text_ellipsis().child(item.label.clone())),
        )
        .on_click(move |_event, window, cx| {
            // Folders expand/collapse via the tree itself.
            if is_folder {
                return;
            }
            on_click(window, cx);
        })
}

/// Build nested tree items from a flat, sorted (dirs-first) entry list.
///
/// Returns [`TreeItemSeed`]s so the build can run off the main thread; a
/// worktree walk can yield tens of thousands of entries. Nodes live in an
/// arena and parents are found via a path -> index map, which keeps the
/// build linear in the number of path components.
pub(super) fn build_tree_items(entries: &[PathBuf]) -> Vec<TreeItemSeed> {
    // Node indices by full path, for O(1) parent lookup while inserting.
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut nodes: Vec<(String, String, Vec<usize>)> = Vec::new();
    let mut roots: Vec<usize> = Vec::new();

    for entry in entries {
        let mut parent: Option<usize> = None;
        let mut path = String::new();
        for part in entry.components() {
            let label = part.as_os_str().to_string_lossy().into_owned();
            path = if path.is_empty() {
                label.clone()
            } else {
                format!("{path}/{label}")
            };
            let ix = *index.entry(path.clone()).or_insert_with(|| {
                let ix = nodes.len();
                nodes.push((path.clone(), label.clone(), Vec::new()));
                match parent {
                    Some(parent) => nodes[parent].2.push(ix),
                    None => roots.push(ix),
                }
                ix
            });
            parent = Some(ix);
        }
    }

    fn assemble(ix: usize, nodes: &[(String, String, Vec<usize>)]) -> TreeItemSeed {
        let (id, label, children) = &nodes[ix];
        TreeItemSeed {
            id: id.clone(),
            label: label.clone(),
            children: children
                .iter()
                .map(|child| assemble(*child, nodes))
                .collect(),
        }
    }

    roots.iter().map(|root| assemble(*root, &nodes)).collect()
}

/// The markdown fence language for a file path, or `None` for plain text.
///
/// Names are chosen so `gpui_component`'s highlighter can resolve them
/// (`highlighter::Language::from_name` accepts short aliases such as `rs` and `js`).
pub(super) fn code_language(path: &str) -> Option<&'static str> {
    let name = Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();

    // Some common files are recognized by name rather than extension.
    match name {
        "Makefile" | "makefile" => return Some("make"),
        "CMakeLists.txt" => return Some("cmake"),
        _ => {}
    }

    let ext = Path::new(path).extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "rs" => "rust",
        "toml" => "toml",
        "json" | "jsonc" => "json",
        "py" => "python",
        "js" | "mjs" | "cjs" => "javascript",
        "ts" | "mts" | "cts" => "typescript",
        "tsx" | "jsx" => "tsx",
        "go" => "go",
        "c" | "h" => "c",
        "cc" | "cpp" | "cxx" | "hh" | "hpp" | "hxx" => "cpp",
        "cs" => "csharp",
        "java" => "java",
        "kt" | "kts" | "ktm" => "kotlin",
        "swift" => "swift",
        "php" | "phtml" => "php",
        "rb" => "ruby",
        "sh" | "bash" | "zsh" => "bash",
        "yml" | "yaml" => "yaml",
        "css" | "scss" | "sass" => "css",
        "html" | "htm" => "html",
        "lua" => "lua",
        "sql" => "sql",
        "proto" | "protobuf" => "proto",
        "cmake" => "cmake",
        "zig" => "zig",
        "ex" | "exs" => "elixir",
        "graphql" | "gql" => "graphql",
        "diff" | "patch" => "diff",
        "svelte" => "svelte",
        "astro" => "astro",
        "scala" => "scala",
        _ => return None,
    })
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

/// A centered muted placeholder message.
pub(super) fn placeholder(message: &str, cx: &App) -> AnyElement {
    v_flex()
        .size_full()
        .items_center()
        .justify_center()
        .p_4()
        .child(
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(message.to_string()),
        )
        .into_any_element()
}

/// The status badge shown next to an issue or pull request: icon + colored square,
/// with a tooltip describing the status.
pub(super) fn status_badge(status: RepoStatus, cx: &App) -> AnyElement {
    let (icon, label, tooltip, bg, fg) = match status {
        RepoStatus::Open => (
            CustomIconName::GitIssueDone,
            "open",
            "Issue is open",
            cx.theme().primary,
            cx.theme().primary_foreground,
        ),
        RepoStatus::Closed => (
            CustomIconName::GitIssueClosed,
            "closed",
            "Issue is closed",
            cx.theme().danger,
            cx.theme().danger_foreground,
        ),
        RepoStatus::Draft => (
            CustomIconName::GitIssueOngoing,
            "draft",
            "Issue is draft",
            cx.theme().accent,
            cx.theme().accent_foreground,
        ),
        RepoStatus::Applied => (
            CustomIconName::GitIssueOpen,
            "applied",
            "Issue is completed",
            cx.theme().secondary,
            cx.theme().secondary_foreground,
        ),
    };

    v_flex()
        .id(label)
        .flex_shrink_0()
        .size_7()
        .items_center()
        .justify_center()
        .rounded(cx.theme().radius)
        .bg(bg)
        .child(Icon::new(icon).small().text_color(fg))
        .tooltip(move |window, cx| Tooltip::new(tooltip).build(window, cx))
        .into_any_element()
}

/// A split dropdown button built on `gpui_base::Popover`: an action element
/// with a separate caret trigger that opens a [`PopupMenu`].
///
/// The action and the caret are caller-supplied elements, so the look stays
/// in the application; this component only owns the popover wiring.
#[derive(IntoElement)]
pub(super) struct BaseDropdownButton {
    id: ElementId,
    style: StyleRefinement,
    anchor: Anchor,
    action: Option<AnyElement>,
    caret: Option<CaretBuilder>,
    menu: Option<MenuBuilder>,
}

type MenuBuilder =
    Box<dyn Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static>;
type CaretBuilder = Box<dyn FnOnce(bool, &Window, &App) -> AnyElement>;

impl BaseDropdownButton {
    pub(super) fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            style: StyleRefinement::default(),
            anchor: Anchor::TopRight,
            action: None,
            caret: None,
            menu: None,
        }
    }

    /// The action half of the button. It keeps its own icon, label, tooltip
    /// and click handler.
    pub(super) fn action(mut self, action: impl IntoElement + 'static) -> Self {
        self.action = Some(action.into_any_element());
        self
    }

    /// The menu built by `builder` — the same signature as gpui-component's
    /// `DropdownButton::dropdown_menu`, so existing menu code keeps working.
    pub(super) fn dropdown_menu(
        mut self,
        builder: impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static,
    ) -> Self {
        self.menu = Some(Box::new(builder));
        self
    }

    /// Which corner of the caret the menu anchors to. Defaults to
    /// [`Anchor::TopRight`], so the menu's right edge lines up with the
    /// caret's.
    #[allow(dead_code)] // API knob; current call sites use the default anchor.
    pub(super) fn anchor(mut self, anchor: impl Into<Anchor>) -> Self {
        self.anchor = anchor.into();
        self
    }
}

impl Styled for BaseDropdownButton {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

/// Holds the [`PopupMenu`] entity of one popover between renders. Dismissal
/// drops it, so the menu is rebuilt with fresh items on the next open.
#[derive(Default)]
struct DropdownMenuState {
    menu: Option<Entity<PopupMenu>>,
}

impl RenderOnce for BaseDropdownButton {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        debug_assert!(
            self.menu.is_some(),
            "a BaseDropdownButton needs a `dropdown_menu`"
        );

        // The popover needs its own id: both the container and the popover register keyed state on this window.
        let popover_id = SharedString::from(format!("{}-popover", self.id));
        let anchor = self.anchor;
        let menu_state =
            window.use_keyed_state(popover_id.clone(), cx, |_, _| DropdownMenuState::default());

        let caret = self.caret.unwrap_or_else(|| {
            let id = popover_id.clone();
            Box::new(move |is_open, _, cx| {
                let caret = default_caret(id.clone(), cx);
                let selected = caret.is_selected();
                caret.selected(selected || is_open).into_any_element()
            })
        });

        h_flex()
            .id(self.id)
            .refine_style(&self.style)
            .gap_0p5()
            .when_some(self.action, |this, action| this.child(action))
            .when_some(self.menu, |this, builder| {
                this.child(
                    Popover::new(popover_id)
                        .anchor(anchor)
                        // The menu dismisses itself on outside click or Escape;
                        // the subscription below closes the popover along with it.
                        .overlay_closable(false)
                        .trigger_with(caret)
                        .content(
                            move |_, window, cx| match menu_state.read(cx).menu.clone() {
                                Some(menu) => menu,
                                None => {
                                    let menu = PopupMenu::build(window, cx, |menu, window, cx| {
                                        builder(menu, window, cx)
                                    });
                                    menu_state
                                        .update(cx, |state, _| state.menu = Some(menu.clone()));
                                    menu.focus_handle(cx).focus(window, cx);

                                    let popover_state = cx.entity();
                                    window
                                        .subscribe(&menu, cx, {
                                            let menu_state = menu_state.clone();
                                            move |_, _: &DismissEvent, window, cx| {
                                                popover_state.update(cx, |state, cx| {
                                                    state.dismiss(window, cx);
                                                });
                                                menu_state.update(cx, |state, _| {
                                                    state.menu = None;
                                                });
                                            }
                                        })
                                        .detach();

                                    menu.clone()
                                }
                            },
                        ),
                )
            })
    }
}

/// The default caret: a chevron button the height of a medium button, tinted
/// by the theme, with hover and menu-open states.
fn default_caret(id: impl Into<ElementId>, cx: &App) -> BaseButton {
    BaseButton::new(id)
        .h(px(32.))
        .px_1p5()
        .text_color(cx.theme().muted_foreground)
        .hover(|style| style.bg(cx.theme().secondary_hover))
        .styles(|this| {
            this.selected(|style| style.bg(cx.theme().secondary_active))
                .disabled(|style| style.opacity(0.5))
        })
        .child(Icon::new(IconName::ChevronDown).xsmall())
}

pub(super) struct ShareTargets {
    /// NIP-19 `naddr1...` of the announcement (with its announced relays).
    pub(super) naddr: String,
    /// Hex ID of the announcement event itself.
    pub(super) event_id: String,
    /// NIP-34 coordinate `30617:<pubkey>:<repo-id>`.
    pub(super) coordinate: String,
    /// `https://gitworkshop.dev/<naddr>`
    pub(super) gitworkshop: String,
    /// `https://ditto.pub/<naddr>`
    pub(super) ditto: String,
}

impl ShareTargets {
    pub(super) fn from_announcement(announcement: &Announcement) -> Self {
        let addr = announcement.addr();
        let coordinate = addr.to_string();
        let naddr = Nip19Coordinate::new(addr, announcement.relays.iter().cloned())
            .to_bech32()
            .expect("a complete coordinate always encodes to naddr");

        Self {
            naddr: naddr.clone(),
            event_id: announcement.event_id.to_bech32().unwrap(),
            coordinate,
            gitworkshop: format!("https://gitworkshop.dev/{naddr}"),
            ditto: format!("https://ditto.pub/{naddr}"),
        }
    }

    /// The share dropdown menu: one row per target, each showing a compact
    /// label while the copy button (and row click) copy the full value.
    pub(super) fn menu(&self, menu: PopupMenu) -> PopupMenu {
        menu.min_w(px(340.))
            .item(share_menu_row(
                "copy-gitworkshop",
                "GitWorkshop",
                truncate_naddr_link(&self.gitworkshop, 4),
                self.gitworkshop.clone(),
            ))
            .item(share_menu_row(
                "copy-ditto",
                "Ditto",
                truncate_naddr_link(&self.ditto, 4),
                self.ditto.clone(),
            ))
            .item(share_menu_row(
                "copy-event-id",
                "Event ID",
                middle_truncate(&self.event_id, 10, 10),
                self.event_id.clone(),
            ))
            .item(share_menu_row(
                "copy-coordinate",
                "Coordinate",
                middle_truncate(&self.coordinate, 10, 10),
                self.coordinate.clone(),
            ))
    }
}

/// One row of the share menu: a small title above the compact label, with
/// a copy button that flips to a check while the value is on the clipboard.
/// Clicking the row copies and dismisses the menu; the copy button stops
/// propagation so the menu stays open. Both copy `copy`, never the label.
pub(super) fn share_menu_row(
    id: &'static str,
    title: &'static str,
    label: String,
    copy: String,
) -> PopupMenuItem {
    let row_copy = copy.clone();
    PopupMenuItem::element(move |_window, _cx| {
        let button_copy = copy.clone();
        h_flex()
            .flex_1()
            .gap_2()
            .items_end()
            .child(
                h_flex()
                    .flex_1()
                    .gap_1()
                    .text_xs()
                    .child(div().flex_shrink_0().w_20().font_semibold().child(title))
                    .child(div().flex_1().text_ellipsis().child(label.clone())),
            )
            .child(Clipboard::new(id).tooltip("Copy").value(button_copy))
    })
    .on_click(move |_, _, cx| {
        cx.write_to_clipboard(ClipboardItem::new_string(row_copy.clone()));
    })
}

/// `[head chars]...[tail chars]` middle truncation; the value is left alone
/// when it is too short for the ellipsis to save space.
pub(super) fn middle_truncate(value: &str, head: usize, tail: usize) -> String {
    let len = value.chars().count();
    if len <= head + tail + 3 {
        return value.to_string();
    }
    let head: String = value.chars().take(head).collect();
    let tail: String = value.chars().skip(len - tail).collect();
    format!("{head}...{tail}")
}

/// Shorten an naddr link to `<url>/naddr1...[last tail chars]`, e.g.
/// `https://gitworkshop.dev/naddr1...abcd`. Only the label is shortened;
/// the value to be copied stays the full URL.
fn truncate_naddr_link(url: &str, tail: usize) -> String {
    let Some(end) = url.find("naddr1").map(|i| i + "naddr1".len()) else {
        return url.to_string();
    };
    if url.len() - end <= tail + 3 {
        return url.to_string();
    }
    format!("{}...{}", &url[..end], &url[url.len() - tail..])
}

/// Width of one line-number gutter in a diff row.
pub(super) const GUTTER_WIDTH: f32 = 44.;
/// Height of one row in a virtual diff list.
pub(super) const DIFF_ROW_HEIGHT: f32 = 20.;

/// One row of a virtual diff list: a hunk header, or a line of a hunk.
/// Shared by the commit diff and pull request diff viewers.
#[derive(Clone, Copy)]
pub(super) enum DiffRow {
    Hunk {
        old_start: u32,
        old_lines: u32,
        new_start: u32,
        new_lines: u32,
    },
    /// Line `line` of hunk `hunk` of the selected file's diff.
    Line { hunk: usize, line: usize },
}

/// The rows of `file`'s diff: one header row per hunk, then its lines.
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

/// One row of the virtual diff list: a hunk header or a single line.
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
            .font_family(cx.theme().mono_font_family.clone())
            .text_xs()
            .bg(cx.theme().muted)
            .border_y(px(1.))
            .border_color(cx.theme().border)
            .text_color(cx.theme().muted_foreground)
            .child(SharedString::from(format!(
                "@@ -{},{} +{},{} @@",
                old_start, old_lines, new_start, new_lines
            )))
            .into_any_element(),
        DiffRow::Line { hunk, line } => render_diff_line(&hunks[hunk].lines[line], cx),
    }
}

/// One diff line: old and new line numbers in gutters, then the content,
/// tinted by kind (addition / deletion / context).
pub(super) fn render_diff_line(line: &DiffLine, cx: &App) -> AnyElement {
    let bg = match line.kind {
        DiffLineKind::Addition => Some(cx.theme().success.opacity(0.2)),
        DiffLineKind::Deletion => Some(cx.theme().danger.opacity(0.2)),
        DiffLineKind::Context => None,
    };
    let gutter = cx.theme().muted_foreground;

    // Fixed height and nowrap: the virtual list assumes every row has
    // the same height, so long lines are clipped instead of wrapped.
    h_flex()
        .w_full()
        .h(px(DIFF_ROW_HEIGHT))
        .items_center()
        .font_family(cx.theme().mono_font_family.clone())
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
                .text_color(cx.theme().foreground)
                .child(line.text.clone()),
        )
        .into_any_element()
}

/// Find a tree item by id, searching into nested children.
pub(super) fn find_item<'a>(items: &'a [TreeItem], id: Option<&str>) -> Option<&'a TreeItem> {
    let id = id?;
    items.iter().find_map(|item| {
        if item.id.as_ref() == id {
            Some(item)
        } else {
            find_item(&item.children, Some(id))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_nested_tree_from_flat_entries() {
        let entries = vec![
            PathBuf::from("src"),
            PathBuf::from("src/lib.rs"),
            PathBuf::from("README.md"),
            PathBuf::from("docs/guide.md"),
        ];

        let items = build_tree_items(&entries);

        // Input order is preserved (dirs-first, as produced by worktree_entries).
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].label, "src");
        assert_eq!(items[0].id, "src");
        assert_eq!(items[0].children.len(), 1);
        assert_eq!(items[0].children[0].label, "lib.rs");
        assert_eq!(items[0].children[0].id, "src/lib.rs");

        assert_eq!(items[1].label, "README.md");
        assert_eq!(items[1].id, "README.md");

        assert_eq!(items[2].label, "docs");
        assert_eq!(items[2].children[0].label, "guide.md");
        assert_eq!(items[2].children[0].id, "docs/guide.md");
    }

    #[test]
    fn tree_builder_handles_deep_nesting() {
        let entries = vec![
            PathBuf::from("a"),
            PathBuf::from("a/b"),
            PathBuf::from("a/b/c.txt"),
        ];

        let items = build_tree_items(&entries);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].children[0].id, "a/b");
        assert_eq!(items[0].children[0].children[0].id, "a/b/c.txt");
    }

    #[test]
    fn tree_builder_merges_shared_prefixes() {
        // File children of a directory arrive after other directories'
        // entries (the worktree list is dirs-first globally); the shared
        // prefix must still resolve to one node.
        let entries = vec![
            PathBuf::from("a/x.txt"),
            PathBuf::from("b/y.txt"),
            PathBuf::from("a/z.txt"),
        ];

        let items = build_tree_items(&entries);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].label, "a");
        assert_eq!(items[0].children.len(), 2);
        assert_eq!(items[1].label, "b");
    }

    #[test]
    fn tree_seeds_convert_to_tree_items() {
        let entries = vec![
            PathBuf::from("src"),
            PathBuf::from("src/main.rs"),
            PathBuf::from("README.md"),
        ];

        let items: Vec<TreeItem> = tree_items(build_tree_items(&entries), false);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].label, "src");
        assert_eq!(items[0].children.len(), 1);
        assert_eq!(items[0].children[0].label, "main.rs");
    }

    #[test]
    fn code_language_maps_extensions_and_names() {
        assert_eq!(code_language("src/main.rs"), Some("rust"));
        assert_eq!(code_language("Cargo.toml"), Some("toml"));
        assert_eq!(code_language("app.js"), Some("javascript"));
        assert_eq!(code_language("index.tsx"), Some("tsx"));
        assert_eq!(code_language("Makefile"), Some("make"));
        assert_eq!(code_language("CMakeLists.txt"), Some("cmake"));
        assert_eq!(code_language("data.csv"), None);
        assert_eq!(code_language("LICENSE"), None);
        assert_eq!(code_language("README.md"), None);
    }

    #[test]
    fn middle_truncates_long_values_only() {
        assert_eq!(
            middle_truncate(
                "a008def15796fba9a0d6fab04e8fd57089285d9fd505da5a83fe8aad57a3564d",
                10,
                10,
            ),
            "a008def157...ad57a3564d"
        );
        assert_eq!(
            middle_truncate(
                "30617:a008def15796fba9a0d6fab04e8fd57089285d9fd505da5a83fe8aad57a3564d:ngit",
                10,
                10
            ),
            "30617:a008...3564d:ngit"
        );
        // Too short to save space with the ellipsis: left alone.
        assert_eq!(middle_truncate("short", 10, 10), "short");
    }

    #[test]
    fn naddr_link_keeps_url_and_tail() {
        assert_eq!(
            truncate_naddr_link("https://gitworkshop.dev/naddr1qqqxyzabc1234", 4),
            "https://gitworkshop.dev/naddr1...1234"
        );
        // No naddr1 prefix: unchanged.
        assert_eq!(
            truncate_naddr_link("https://example.com/x", 4),
            "https://example.com/x"
        );
    }

    #[test]
    fn base_dropdown_button_builder_state() {
        let button = BaseDropdownButton::new("issues")
            .action(div())
            .anchor(Anchor::BottomLeft)
            .dropdown_menu(|menu, _, _| menu);

        assert!(button.action.is_some());
        assert!(button.caret.is_some());
        assert!(button.menu.is_some());
        assert_eq!(button.anchor, Anchor::BottomLeft);
    }
}

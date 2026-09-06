use std::collections::HashMap;
use std::path::{Path, PathBuf};

use assets::CustomIconName;
use gpui::prelude::*;
use gpui::{AnyElement, App, Entity, SharedString, div, px};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::combobox::{Caret, ComboboxTriggerContext};
use gpui_component::input::{Textarea, TextareaState};
use gpui_component::menu::PopupMenu;
use gpui_component::searchable_list::SearchableVec;
use gpui_component::tag::Tag;
use gpui_component::tree::TreeItem;
use gpui_component::{ActiveTheme, Icon, Sizable, StyledExt, h_flex, v_flex};
use nostr::nips::nip19::{Nip19Coordinate, ToBech32};
use nostr::prelude::{Event, EventId, PublicKey};
use signed_core::Announcement;
use signed_git::{DiffHunk, DiffLine, DiffLineKind, FileDiff};
use signed_state::{ProfileStore, RepoStore};
use signed_ui::{UserAvatar, menu_copy_row, middle_truncate};
use utils::relative_time;

pub(super) struct TreeItemSeed {
    /// Path of the node, relative to the worktree root.
    id: String,
    /// File or directory name.
    label: String,
    children: Vec<TreeItemSeed>,
}

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

/// Build nested tree items from a flat entry list sorted dirs-first.
pub(super) fn build_tree_items(entries: &[PathBuf]) -> Vec<TreeItemSeed> {
    // Node indices by full path, so parents resolve in constant time while inserting.
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

pub(super) struct ShareTargets {
    /// NIP-19 `naddr1...` of the announcement, with its announced relays.
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

    /// The share dropdown menu, one row per target.
    ///
    /// Each shows a compact label, the copy button and row click copy the full value.
    pub(super) fn menu(&self, menu: PopupMenu) -> PopupMenu {
        menu.min_w(px(340.))
            .item(menu_copy_row(
                "copy-gitworkshop",
                "GitWorkshop",
                truncate_naddr_link(&self.gitworkshop, 4),
                self.gitworkshop.clone(),
            ))
            .item(menu_copy_row(
                "copy-ditto",
                "Ditto",
                truncate_naddr_link(&self.ditto, 4),
                self.ditto.clone(),
            ))
            .item(menu_copy_row(
                "copy-event-id",
                "Event ID",
                middle_truncate(&self.event_id, 10, 10),
                self.event_id.clone(),
            ))
            .item(menu_copy_row(
                "copy-coordinate",
                "Coordinate",
                middle_truncate(&self.coordinate, 10, 10),
                self.coordinate.clone(),
            ))
    }
}

/// Shorten an naddr link to `<url>/naddr1...[last tail chars]`.
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

/// One row of a virtual diff list, a hunk header or a line of a hunk.
///
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

/// The rows of `file`'s diff, one header row per hunk then its lines.
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

/// One row of the virtual diff list, a hunk header or a single line.
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

/// One diff line, old and new line numbers in the gutters.
///
/// The content is tinted by kind, addition, deletion or context.
pub(super) fn render_diff_line(line: &DiffLine, cx: &App) -> AnyElement {
    let bg = match line.kind {
        DiffLineKind::Addition => Some(cx.theme().success.opacity(0.2)),
        DiffLineKind::Deletion => Some(cx.theme().danger.opacity(0.2)),
        DiffLineKind::Context => None,
    };
    let gutter = cx.theme().muted_foreground;

    // Fixed height and nowrap, the virtual list assumes every row has the same height.
    // Long lines are clipped instead of wrapped.
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

/// The root issue events of a repo store, for the shared detail sections.
pub(super) fn issue_roots(store: &RepoStore) -> &[Event] {
    &store.issues
}

/// The root pull request events of a repo store, for the shared detail sections.
pub(super) fn pr_roots(store: &RepoStore) -> &[Event] {
    &store.pull_requests
}

/// The trigger body of the branch/tag selectors.
///
/// The kind icon, the selection or placeholder, and the caret.
/// `Combobox` replaces its default trigger entirely,
/// the only way to show an icon inside it.
pub(super) fn ref_selector_trigger(
    ctx: &ComboboxTriggerContext<SearchableVec<SharedString>>,
    icon: CustomIconName,
    cx: &App,
) -> AnyElement {
    let muted = cx.theme().muted_foreground;

    h_flex()
        .w_full()
        .min_w_0()
        .gap_1()
        .items_center()
        .child(Icon::new(icon).small().flex_shrink_0())
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .text_ellipsis()
                .whitespace_nowrap()
                .when(ctx.selection().is_empty(), |this| this.text_color(muted))
                .child(
                    ctx.selection()
                        .first()
                        .map(|(_, item)| item.clone())
                        .or_else(|| ctx.placeholder().cloned())
                        .unwrap_or_default(),
                ),
        )
        .child(Caret::new(ctx.size()).text_color(muted))
        .into_any_element()
}

/// Section heading of a detail sidebar, shared by the issue and PR panels.
pub(super) fn sidebar_title(text: &str, cx: &App) -> AnyElement {
    div()
        .text_xs()
        .font_semibold()
        .text_color(cx.theme().muted_foreground)
        .child(text.to_string())
        .into_any_element()
}

/// Right sidebar with participants and labels of a root event, issue or PR.
pub(super) fn sidebar_section(
    store: &Entity<RepoStore>,
    id: EventId,
    roots: fn(&RepoStore) -> &[Event],
    top_gap: bool,
    cx: &App,
) -> AnyElement {
    let store = store.read(cx);
    let Some(root) = roots(store).iter().find(|event| event.id == id) else {
        // The caller bails out when the root is missing.
        return div().into_any_element();
    };
    let profile_store = ProfileStore::global(cx);

    // Participants, the root author plus everyone who commented.
    let mut participants: Vec<PublicKey> = vec![root.pubkey];
    participants.extend(store.comments_of(&root.id).map(|comment| comment.pubkey));
    participants.sort_by_key(PublicKey::to_hex);
    participants.dedup();

    // Labels are NIP-34 `t` hashtag tags on the event.
    let labels: Vec<String> = root.tags.hashtags().map(|tag| tag.to_string()).collect();

    v_flex()
        .w(px(240.))
        .h_full()
        .flex_none()
        .px_4()
        .gap_4()
        .border_l(px(1.))
        .border_color(cx.theme().sidebar_border)
        .child(
            v_flex()
                .when(top_gap, |this| this.mt_4())
                .gap_2()
                .child(sidebar_title("Participants", cx))
                .children(participants.iter().map(|pubkey| {
                    let profile = profile_store.read(cx).get(pubkey);
                    let name = profile.name();
                    let picture = profile.picture();

                    h_flex()
                        .gap_1()
                        .items_center()
                        .child(UserAvatar::new(name.clone()).picture(picture))
                        .child(div().text_sm().truncate().text_ellipsis().child(name))
                        .into_any_element()
                })),
        )
        .child(
            v_flex()
                .gap_2()
                .child(sidebar_title("Labels", cx))
                .map(|this| {
                    if labels.is_empty() {
                        this.child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child("None yet."),
                        )
                    } else {
                        this.child(h_flex().gap_1().children({
                            let mut items = vec![];

                            for label in labels.iter() {
                                items.push(
                                    Tag::secondary()
                                        .outline()
                                        .xsmall()
                                        .child(SharedString::from(label)),
                                );
                            }

                            items
                        }))
                    }
                }),
        )
        .into_any_element()
}

/// The comments on a root event, issue or PR, one card per comment.
pub(super) fn comments_section(store: &Entity<RepoStore>, root: EventId, cx: &App) -> AnyElement {
    let store = store.read(cx);
    let comments: Vec<&Event> = store.comments_of(&root).collect();
    let title = SharedString::from(format!("Discussions {}", comments.len()));

    v_flex()
        .gap_4()
        .child(div().text_xs().font_semibold().child(title))
        .children(comments.iter().map(|comment| {
            let profile = ProfileStore::global(cx).read(cx).get(&comment.pubkey);
            let author = profile.name();
            let picture = profile.picture();
            let age = relative_time(comment.created_at);
            let content = SharedString::from(comment.content.as_str());

            v_flex()
                .gap_1()
                .p_3()
                .border_1()
                .border_color(cx.theme().border)
                .rounded(cx.theme().radius)
                .child(
                    h_flex()
                        .gap_2()
                        .text_sm()
                        .child(
                            h_flex()
                                .gap_1()
                                .child(UserAvatar::new(author.clone()).picture(picture))
                                .child(author),
                        )
                        .child(
                            div()
                                .text_color(cx.theme().muted_foreground)
                                .child("commented"),
                        )
                        .child(
                            div()
                                .text_color(cx.theme().muted_foreground)
                                .child(SharedString::from(age)),
                        ),
                )
                .child(div().text_sm().child(content))
        }))
        .into_any_element()
}

/// The comment form posting to an issue or PR root event.
///
/// `roots` selects the root's list within the store, issues or pull requests.
pub(super) fn comment_form(
    store: &Entity<RepoStore>,
    root: EventId,
    roots: fn(&RepoStore) -> &[Event],
    comment_input: &Entity<TextareaState>,
    button_id: &'static str,
    cx: &App,
) -> AnyElement {
    let comment_input = comment_input.clone();
    let store = store.clone();

    v_flex()
        .gap_2()
        .child(
            Textarea::new(&comment_input)
                .h_24()
                .text_color(cx.theme().muted_foreground)
                .bg(cx.theme().muted),
        )
        .child(
            h_flex()
                .justify_between()
                .child(
                    h_flex()
                        .gap_1()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(Icon::new(CustomIconName::Markdown).small())
                        .child("Markdown is supported"),
                )
                .child(
                    Button::new(button_id)
                        .primary()
                        .label("Comment")
                        .tooltip("Post comment")
                        .on_click(move |_event, window, cx| {
                            let content = comment_input.read(cx).value().trim().to_string();
                            if content.is_empty() {
                                return;
                            }
                            let Some(root) = roots(store.read(cx))
                                .iter()
                                .find(|event| event.id == root)
                                .cloned()
                            else {
                                return;
                            };
                            store.update(cx, |store, cx| {
                                store.comment(&root, content, cx);
                            });
                            comment_input.update(cx, |input, cx| {
                                input.set_value("", window, cx);
                            });
                        }),
                ),
        )
        .into_any_element()
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

        // Input order is preserved, dirs-first as produced by worktree_entries.
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
        // File children of a directory arrive after other directories' entries.
        // The worktree list is dirs-first globally.
        // The shared prefix must still resolve to one node.
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
    fn naddr_link_keeps_url_and_tail() {
        assert_eq!(
            truncate_naddr_link("https://gitworkshop.dev/naddr1qqqxyzabc1234", 4),
            "https://gitworkshop.dev/naddr1...1234"
        );
        // Without the naddr1 prefix, unchanged.
        assert_eq!(
            truncate_naddr_link("https://example.com/x", 4),
            "https://example.com/x"
        );
    }
}

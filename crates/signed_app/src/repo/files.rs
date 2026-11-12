use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Component, Path, PathBuf};

use anyhow::Error;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Task, WeakEntity, Window, div, px,
};
use gpui_native_kit::common::{h_flex, v_flex};
use gpui_native_kit::data::tree::{Tree, TreeItem};
use gpui_native_kit::editor::code::CodeViewState;
use gpui_native_kit::editor::markdown::MarkdownViewState;
use gpui_native_kit::icons::{Icon, IconRole};
use gpui_native_kit::theme::{ActiveTheme as _, TextRole, Typeset as _};
use signed_git::{FileCommit, Repo, WorktreeSnapshot};

use super::helpers::{code_language, is_markdown_path, preview_spinner, source_hash};
use crate::shared::tree::{TreeItemSeed, TreeRow, collect_expanded, flatten};
use crate::shared::widgets::empty_state;

const TREE_WIDTH: f32 = 240.;
const MAX_PREVIEW_BYTES: usize = 1024 * 1024;
const MAX_PREVIEWED_FILES: usize = 32;
const MAX_PREVIEW_CACHE_BYTES: usize = 8 * 1024 * 1024;

enum FileContent {
    Text(String),
    Binary,
    TooLarge,
    Failed(String),
}

struct MarkdownView {
    /// `None` means the repository README.
    path: Option<SharedString>,
    state: Entity<MarkdownViewState>,
    /// Hash of the source, so the same document is not re-parsed on a refresh.
    source_hash: u64,
}

struct CodeView {
    /// Source path, relative to the worktree root.
    path: SharedString,
    state: Entity<CodeViewState>,
    /// Hash of the source, so the same document is not re-parsed on a refresh.
    source_hash: u64,
}

pub struct RepoFilesView {
    seeds: Vec<TreeItemSeed>,
    expanded: HashSet<String>,
    rows: Vec<TreeRow>,
    selected_index: Option<usize>,
    worktree: Option<PathBuf>,
    worktree_paths: Vec<String>,
    md: Option<MarkdownView>,
    code: Option<CodeView>,
    readme_name: Option<SharedString>,
    selected_file: Option<SharedString>,
    files: HashMap<String, FileContent>,
    file_order: VecDeque<String>,
    preview_bytes: usize,
    loading_files: HashSet<String>,
    commits: HashMap<String, FileCommit>,
    pending_commits: Vec<String>,
    loading_commits: bool,
    tasks: Vec<Task<Result<(), Error>>>,
}

impl RepoFilesView {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        Self {
            seeds: Vec::new(),
            expanded: HashSet::new(),
            rows: Vec::new(),
            selected_index: None,
            worktree: None,
            worktree_paths: Vec::new(),
            md: None,
            code: None,
            readme_name: None,
            selected_file: None,
            files: HashMap::new(),
            file_order: VecDeque::new(),
            preview_bytes: 0,
            loading_files: HashSet::new(),
            commits: HashMap::new(),
            pending_commits: Vec::new(),
            loading_commits: false,
            tasks: Vec::new(),
        }
    }

    pub fn set_worktree(&mut self, path: PathBuf) {
        self.worktree = Some(path);
    }

    pub fn apply_entries(
        &mut self,
        tree: Vec<TreeItemSeed>,
        paths: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        self.worktree_paths = paths;
        self.seeds = tree;
        let folder_ids = collect_expanded(&self.seeds);
        self.expanded.retain(|id| folder_ids.contains(id));
        self.rebuild_rows();
        cx.notify();
    }

    fn rebuild_rows(&mut self) {
        self.rows = flatten(&self.seeds, &self.expanded);
        self.selected_index = self
            .selected_file
            .as_ref()
            .and_then(|path| self.rows.iter().position(|row| row.id == path.as_ref()));
    }

    fn toggle_folder(&mut self, id: &str) {
        if !self.expanded.remove(id) {
            self.expanded.insert(id.to_owned());
        }
        self.rebuild_rows();
    }

    /// Point the README pane at `path`/`bytes`, or clear it when absent.
    pub fn set_readme(
        &mut self,
        path: Option<PathBuf>,
        bytes: Option<Vec<u8>>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some((path, bytes)) = path.zip(bytes) else {
            let changed = self.readme_name.is_some() || self.md.is_some();
            self.readme_name = None;
            self.md = None;
            return changed;
        };

        let name: SharedString = path.to_string_lossy().into();
        let mut changed = self.readme_name.as_ref() != Some(&name);
        self.readme_name = Some(name);
        self.load_commit(&path.to_string_lossy(), cx);

        if let Ok(text) = String::from_utf8(bytes) {
            changed |= self.set_markdown(None, &text, cx);
        }

        changed
    }

    /// Drop every cached preview and the README, e.g. on a branch switch.
    pub fn clear_previews(&mut self) {
        self.selected_file = None;
        self.selected_index = None;
        self.files.clear();
        self.file_order.clear();
        self.preview_bytes = 0;
        self.loading_files.clear();
        self.commits.clear();
        self.pending_commits.clear();
        self.loading_commits = false;
        self.md = None;
        self.code = None;
        self.readme_name = None;
    }

    /// Refresh after the mirror caught up with the remote.
    pub fn catch_up(
        &mut self,
        snapshot: &WorktreeSnapshot,
        tree: Vec<TreeItemSeed>,
        paths: Vec<String>,
        cx: &mut Context<Self>,
    ) -> bool {
        let mut changed = false;

        if paths != self.worktree_paths {
            self.apply_entries(tree, paths, cx);
            changed = true;
        }

        let present: HashSet<String> = snapshot
            .entries
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect();

        let mut previewed: Vec<String> = Vec::new();
        previewed.extend(self.files.keys().cloned());
        previewed.extend(self.selected_file.clone().map(|path| path.to_string()));

        if let Some(path) = self.md.as_ref().and_then(|md| md.path.clone()) {
            previewed.push(path.to_string());
        }
        if let Some(path) = self.code.as_ref().map(|code| code.path.clone()) {
            previewed.push(path.to_string());
        }

        previewed.sort();
        previewed.dedup();

        for path in previewed {
            if !present.contains(&path) {
                self.drop_preview_of(&path);
                changed = true;
            }
        }

        if self.selected_file.is_none() {
            changed |= self.set_readme(snapshot.readme_path.clone(), snapshot.readme.clone(), cx);
        }

        changed
    }

    fn pane_title(&self) -> SharedString {
        self.selected_file
            .clone()
            .or_else(|| self.readme_name.clone())
            .unwrap_or_else(|| "Overview".into())
    }

    fn activate_row(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.rows.get(index) else {
            return;
        };
        let id = row.id.clone();
        let expandable = row.expandable;

        self.selected_index = Some(index);

        if expandable {
            self.toggle_folder(&id);
        } else {
            self.open_file(&id, window, cx);
        }
        cx.notify();
    }

    fn render_tree_column(
        &self,
        rows: Vec<TreeRow>,
        selected: Option<usize>,
        view: WeakEntity<Self>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let change_view = view.clone();
        let disclosure_view = view;

        v_flex()
            .h_full()
            .w(px(TREE_WIDTH))
            .p_2()
            .flex_none()
            .border_r_1()
            .border_color(cx.theme().colors.border)
            .child(
                div().flex_1().min_h_0().child(
                    Tree::new("repo-tree")
                        .selected(selected)
                        .items(move |_, _| {
                            rows.iter()
                                .map(|row| {
                                    let item = TreeItem::new(row.id.clone(), row.label.clone())
                                        .depth(row.depth)
                                        .expandable(row.expandable)
                                        .expanded(row.expanded);

                                    if row.expandable {
                                        item.icon(Icon::named(IconRole::Folder))
                                    } else {
                                        item
                                    }
                                })
                                .collect()
                        })
                        .on_change({
                            let view = change_view;
                            move |index, window, cx| {
                                if let Some(index) = index
                                    && let Err(error) = view.update(cx, |this, cx| {
                                        this.activate_row(index, window, cx);
                                    })
                                {
                                    log::warn!("failed to activate tree row: {error}");
                                }
                            }
                        })
                        .on_disclosure({
                            let view = disclosure_view;
                            move |index, _, cx| {
                                if let Err(error) = view.update(cx, |this, cx| {
                                    let id = this.rows.get(index).map(|row| row.id.clone());
                                    if let Some(id) = id {
                                        this.toggle_folder(&id);
                                        cx.notify();
                                    }
                                }) {
                                    log::warn!("failed to toggle folder: {error}");
                                }
                            }
                        }),
                ),
            )
    }

    fn render_content_column(
        &self,
        pane_title: SharedString,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let body: AnyElement = if let Some(path) = self.selected_file.clone() {
            match self.files.get(path.as_ref()) {
                Some(FileContent::Text(_)) => {
                    if is_markdown_path(path.as_ref()) {
                        self.markdown_element(Some(path.as_ref()))
                    } else {
                        self.code_element(path.as_ref())
                    }
                }
                Some(FileContent::Binary) => empty_state("Binary file - preview not supported"),
                Some(FileContent::TooLarge) => empty_state("File is too large to preview"),
                Some(FileContent::Failed(message)) => empty_state(message),
                None => preview_spinner(),
            }
        } else if self.readme_name.is_some() {
            self.markdown_element(None)
        } else {
            empty_state("No README found")
        };

        let commit = match &self.selected_file {
            Some(path) => self.commits.get(path.as_ref()),
            None => self
                .readme_name
                .as_ref()
                .and_then(|name| self.commits.get(name.as_ref())),
        };

        v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(
                h_flex()
                    .px_3()
                    .h_9()
                    .gap_2()
                    .bg(cx.theme().colors.muted)
                    .border_b_1()
                    .border_color(cx.theme().colors.border)
                    .text_color(cx.theme().colors.muted_foreground)
                    .child(
                        div()
                            .typeset(TextRole::Caption2, cx)
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(pane_title),
                    )
                    .when_some(commit, |this, commit| {
                        this.child(
                            h_flex()
                                .flex_1()
                                .gap_1()
                                .child(
                                    div()
                                        .typeset(TextRole::Caption, cx)
                                        .font_family(cx.theme().mono_font)
                                        .child(commit.id.clone()),
                                )
                                .child(
                                    div()
                                        .max_w(px(250.))
                                        .typeset(TextRole::Caption, cx)
                                        .text_ellipsis()
                                        .whitespace_nowrap()
                                        .child(commit.summary.clone()),
                                ),
                        )
                    }),
            )
            .child(div().flex_1().min_h_0().child(body))
    }

    fn set_markdown(
        &mut self,
        path: Option<SharedString>,
        text: &str,
        cx: &mut Context<Self>,
    ) -> bool {
        let hash = source_hash(text);

        if let Some(md) = &self.md
            && md.path == path
            && md.source_hash == hash
        {
            return false;
        }

        if let Some(md) = self.md.take()
            && md.path == path
        {
            md.state.update(cx, |state, cx| {
                state.set_source(text, cx);
            });
            self.md = Some(MarkdownView {
                path,
                state: md.state,
                source_hash: hash,
            });
            return true;
        }

        let state = cx.new(|_| MarkdownViewState::new("repo-readme", text));
        self.md = Some(MarkdownView {
            path,
            state,
            source_hash: hash,
        });

        true
    }

    fn markdown_element(&self, path: Option<&str>) -> AnyElement {
        let Some(md) = &self.md else {
            return preview_spinner();
        };

        let ready = match path {
            Some(path) => md.path.as_deref() == Some(path),
            None => md.path.is_none(),
        };

        if !ready {
            return preview_spinner();
        }

        div()
            .p_4()
            .size_full()
            .overflow_hidden()
            .child(md.state.clone())
            .into_any_element()
    }

    fn set_code(&mut self, path: SharedString, text: &str, cx: &mut Context<Self>) {
        let hash = source_hash(text);
        let language = code_language(path.as_ref());

        if let Some(code) = &self.code
            && code.path == path
            && code.source_hash == hash
        {
            return;
        }

        if let Some(code) = self.code.take()
            && code.path == path
        {
            code.state.update(cx, |state, cx| {
                state.set_source(text, language, cx);
            });
            self.code = Some(CodeView {
                path,
                state: code.state,
                source_hash: hash,
            });
            return;
        }

        let state = cx.new(|cx| CodeViewState::new("repo-code", text, language, cx));
        self.code = Some(CodeView {
            path,
            state,
            source_hash: hash,
        });
    }

    fn code_element(&self, path: &str) -> AnyElement {
        let Some(code) = &self.code else {
            return preview_spinner();
        };
        if code.path.as_ref() != path {
            return preview_spinner();
        }

        div()
            .size_full()
            .overflow_hidden()
            .child(code.state.clone())
            .into_any_element()
    }

    fn open_file(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.selected_file = Some(path.into());

        if self.files.contains_key(path) {
            if let Some(FileContent::Text(text)) = self.files.get(path) {
                let text = text.clone();
                if is_markdown_path(path) {
                    if self.md.as_ref().map(|md| md.path.as_deref()) != Some(Some(path)) {
                        self.set_markdown(Some(path.into()), &text, cx);
                    }
                } else if self.code.as_ref().map(|code| code.path.as_str()) != Some(path) {
                    self.set_code(path.into(), &text, cx);
                }
            }
            cx.notify();
            return;
        }
        if self.loading_files.contains(path) {
            cx.notify();
            return;
        }

        let rel = Path::new(path);
        let unsafe_path = rel.is_absolute()
            || rel.components().any(|c| {
                matches!(
                    c,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            });

        let Some(worktree) = self.worktree.clone() else {
            return;
        };

        if unsafe_path {
            return;
        }

        self.loading_files.insert(path.to_string());
        let path = path.to_string();

        self.load_commit(&path, cx);

        let task: Task<Result<(), Error>> = cx.spawn_in(window, async move |this, cx| {
            let path_for_read = path.clone();
            let content = cx
                .background_spawn(async move {
                    let full = worktree.join(&path_for_read);

                    let metadata = match std::fs::metadata(&full) {
                        Ok(metadata) => metadata,
                        Err(error) => return Err(anyhow::anyhow!("{}", error)),
                    };

                    if metadata.len() > MAX_PREVIEW_BYTES as u64 {
                        return Ok(FileContent::TooLarge);
                    }

                    let bytes = match std::fs::read(&full) {
                        Ok(bytes) => bytes,
                        Err(error) => return Err(anyhow::anyhow!("{}", error)),
                    };

                    match String::from_utf8(bytes) {
                        Ok(text) => Ok(FileContent::Text(text)),
                        Err(_) => Ok(FileContent::Binary),
                    }
                })
                .await;

            this.update_in(cx, |this, _window, cx| {
                this.loading_files.remove(&path);

                match content {
                    Ok(kind) => {
                        if let FileContent::Text(text) = &kind {
                            if is_markdown_path(&path) {
                                let same = this.md.as_ref().map(|md| md.path.as_deref())
                                    == Some(Some(path.as_str()));
                                if !same {
                                    this.set_markdown(Some(path.clone().into()), text, cx);
                                }
                            } else {
                                let same = this.code.as_ref().map(|code| code.path.as_str())
                                    == Some(path.as_str());
                                if !same {
                                    this.set_code(path.clone().into(), text, cx);
                                }
                            }
                            this.preview_bytes += text.len();
                        }
                        this.files.insert(path.clone(), kind);
                        this.file_order.push_back(path);
                        this.evict_previews();
                    }
                    Err(error) => {
                        this.files
                            .insert(path, FileContent::Failed(error.to_string()));
                    }
                }
                cx.notify();
            })?;

            Ok(())
        });

        self.tasks.push(task);
    }

    fn drop_preview_of(&mut self, path: &str) {
        if let Some(FileContent::Text(text)) = self.files.remove(path) {
            self.preview_bytes -= text.len();
        }

        self.commits.remove(path);

        if self.selected_file.as_deref() == Some(path) {
            self.selected_file = None;
        }
        if self.md.as_ref().and_then(|md| md.path.as_deref()) == Some(path) {
            self.md = None;
        }
        if self.code.as_ref().map(|code| code.path.as_ref()) == Some(path) {
            self.code = None;
        }
    }

    fn evict_previews(&mut self) {
        while (self.files.len() > MAX_PREVIEWED_FILES
            || self.preview_bytes > MAX_PREVIEW_CACHE_BYTES)
            && self.file_order.len() > 1
        {
            let Some(path) = self.file_order.pop_front() else {
                break;
            };

            if Some(path.as_str()) == self.selected_file.as_deref() {
                self.file_order.push_back(path);
                continue;
            }

            if let Some(FileContent::Text(text)) = self.files.remove(&path) {
                self.preview_bytes -= text.len();
            }
            if self.md.as_ref().and_then(|md| md.path.as_deref()) == Some(path.as_str()) {
                self.md = None;
            }
            if self
                .code
                .as_ref()
                .is_some_and(|code| code.path.as_ref() == path.as_str())
            {
                self.code = None;
            }
            self.commits.remove(&path);
        }
    }

    fn load_commit(&mut self, path: &str, cx: &mut Context<Self>) {
        if self.commits.contains_key(path) || self.pending_commits.iter().any(|p| p == path) {
            return;
        }

        self.pending_commits.push(path.to_string());

        if !self.loading_commits {
            self.load_commits(cx);
        }
    }

    fn load_commits(&mut self, cx: &mut Context<Self>) {
        if self.pending_commits.is_empty() || self.loading_commits {
            return;
        }

        let Some(worktree) = self.worktree.clone() else {
            self.pending_commits.clear();
            return;
        };

        self.loading_commits = true;
        let paths = std::mem::take(&mut self.pending_commits);

        let task: Task<Result<(), Error>> = cx.spawn(async move |this, cx| {
            let rels: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
            let result = cx
                .background_spawn(async move {
                    Repo::open_cached(&worktree).and_then(|repo| repo.last_commits(&rels))
                })
                .await;

            this.update(cx, |this, cx| {
                this.loading_commits = false;

                if let Ok(found) = result {
                    for (path, commit) in found {
                        this.commits
                            .insert(path.to_string_lossy().into_owned(), commit);
                    }
                }

                if !this.pending_commits.is_empty() {
                    this.load_commits(cx);
                }

                cx.notify();
            })?;

            Ok(())
        });

        self.tasks.push(task);
    }
}

impl Render for RepoFilesView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.rows.clone();
        let selected = self.selected_index;
        let view = cx.entity().downgrade();
        let pane_title = self.pane_title();

        h_flex()
            .flex_1()
            .w_full()
            .overflow_hidden()
            .child(self.render_tree_column(rows, selected, view, cx))
            .child(self.render_content_column(pane_title, cx))
    }
}

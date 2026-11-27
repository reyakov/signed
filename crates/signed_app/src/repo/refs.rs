use gpui::{App, AppContext as _, Entity, SharedString, WeakEntity, Window};
use ui::combobox::{Combobox, ComboboxState};
use ui::icons::IconRole;

use super::{RefKind, RepoDetailView};

pub(super) struct RefSwitcher {
    pub(super) branch_select: Entity<ComboboxState<SharedString>>,
    pub(super) tag_select: Entity<ComboboxState<SharedString>>,
    ref_branches: Vec<SharedString>,
    ref_tags: Vec<SharedString>,
    pub(super) switching_ref: bool,
    /// The commit shown in place of the branch while the panel browses one.
    pub(super) commit_label: Option<SharedString>,
}

impl RefSwitcher {
    pub(super) fn new(view: WeakEntity<RepoDetailView>, window: &mut Window, cx: &mut App) -> Self {
        let branch_view = view.clone();
        let branch_select = cx.new(|cx| {
            ComboboxState::new(
                Combobox::new("branch-select", "Branch", Vec::new())
                    .placeholder("Branch")
                    .selected_icon(IconRole::GitBranch.into())
                    .on_change(move |value, window, cx| {
                        if let Some(name) = value
                            && let Err(error) = branch_view.update(cx, |this, cx| {
                                this.switch_ref(super::RefKind::Branch, name, window, cx);
                            })
                        {
                            log::warn!("failed to switch branch: {error}");
                        }
                    }),
                window,
                cx,
            )
        });

        let tag_view = view;
        let tag_select = cx.new(|cx| {
            ComboboxState::new(
                Combobox::new("tag-select", "Tag", Vec::new())
                    .placeholder("Tag")
                    .on_change(move |value, window, cx| {
                        if let Some(name) = value
                            && let Err(error) = tag_view.update(cx, |this, cx| {
                                this.switch_ref(super::RefKind::Tag, name, window, cx);
                            })
                        {
                            log::warn!("failed to switch tag: {error}");
                        }
                    }),
                window,
                cx,
            )
        });

        Self {
            branch_select,
            tag_select,
            ref_branches: Vec::new(),
            ref_tags: Vec::new(),
            switching_ref: false,
            commit_label: None,
        }
    }

    pub(super) fn set_branches(
        &mut self,
        branches: Vec<SharedString>,
        selected: Option<SharedString>,
        window: &mut Window,
        cx: &mut App,
    ) -> bool {
        // A shown commit keeps its label until a ref is picked.
        let selected = if self.commit_label.is_some() {
            None
        } else {
            selected
        };

        self.sync_select(RefKind::Branch, branches, selected, window, cx)
    }

    /// Pushes `items`/`selected` into a selector, reporting whether it changed.
    fn sync_select(
        &mut self,
        kind: RefKind,
        items: Vec<SharedString>,
        selected: Option<SharedString>,
        window: &mut Window,
        cx: &mut App,
    ) -> bool {
        let select = match kind {
            RefKind::Branch => self.branch_select.clone(),
            RefKind::Tag => self.tag_select.clone(),
        };

        let cached = match kind {
            RefKind::Branch => &mut self.ref_branches,
            RefKind::Tag => &mut self.ref_tags,
        };

        let select = &select;
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
                state.set_selected_silent(Some(value), window, cx);
            }
        });

        *cached = items;
        true
    }

    pub(super) fn set_tags(
        &mut self,
        tags: Vec<SharedString>,
        window: &mut Window,
        cx: &mut App,
    ) -> bool {
        self.sync_select(RefKind::Tag, tags, None, window, cx)
    }

    /// Shows `label` in the branch selector until a ref is picked.
    pub(super) fn show_commit(&mut self, label: SharedString, window: &mut Window, cx: &mut App) {
        self.commit_label = Some(label.clone());
        self.branch_select.update(cx, |state, cx| {
            state.set_display_label(label, window, cx);
        });
    }

    pub(super) fn restore_selection(
        &self,
        select: &Entity<ComboboxState<SharedString>>,
        previous: &Option<SharedString>,
        window: &mut Window,
        cx: &mut App,
    ) {
        select.update(cx, |state, cx| {
            state.set_selected_silent(previous.clone(), window, cx)
        });
    }
}

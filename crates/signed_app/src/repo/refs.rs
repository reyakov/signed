use gpui::{App, AppContext as _, Entity, SharedString, WeakEntity, Window};
use gpui_native_kit::combobox::{Combobox, ComboboxState};

use super::RepoDetailView;
use super::helpers::sync_selector;

pub(super) struct RefSwitcher {
    pub(super) branch_select: Entity<ComboboxState<SharedString>>,
    pub(super) tag_select: Entity<ComboboxState<SharedString>>,
    ref_branches: Vec<SharedString>,
    ref_tags: Vec<SharedString>,
    pub(super) switching_ref: bool,
}

impl RefSwitcher {
    pub(super) fn new(view: WeakEntity<RepoDetailView>, window: &mut Window, cx: &mut App) -> Self {
        let branch_view = view.clone();
        let branch_select = cx.new(|cx| {
            ComboboxState::new(
                Combobox::new("branch-select", "Branch", Vec::new())
                    .placeholder("Branch")
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
        }
    }

    pub(super) fn set_branches(
        &mut self,
        branches: Vec<SharedString>,
        selected: Option<SharedString>,
        window: &mut Window,
        cx: &mut App,
    ) -> bool {
        sync_selector(
            &self.branch_select,
            &mut self.ref_branches,
            branches,
            selected,
            window,
            cx,
        )
    }

    pub(super) fn set_tags(
        &mut self,
        tags: Vec<SharedString>,
        window: &mut Window,
        cx: &mut App,
    ) -> bool {
        sync_selector(&self.tag_select, &mut self.ref_tags, tags, None, window, cx)
    }

    pub(super) fn restore_selection(
        &self,
        select: &Entity<ComboboxState<SharedString>>,
        previous: &Option<SharedString>,
        window: &mut Window,
        cx: &mut App,
    ) {
        select.update(cx, |state, cx| {
            state.set_selected(previous.clone(), window, cx)
        });
    }
}

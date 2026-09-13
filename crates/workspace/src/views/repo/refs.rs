use gpui::prelude::*;
use gpui::{App, Entity, SharedString, Window};
use gpui_component::combobox::ComboboxState;
use gpui_component::searchable_list::SearchableVec;

pub(super) struct RefSwitcher {
    pub(super) branch_select: Entity<ComboboxState<SearchableVec<SharedString>>>,
    pub(super) tag_select: Entity<ComboboxState<SearchableVec<SharedString>>>,
    ref_branches: Vec<SharedString>,
    ref_tags: Vec<SharedString>,
    pub(super) switching_ref: bool,
}

impl RefSwitcher {
    pub(super) fn new(window: &mut Window, cx: &mut App) -> Self {
        let branch_select = cx.new(|cx| {
            ComboboxState::new(
                SearchableVec::new(Vec::<SharedString>::new()),
                Vec::new(),
                window,
                cx,
            )
            .searchable(true)
        });
        let tag_select = cx.new(|cx| {
            ComboboxState::new(
                SearchableVec::new(Vec::<SharedString>::new()),
                Vec::new(),
                window,
                cx,
            )
            .searchable(true)
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
        select: &Entity<ComboboxState<SearchableVec<SharedString>>>,
        previous: &Option<SharedString>,
        window: &mut Window,
        cx: &mut App,
    ) {
        select.update(cx, |state, cx| match previous {
            Some(value) => state.set_selected_values(std::slice::from_ref(value), window, cx),
            None => state.clear_selection(cx),
        });
    }
}

fn sync_selector(
    select: &Entity<ComboboxState<SearchableVec<SharedString>>>,
    cached: &mut Vec<SharedString>,
    items: Vec<SharedString>,
    selected: Option<SharedString>,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    let items_changed = *cached != items;

    let selection_changed = selected
        .as_ref()
        .is_some_and(|value| select.read(cx).selected_value().as_ref() != Some(value));

    if !items_changed && !selection_changed {
        return false;
    }

    select.update(cx, |state, cx| {
        if items_changed {
            state.set_items(SearchableVec::from(items.clone()), window, cx);
        }
        if let Some(value) = selected
            && (items_changed || selection_changed)
        {
            state.set_selected_values(std::slice::from_ref(&value), window, cx);
        }
    });
    *cached = items;

    true
}

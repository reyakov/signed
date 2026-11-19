use std::collections::HashMap;
use std::sync::Arc;

use gpui::{
    App, Context, IntoElement, ParentElement as _, Render, SharedString, Styled as _, Window, div,
};
use ui::common::v_flex;
use ui::layout::panel::{Panel, PanelView};
use ui::tabs::{Tab, Tabs, TabsVariant};

use crate::shared::log_dropped;

/// The repository shown in the main panel and the detail panels opened beside it.
#[derive(Default)]
pub struct Workspace {
    main: Option<SharedString>,
    main_order: Vec<SharedString>,
    main_views: HashMap<SharedString, Arc<dyn PanelView>>,
    details: Vec<SharedString>,
    detail_views: HashMap<SharedString, Arc<dyn PanelView>>,
    active_detail: Option<SharedString>,
}

impl Workspace {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn active_main(&self) -> Option<Arc<dyn PanelView>> {
        self.main
            .as_ref()
            .and_then(|id| self.main_views.get(id))
            .cloned()
    }

    pub fn active_main_id(&self) -> Option<&SharedString> {
        self.main.as_ref()
    }

    pub fn open_mains(&self) -> &[SharedString] {
        &self.main_order
    }

    /// The open main panel with this id.
    pub fn main_view(&self, id: &SharedString) -> Option<Arc<dyn PanelView>> {
        self.main_views.get(id).cloned()
    }

    pub fn active_detail(&self) -> Option<Arc<dyn PanelView>> {
        self.active_detail
            .as_ref()
            .and_then(|id| self.detail_views.get(id))
            .cloned()
    }

    pub fn has_main(&self, id: &SharedString) -> bool {
        self.main_views.contains_key(id)
    }

    pub fn has_details(&self) -> bool {
        !self.details.is_empty()
    }

    pub fn has_detail(&self, id: &SharedString) -> bool {
        self.detail_views.contains_key(id)
    }

    pub fn show_main(
        &mut self,
        id: SharedString,
        view: Arc<dyn PanelView>,
        cx: &mut Context<Self>,
    ) {
        if self.main.as_ref() != Some(&id) {
            self.clear_details();
        }
        if !self.main_views.contains_key(&id) {
            self.main_order.push(id.clone());
        }
        self.main_views.insert(id.clone(), view);
        self.main = Some(id);
        cx.notify();
    }

    pub fn activate_main(&mut self, id: &SharedString, cx: &mut Context<Self>) {
        if self.main.as_ref() != Some(id) {
            self.clear_details();
        }
        self.main = Some(id.clone());
        cx.notify();
    }

    pub fn activate_detail(&mut self, id: &SharedString, cx: &mut Context<Self>) {
        if self.detail_views.contains_key(id) {
            self.active_detail = Some(id.clone());
            cx.notify();
        }
    }

    pub fn show_detail(
        &mut self,
        id: SharedString,
        view: Arc<dyn PanelView>,
        cx: &mut Context<Self>,
    ) {
        if !self.details.contains(&id) {
            self.details.push(id.clone());
        }
        self.detail_views.insert(id.clone(), view);
        self.active_detail = Some(id);
        cx.notify();
    }

    pub fn close_detail(&mut self, id: &SharedString, cx: &mut Context<Self>) {
        let Some(index) = self.details.iter().position(|open| open == id) else {
            return;
        };
        self.close_detail_at(index, cx);
    }

    pub fn close_main_at(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.main_order.len() {
            return;
        }

        let id = self.main_order.remove(index);
        self.main_views.remove(&id);

        if self.main.as_ref() == Some(&id) {
            self.main = self
                .main_order
                .get(index)
                .or_else(|| {
                    index
                        .checked_sub(1)
                        .and_then(|prev| self.main_order.get(prev))
                })
                .cloned();
            self.clear_details();
        }

        cx.notify();
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.main = None;
        self.main_order.clear();
        self.main_views.clear();
        self.clear_details();
        cx.notify();
    }

    fn clear_details(&mut self) {
        self.details.clear();
        self.detail_views.clear();
        self.active_detail = None;
    }

    fn close_detail_at(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.details.len() {
            return;
        }

        let id = self.details.remove(index);
        self.detail_views.remove(&id);

        if self.active_detail.as_ref() == Some(&id) {
            self.active_detail = self
                .details
                .get(index)
                .or_else(|| index.checked_sub(1).and_then(|prev| self.details.get(prev)))
                .cloned();
        }

        cx.notify();
    }

    fn activate_detail_at(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(id) = self.details.get(index) else {
            return;
        };
        self.active_detail = Some(id.clone());
        cx.notify();
    }
}

impl Panel for Workspace {
    fn title(&self, _cx: &App) -> SharedString {
        SharedString::from("Workspace")
    }
}

impl Render for Workspace {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self
            .active_detail
            .as_ref()
            .and_then(|active| self.details.iter().position(|id| id == active))
            .unwrap_or(usize::MAX);

        let details: Vec<_> = self
            .details
            .iter()
            .map(|id| (id.clone(), self.detail_views.get(id).cloned()))
            .collect();

        let weak = cx.entity().downgrade();
        let close_weak = weak.clone();

        let tabs = Tabs::new("signed-detail-tabs")
            .variant(TabsVariant::Pill)
            .overflow_scroll(true)
            .selected(selected)
            .tabs(move |_, cx| {
                details
                    .iter()
                    .map(|(id, view)| match view {
                        Some(view) => view.tab(id.clone().into(), cx).closeable(true),
                        None => Tab::new(id.clone(), id.clone()).closeable(true),
                    })
                    .collect()
            })
            .on_change(move |index, _, cx| {
                log_dropped(weak.update(cx, |workspace, cx| {
                    workspace.activate_detail_at(index, cx);
                }));
            })
            .on_close(move |index, _, cx| {
                log_dropped(close_weak.update(cx, |workspace, cx| {
                    workspace.close_detail_at(index, cx);
                }));
            });

        let body = self.active_detail().map(|view| view.view());

        v_flex()
            .size_full()
            .gap_2()
            .child(tabs)
            .child(div().flex_1().min_h_0().w_full().children(body))
    }
}

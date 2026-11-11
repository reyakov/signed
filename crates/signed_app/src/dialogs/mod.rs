use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyView, App, Context, Entity, Global, IntoElement, ParentElement as _, Render, Styled as _,
    Window, div,
};

struct GlobalDialogHost(Entity<DialogHost>);

impl Global for GlobalDialogHost {}

/// Owns the modal currently mounted at the window root.
pub struct DialogHost {
    open: Option<AnyView>,
}

impl DialogHost {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        Self { open: None }
    }

    pub fn set_global(entity: Entity<Self>, cx: &mut App) {
        cx.set_global(GlobalDialogHost(entity));
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalDialogHost>().0.clone()
    }

    pub fn open(&mut self, view: AnyView, cx: &mut Context<Self>) {
        self.open = Some(view);
        cx.notify();
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.open = None;
        cx.notify();
    }
}

impl Render for DialogHost {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .absolute()
            .inset_0()
            .when_some(self.open.clone(), |host, view| host.child(view))
    }
}

use dock::{BasePanel, Panel, PanelEvent};
use gpui::{
    App, Context, EventEmitter, FocusHandle, Focusable, IntoElement, Render, SharedString, Window,
};
use signed_ui::placeholder;

/// A center panel reserving the place of a view that is not built yet.
pub struct PlaceholderPanel {
    name: &'static str,
    title: SharedString,
    focus_handle: FocusHandle,
}

impl PlaceholderPanel {
    pub(crate) fn new(name: &'static str, title: &'static str, cx: &mut Context<Self>) -> Self {
        Self {
            name,
            title: SharedString::from(title),
            focus_handle: cx.focus_handle(),
        }
    }
}

impl BasePanel for PlaceholderPanel {
    fn panel_name(&self) -> &'static str {
        self.name
    }
}

impl Panel for PlaceholderPanel {
    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.title.clone()
    }
}

impl EventEmitter<PanelEvent> for PlaceholderPanel {}

impl Focusable for PlaceholderPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for PlaceholderPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        placeholder("Nothing here yet.", cx)
    }
}

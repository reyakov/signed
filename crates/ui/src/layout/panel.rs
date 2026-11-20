//! The panel row: the window's main content, island-style over the app
//! background. It can split once into two half-width panels with a drag
//! divider between them; dragging the seam resizes the panels and a
//! double-click restores the half split.

use std::sync::Arc;

use gpui::{
    AnyElement, AnyView, App, Axis, Div, ElementId, Entity, IntoElement, ParentElement as _,
    Render, RenderOnce, SharedString, StyleRefinement, Styled as _, Window, div,
};

use crate::components::tabs::Tab;
use crate::layout::StyledExt as _;
use crate::layout::resizable::{Resizable, ResizableState};
use crate::theme::{ActiveTheme as _, RESIZE_HANDLE_WIDTH};

/// A user-defined panel the row can show.
#[allow(unused_variables)]
pub trait Panel: Render + 'static {
    /// The kind the panel belongs to; same-kind panels group in the tab bar.
    fn kind(&self, cx: &App) -> Option<SharedString> {
        None
    }

    /// The title shown in the panel's tab.
    fn title(&self, cx: &App) -> SharedString;

    /// The element drawn before the tab's title, e.g. an avatar or icon.
    fn prefix(&self, cx: &App) -> Option<AnyElement> {
        None
    }

    /// Whether the tab shows the loading spinner.
    fn loading(&self, cx: &App) -> bool {
        false
    }
}

/// An object-safe handle to a panel, so apps can hold panels by kind.
#[allow(unused_variables)]
pub trait PanelView: 'static {
    /// The panel's view.
    fn view(&self) -> AnyView;

    /// The panel's kind, read live from the panel.
    fn kind(&self, cx: &App) -> Option<SharedString> {
        None
    }

    /// The panel's title, read live from the panel.
    fn title(&self, cx: &App) -> SharedString;

    /// The panel's tab prefix, read live from the panel.
    fn prefix(&self, cx: &App) -> Option<AnyElement> {
        None
    }

    /// Whether the panel's tab shows the loading spinner.
    fn loading(&self, cx: &App) -> bool {
        false
    }

    /// Builds the panel's tab from its title, prefix, and loading state.
    fn tab(&self, key: ElementId, cx: &App) -> Tab {
        let mut tab = Tab::new(key, self.title(cx)).busy(self.loading(cx));
        if let Some(prefix) = self.prefix(cx) {
            tab = tab.lead(prefix);
        }
        tab
    }
}

impl<T: Panel> PanelView for Entity<T> {
    fn view(&self) -> AnyView {
        self.clone().into()
    }

    fn kind(&self, cx: &App) -> Option<SharedString> {
        self.read(cx).kind(cx)
    }

    fn title(&self, cx: &App) -> SharedString {
        self.read(cx).title(cx)
    }

    fn prefix(&self, cx: &App) -> Option<AnyElement> {
        self.read(cx).prefix(cx)
    }

    fn loading(&self, cx: &App) -> bool {
        self.read(cx).loading(cx)
    }
}

impl<P: PanelView + ?Sized> PanelView for Arc<P> {
    fn view(&self) -> AnyView {
        (**self).view()
    }

    fn kind(&self, cx: &App) -> Option<SharedString> {
        (**self).kind(cx)
    }

    fn title(&self, cx: &App) -> SharedString {
        (**self).title(cx)
    }

    fn prefix(&self, cx: &App) -> Option<AnyElement> {
        (**self).prefix(cx)
    }

    fn loading(&self, cx: &App) -> bool {
        (**self).loading(cx)
    }
}

/// The layout the cached panel content lays out with.
fn content_style() -> StyleRefinement {
    StyleRefinement::default().size_full()
}

/// The island chrome every panel paints.
fn island(panel: Div, cx: &App) -> Div {
    let theme = cx.theme();
    let colors = theme.colors;

    panel
        .size_full()
        .bg(colors.background)
        .border_1()
        .border_color(colors.border)
        .rounded(theme.radius.overlay)
        .overflow_hidden()
}

/// The panel row drawn under the titlebar.
#[derive(IntoElement)]
#[must_use]
pub struct PanelRow {
    id: ElementId,
    main: Option<AnyElement>,
    split: Option<AnyElement>,
    style: StyleRefinement,
}

impl PanelRow {
    /// Creates the panel row.
    pub fn new<I>(id: I) -> Self
    where
        I: Into<ElementId>,
    {
        Self {
            id: id.into(),
            main: None,
            split: None,
            style: StyleRefinement::default(),
        }
    }

    /// Sets the main panel's content.
    pub fn main<P>(mut self, main: P) -> Self
    where
        P: PanelView,
    {
        self.main = Some(main.view().cached(content_style()).into_any_element());
        self
    }

    /// Splits the row into two half-width panels.
    pub fn split<P>(mut self, panel: P) -> Self
    where
        P: PanelView,
    {
        self.split = Some(panel.view().cached(content_style()).into_any_element());
        self
    }
}

impl gpui::Styled for PanelRow {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for PanelRow {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let id = self.id.clone();
        let gutter = RESIZE_HANDLE_WIDTH;

        let main = island(
            div()
                .size_full()
                .child(self.main.unwrap_or_else(|| div().into_any_element())),
            cx,
        );

        let row = match self.split {
            Some(split) => {
                let sizes = window.with_element_namespace(id.clone(), |window| {
                    window
                        .use_keyed_state("panel-sizes", cx, |_, _| ResizableState::default())
                        .clone()
                });
                let extent = window.viewport_size().width - gutter * 2.0;

                if sizes.read(cx).sizes().is_empty() {
                    sizes.update(cx, |state, cx| {
                        state.set_pane_width(0, (extent - gutter) / 2.0, cx)
                    });
                }

                div().w_full().h_full().child(
                    Resizable::new((id.clone(), "split"), sizes)
                        .axis(Axis::Horizontal)
                        .pane(main)
                        .pane(island(div().size_full().child(split), cx))
                        .h_full(),
                )
            }
            None => div().w_full().h_full().child(main),
        };

        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .p_2()
            .pt_1()
            .refine_style(&self.style)
            .child(row)
    }
}

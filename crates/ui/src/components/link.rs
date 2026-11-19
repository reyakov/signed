//! Renders a link that opens a URL or fires a click handler.
//!
//! # Examples
//!
//! ```no_run
//! use ui::link::Link;
//!
//! let docs = Link::new("docs-link")
//!     .label("Documentation")
//!     .href("https://example.com/docs");
//!
//! let settings = Link::new("settings-link")
//!     .label("Open settings")
//!     .on_click(|_event, _window, _cx| println!("open settings"));
//! ```

use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, ClickEvent, ElementId, InteractiveElement as _, IntoElement, ParentElement as _,
    RenderOnce, Role, SharedString, StatefulInteractiveElement as _, StyleRefinement, Styled as _,
    Window,
};

use crate::components::common::{Sizable, StyledExt as _, focus_ring, h_flex};
use crate::theme::{ActiveTheme as _, DISABLED_OPACITY, Size};

/// The handler called with the [`gpui::ClickEvent`], replacing the default `href` opening.
type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// A hyperlink that opens a URL or fires a click handler.
#[derive(IntoElement)]
#[must_use]
pub struct Link {
    id: ElementId,
    label: Option<SharedString>,
    href: Option<SharedString>,
    size: Size,
    disabled: bool,
    a11y_label: Option<SharedString>,
    on_click: Option<ClickHandler>,
    style: StyleRefinement,
}

impl Link {
    /// Creates a link without a label or target.
    pub fn new<I>(id: I) -> Self
    where
        I: Into<ElementId>,
    {
        Self {
            id: id.into(),
            label: None,
            href: None,
            size: Size::default(),
            disabled: false,
            a11y_label: None,
            on_click: None,
            style: StyleRefinement::default(),
        }
    }

    /// Sets the visible label.
    pub fn label<L>(mut self, label: L) -> Self
    where
        L: Into<SharedString>,
    {
        self.label = Some(label.into());
        self
    }

    /// Sets the URL the link opens through the platform.
    pub fn href<U>(mut self, href: U) -> Self
    where
        U: Into<SharedString>,
    {
        self.href = Some(href.into());
        self
    }

    /// Renders the link inert, unfocusable, and dimmed; disabled always wins.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Overrides the accessibility label, which defaults to the visible label.
    pub fn accessibility_label<L>(mut self, label: L) -> Self
    where
        L: Into<SharedString>,
    {
        self.a11y_label = Some(label.into());
        self
    }

    /// Replaces the default `href` opening with this handler.
    pub fn on_click<F>(mut self, handler: F) -> Self
    where
        F: Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    {
        self.on_click = Some(Box::new(handler));
        self
    }
}

impl gpui::Styled for Link {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl Sizable for Link {
    fn apply_size(&mut self, size: Size) {
        self.size = size;
    }
}

impl RenderOnce for Link {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let colors = theme.colors;
        let ramps = theme.ramps;
        let metrics = theme.sizes.resolve(self.size);

        let on_click = self.on_click;
        let href = self.href;
        let a11y_label = self.a11y_label.or_else(|| self.label.clone());

        h_flex()
            .id(self.id)
            .text_size(metrics.text_size)
            .text_color(ramps.primary.base)
            .underline()
            .refine_style(&self.style)
            .role(Role::Link)
            .when_some(a11y_label, |link, label| link.aria_label(label))
            .when(self.disabled, |link| link.opacity(DISABLED_OPACITY))
            .when(!self.disabled, |link| {
                link.cursor_pointer()
                    .focusable()
                    .tab_stop(true)
                    .hover(move |style| style.text_color(ramps.primary.hover))
                    .focus_visible(|style| style.shadow(focus_ring(colors.ring)))
                    .on_click(move |event, window, cx| match on_click.as_ref() {
                        Some(on_click) => on_click(event, window, cx),
                        None => {
                            if let Some(href) = href.as_ref() {
                                cx.open_url(href);
                            }
                        }
                    })
            })
            .when_some(self.label, |link, label| link.child(label))
    }
}

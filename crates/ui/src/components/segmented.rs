//! Renders a segmented control for choosing one of several options.
//!
//! # Examples
//!
//! ```no_run
//! use ui::segmented::{Segmented, SegmentedOption};
//!
//! let mode = Segmented::new("editor-mode", "text")
//!     .options([
//!         SegmentedOption::new("text", "Text"),
//!         SegmentedOption::new("preview", "Preview"),
//!     ])
//!     .on_change(|value, _window, _cx| println!("mode: {value}"));
//! ```

use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::{
    AnimationExt as _, App, Bounds, ElementId, Entity, InteractiveElement as _, IntoElement,
    KeyDownEvent, ParentElement as _, Point, RenderOnce, Role, SharedString, SpringAnimation,
    StatefulInteractiveElement as _, StyleRefinement, Styled as _, Toggled, Window, canvas, div,
};

use crate::components::common::{
    ControlSizing, Sizable, StyledExt, focus_handle, focus_ring, h_flex,
};
use crate::icons::Icon;
use crate::theme::{ActiveTheme as _, CONTROL_SPRING, Size};

/// Reports the segment the user chose.
type ChangeHandler<T> = Rc<dyn Fn(T, &mut Window, &mut App)>;

/// One choice in a [`Segmented`] strip.
#[derive(Clone)]
#[must_use]
pub struct SegmentedOption<T> {
    value: T,
    label: SharedString,
    icon: Option<Icon>,
}

impl<T> SegmentedOption<T> {
    /// Creates a choice showing `label`; picking it reports `value`.
    pub fn new<L>(value: T, label: L) -> Self
    where
        L: Into<SharedString>,
    {
        Self {
            value,
            label: label.into(),
            icon: None,
        }
    }

    /// Sets a leading icon for the segment.
    pub fn icon<I>(mut self, icon: I) -> Self
    where
        I: Into<Icon>,
    {
        self.icon = Some(icon.into());
        self
    }
}

/// Converts a `(value, label)` pair into an option.
impl<T, L> From<(T, L)> for SegmentedOption<T>
where
    L: Into<SharedString>,
{
    fn from((value, label): (T, L)) -> Self {
        Self {
            value,
            label: label.into(),
            icon: None,
        }
    }
}

// SegmentedOption is a data builder, not an element, adopt the combinators explicitly.
impl<T> FluentBuilder for SegmentedOption<T> {}

/// A pill strip of mutually exclusive choices.
#[derive(IntoElement)]
#[must_use]
pub struct Segmented<T>
where
    T: Clone + PartialEq + 'static,
{
    id: ElementId,
    /// Shared with the per-segment and key handlers, so captures are bumps.
    options: Rc<[SegmentedOption<T>]>,
    selected: T,
    a11y_label: Option<SharedString>,
    size: Size,
    style: StyleRefinement,
    on_change: Option<ChangeHandler<T>>,
}

impl<T> Segmented<T>
where
    T: Clone + PartialEq + 'static,
{
    /// Creates a strip with `selected` chosen.
    pub fn new<I>(id: I, selected: T) -> Self
    where
        I: Into<ElementId>,
    {
        Self {
            id: id.into(),
            options: Rc::from([]),
            selected,
            a11y_label: None,
            size: Size::Small,
            style: StyleRefinement::default(),
            on_change: None,
        }
    }

    /// Sets the choices from [`SegmentedOption`]s or `(value, label)` pairs.
    pub fn options<I>(mut self, options: I) -> Self
    where
        I: IntoIterator,
        I::Item: Into<SegmentedOption<T>>,
    {
        self.options = options.into_iter().map(Into::into).collect();
        self
    }

    /// Reports the segment chosen, by pointer or by arrow.
    pub fn on_change<F>(mut self, handler: F) -> Self
    where
        F: Fn(T, &mut Window, &mut App) + 'static,
    {
        self.on_change = Some(Rc::new(handler));
        self
    }

    /// Overrides the strip's accessibility label, which defaults to the element id.
    pub fn accessibility_label<L>(mut self, label: L) -> Self
    where
        L: Into<SharedString>,
    {
        self.a11y_label = Some(label.into());
        self
    }
}

impl<T> Sizable for Segmented<T>
where
    T: Clone + PartialEq + 'static,
{
    fn apply_size(&mut self, size: Size) {
        self.size = size;
    }
}

impl<T> gpui::Styled for Segmented<T>
where
    T: Clone + PartialEq + 'static,
{
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl<T> RenderOnce for Segmented<T>
where
    T: Clone + PartialEq + 'static,
{
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let muted = theme.colors.muted;
        let surface = theme.colors.surface;
        let foreground = theme.colors.foreground;
        let muted_foreground = theme.colors.muted_foreground;
        let ring = theme.colors.ring;
        let radius = theme.radius.full;
        let plate_shadow = theme.elevations.control.shadows();

        let selected_index = self
            .options
            .iter()
            .position(|segment| segment.value == self.selected);
        let option_count = self.options.len();

        let on_change = self.on_change.clone();
        let key_on_change = on_change.clone();
        let key_options = self.options.clone();
        let options = self.options.clone();

        let label = self
            .a11y_label
            .clone()
            .unwrap_or_else(|| SharedString::from(self.id.to_string()));

        window.with_element_namespace(self.id.clone(), |window| {
            let geometry: Entity<Vec<Option<Bounds<gpui::Pixels>>>> =
                window.use_keyed_state("segmented-geometry", cx, |_, _| Vec::new());

            let track_origin: Entity<Option<Point<gpui::Pixels>>> =
                window.use_keyed_state("segmented-track-origin", cx, |_, _| None);

            let focus = focus_handle(&self.id, window, cx);
            let track_point = *track_origin.read(cx);

            let plate_target = geometry
                .read(cx)
                .get(selected_index.unwrap_or(usize::MAX))
                .copied()
                .flatten()
                .zip(track_point)
                .map(|(bounds, track)| (bounds.origin.x - track.x, bounds.size.width));

            let segments = self
                .options
                .iter()
                .enumerate()
                .map(move |(index, segment)| {
                    let is_selected = Some(index) == selected_index;
                    let on_change = on_change.clone();
                    let options = options.clone();

                    let content_color = if is_selected {
                        foreground
                    } else {
                        muted_foreground
                    };

                    h_flex()
                        .id((ElementId::from("segment"), segment.label.clone()))
                        .gap_2()
                        .justify_center()
                        .cursor_pointer()
                        .h_full()
                        .control_dense_padding_x(self.size)
                        .rounded(radius)
                        .control_text(self.size)
                        .text_color(content_color)
                        .when(!is_selected, |segment| {
                            segment.hover(|style| style.bg(surface))
                        })
                        .role(Role::RadioButton)
                        .aria_label(segment.label.clone())
                        .aria_toggled(Toggled::from(is_selected))
                        .when_some(segment.icon.clone(), |segment, icon| {
                            segment.child(icon.size(self.size.icon_rems()).color(content_color))
                        })
                        .child(segment.label.clone())
                        .child(
                            canvas(
                                {
                                    let geometry = geometry.clone();
                                    move |bounds, _, cx| {
                                        geometry.update(cx, |geometry, _| {
                                            if geometry.len() <= index {
                                                geometry.resize(index + 1, None);
                                            }
                                            geometry[index] = Some(bounds);
                                        });
                                    }
                                },
                                |_, _, _, _| {},
                            )
                            .absolute()
                            .inset_0(),
                        )
                        .on_click({
                            move |_, window, cx| {
                                if Some(index) != selected_index
                                    && let Some(on_change) = &on_change
                                    && let Some(option) = options.get(index)
                                {
                                    on_change(option.value.clone(), window, cx);
                                }
                            }
                        })
                });

            div().child(
                div()
                    .id(self.id.clone())
                    .track_focus(&focus)
                    .focusable()
                    .tab_stop(true)
                    .focus_visible(|style| style.shadow(focus_ring(ring)))
                    .role(Role::RadioGroup)
                    .aria_label(label)
                    .flex()
                    .items_center()
                    .gap_1()
                    .relative()
                    .flex_none()
                    .control_height(self.size)
                    .control_plate_inset(self.size)
                    .rounded(radius)
                    .bg(muted)
                    .control_text(self.size)
                    .child(
                        canvas(
                            {
                                let track_origin = track_origin.clone();
                                move |bounds, _, cx| {
                                    track_origin
                                        .update(cx, |origin, _| *origin = Some(bounds.origin));
                                }
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .inset_0(),
                    )
                    .when_some(plate_target, |track, (x, width)| {
                        track.child(
                            div()
                                .id("segmented-plate")
                                .debug_selector(|| "segmented-plate".to_string())
                                .absolute()
                                .top(self.size.plate_inset_rems())
                                .bottom(self.size.plate_inset_rems())
                                .rounded(radius)
                                .bg(surface)
                                .shadow(plate_shadow.clone())
                                .with_spring(
                                    "segmented-plate-x",
                                    SpringAnimation::new(CONTROL_SPRING).to(x),
                                    |plate, x| plate.left(x),
                                )
                                .with_spring(
                                    "segmented-plate-w",
                                    SpringAnimation::new(CONTROL_SPRING).to(width),
                                    |spring_plate, width| {
                                        spring_plate.map_element(move |plate| plate.w(width))
                                    },
                                ),
                        )
                    })
                    .on_key_down({
                        move |event: &KeyDownEvent, window, cx| {
                            let target = match event.keystroke.key.as_str() {
                                "right" => match selected_index {
                                    Some(index) => {
                                        index.checked_add(1).filter(|next| *next < option_count)
                                    }
                                    None => (option_count > 0).then_some(0),
                                },
                                "left" => selected_index.and_then(|index| index.checked_sub(1)),
                                "home" => (option_count > 0).then_some(0),
                                "end" => option_count.checked_sub(1),
                                _ => return,
                            };

                            if let Some(next) = target
                                && Some(next) != selected_index
                                && let Some(on_change) = &key_on_change
                                && let Some(option) = key_options.get(next)
                            {
                                on_change(option.value.clone(), window, cx);
                            }

                            cx.stop_propagation();
                        }
                    })
                    .children(segments)
                    .refine_style(&self.style),
            )
        })
    }
}

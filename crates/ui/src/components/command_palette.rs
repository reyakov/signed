//! Renders a modal query field over the application's actions.
//!
//! The state is the view: [`CommandPaletteState`] owns the query field and
//! the matches, and the owner renders the entity itself. Opening resets the
//! query, typing asks the caller's matcher for fresh matches, the arrow keys
//! move a highlight, and Enter runs the highlighted command through
//! [`CommandPalette::on_execute`].
//!
//! # Examples
//!
//! ```no_run
//! use gpui::prelude::*;
//! use gpui::{App, Entity, Window};
//! use ui::command_palette::{Command, CommandPalette, CommandPaletteState};
//!
//! # fn build(window: &mut Window, cx: &mut App) -> Entity<CommandPaletteState> {
//! # let commands = vec![Command::new("save", "Save file").hint("Cmd+S")];
//! let palette = CommandPalette::new("palette", commands, |query, all| {
//!     all.iter().filter(|command| command.label.contains(query)).cloned().collect()
//! })
//! .on_execute(|command, _window, _cx| println!("run {}", command.id));
//! let state = cx.new(|cx| CommandPaletteState::new(palette, window, cx));
//! # state
//! # }
//! ```

use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, App, Context, ElementId, Entity, InteractiveElement as _, IntoElement, KeyBinding,
    MouseButton, ParentElement as _, Render, Role, SharedString, StatefulInteractiveElement as _,
    StyleRefinement, Styled as _, Subscription, UniformListScrollHandle, Window, actions, deferred,
    div, relative, rems, uniform_list,
};

use crate::components::common::{
    DIALOG_LAYER, ElementIdExt as _, SelectableRows as _, StyledExt as _, h_flex, v_flex,
};
use crate::components::empty_state::EmptyState;
use crate::components::input::{Input, InputState, InputStateEvent};
use crate::components::menu::focus_capture;
use crate::icons::Icon;
use crate::theme::{ActiveTheme as _, POPUP_MAX_HEIGHT};

/// The key context the palette's panel declares.
pub const COMMAND_PALETTE_CONTEXT: &str = "CommandPalette";

/// Filters and ranks the commands for a query.
pub type CommandMatcher = Rc<dyn Fn(&str, &[Command]) -> Vec<Command>>;
/// Runs the chosen command.
pub type CommandHandler = Rc<dyn Fn(&Command, &mut Window, &mut App)>;

actions!(
    command_palette,
    [HighlightNext, HighlightPrevious, Choose, Dismiss]
);

/// A command the palette can run: a stable id and the label the matcher sees.
#[derive(Clone)]
#[must_use]
pub struct Command {
    /// The stable domain id handed to `on_execute`.
    pub id: SharedString,
    /// What the matcher matches and the row shows.
    pub label: SharedString,
    /// Gray text on the row's right edge, e.g. the keybinding.
    pub hint: Option<SharedString>,
    /// Leading glyph shown before the label.
    pub icon: Option<Icon>,
}

impl Command {
    /// Creates a command without a hint or icon.
    pub fn new<I, L>(id: I, label: L) -> Self
    where
        I: Into<SharedString>,
        L: Into<SharedString>,
    {
        Self {
            id: id.into(),
            label: label.into(),
            hint: None,
            icon: None,
        }
    }

    /// Shows an icon before the label.
    pub fn icon<I>(mut self, icon: I) -> Self
    where
        I: Into<Icon>,
    {
        self.icon = Some(icon.into());
        self
    }

    /// Sets the row's right-edge hint.
    pub fn hint<H>(mut self, hint: H) -> Self
    where
        H: Into<SharedString>,
    {
        self.hint = Some(hint.into());
        self
    }
}

// Command is a data builder, not an element, adopt the combinators explicitly.
impl FluentBuilder for Command {}

/// Builder for a command palette, consumed by [`CommandPaletteState::new`].
#[must_use]
pub struct CommandPalette {
    id: ElementId,
    name: SharedString,
    placeholder: SharedString,
    commands: Vec<Command>,
    matcher: CommandMatcher,
    on_execute: Option<CommandHandler>,
    style: StyleRefinement,
}

impl CommandPalette {
    /// Creates the builder with a caller-owned matcher that decides what matches.
    pub fn new<I, M>(id: I, commands: Vec<Command>, matcher: M) -> Self
    where
        I: Into<ElementId>,
        M: Fn(&str, &[Command]) -> Vec<Command> + 'static,
    {
        Self {
            id: id.into(),
            name: SharedString::from("Command palette"),
            placeholder: SharedString::from("Type a command..."),
            commands,
            matcher: Rc::new(matcher),
            on_execute: None,
            style: StyleRefinement::default(),
        }
    }

    /// Sets the text shown while the query is empty.
    pub fn placeholder<L>(mut self, placeholder: L) -> Self
    where
        L: Into<SharedString>,
    {
        self.placeholder = placeholder.into();
        self
    }

    /// Runs `handler` with the chosen command, then closes the palette.
    pub fn on_execute<F>(mut self, handler: F) -> Self
    where
        F: Fn(&Command, &mut Window, &mut App) + 'static,
    {
        self.on_execute = Some(Rc::new(handler));
        self
    }

    /// Overrides the accessibility label of the panel.
    pub fn accessibility_label<L>(mut self, label: L) -> Self
    where
        L: Into<SharedString>,
    {
        self.name = label.into();
        self
    }
}

// CommandPalette is a data builder, not an element, adopt the combinators explicitly.
impl FluentBuilder for CommandPalette {}

/// The stateful palette.
///
/// # Examples
///
/// ```no_run
/// # use gpui::prelude::*;
/// # use gpui::{App, Entity, Window};
/// # use ui::command_palette::{Command, CommandPalette, CommandPaletteState};
/// #
/// # fn build(window: &mut Window, cx: &mut App) -> Entity<CommandPaletteState> {
/// # let palette = CommandPalette::new("palette", Vec::new(), |_, all| all.to_vec());
/// # let state = cx.new(|cx| CommandPaletteState::new(palette, window, cx));
/// state.update(cx, |state, cx| state.toggle(window, cx));
/// assert!(state.read(cx).is_open());
/// # state
/// # }
/// ```
pub struct CommandPaletteState {
    id: ElementId,
    name: SharedString,
    placeholder: SharedString,
    commands: Vec<Command>,
    matcher: CommandMatcher,
    on_execute: Option<CommandHandler>,
    style: StyleRefinement,
    query: Entity<InputState>,
    /// The commands the matcher returned for the query, in display order.
    matches: Vec<Command>,
    /// The highlighted row, when the palette is open.
    highlight: Option<usize>,
    open: bool,
    /// The row list's scroll, kept across frames while the palette is open.
    rows_scroll: UniformListScrollHandle,
    /// Keeps the query-event subscription tied to the state's lifetime.
    _subscription: Subscription,
}

impl CommandPaletteState {
    /// Builds the state, its query field, and the subscription tying them.
    pub fn new(palette: CommandPalette, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let query = InputState::new(cx);
        let subscription = cx.subscribe_in(&query, window, |state, _, event, window, cx| {
            if event == &InputStateEvent::TextChanged {
                state.text_changed(window, cx);
            }
        });

        let matches = (palette.matcher)("", &palette.commands);

        Self {
            id: palette.id,
            name: palette.name,
            placeholder: palette.placeholder,
            commands: palette.commands,
            matcher: palette.matcher,
            on_execute: palette.on_execute,
            style: palette.style,
            query,
            matches,
            highlight: None,
            open: false,
            rows_scroll: UniformListScrollHandle::new(),
            _subscription: subscription,
        }
    }

    /// Opens the palette: fresh query, fresh matches, focus in the field.
    pub fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.open {
            self.focus(window, cx);
            return;
        }
        self.open = true;

        self.query.update(cx, |query, cx| {
            query.set_value_silent("", window, cx);
        });

        self.refilter("");
        self.highlight = (!self.matches.is_empty()).then_some(0);
        self.focus(window, cx);

        cx.notify();
    }

    /// Closes the palette.
    pub fn close(&mut self, cx: &mut Context<Self>) {
        if !self.open {
            return;
        }
        self.open = false;
        self.highlight = None;
        cx.notify();
    }

    /// Opens the palette when closed, closes it when open.
    pub fn toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.open {
            self.close(cx);
        } else {
            self.open(window, cx);
        }
    }

    /// Whether the palette is showing.
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Replaces the command list and re-runs the current query's matches.
    pub fn set_commands(&mut self, commands: Vec<Command>, cx: &mut Context<Self>) {
        self.commands = commands;
        let query = self.query.read(cx).value().to_string();
        self.refilter(&query);
        cx.notify();
    }

    /// Focuses the query field.
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        window.focus(&self.query.read(cx).focus_handle(), cx);
    }

    /// Asks the caller's matcher who matches, and clamps the highlight.
    fn refilter(&mut self, query: &str) {
        self.matches = (self.matcher)(query, &self.commands);
        if let Some(highlight) = self.highlight {
            self.highlight = (highlight < self.matches.len()).then_some(highlight);
        }
    }

    /// Typing: fresh matches, highlight the first row.
    fn text_changed(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let query = self.query.read(cx).value().to_string();
        self.refilter(&query);
        self.highlight = (!self.matches.is_empty()).then_some(0);
        cx.notify();
    }

    /// Down and Up over the matched rows, wrapping at the ends.
    fn move_highlight(&mut self, delta: i32, cx: &mut Context<Self>) {
        if self.matches.is_empty() {
            return;
        }

        let rows: Vec<usize> = (0..self.matches.len()).collect();
        let next = rows.wrapping_adjacent(self.highlight, delta > 0);

        self.highlight = next;
        cx.notify();
    }

    /// Enter. Runs the highlighted command and closes.
    fn choose(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.highlight else {
            cx.propagate();
            return;
        };
        self.choose_row(row, window, cx);
    }

    /// Runs the command at a row and closes.
    fn choose_row(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(command) = self.matches.get(row) else {
            return;
        };
        let command = command.clone();
        if let Some(on_execute) = self.on_execute.clone() {
            on_execute(&command, window, cx);
        }
        self.close(cx);
    }

    /// Escape. Closes the palette.
    fn dismiss(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if !self.open {
            cx.propagate();
            return;
        }
        self.close(cx);
    }

    /// Binds the palette's four keys: down, up, enter, and escape.
    pub fn bind_keys(cx: &mut App) {
        let under_input = Some("CommandPalette > Input");
        cx.bind_keys([
            KeyBinding::new("down", HighlightNext, under_input),
            KeyBinding::new("up", HighlightPrevious, under_input),
            KeyBinding::new("enter", Choose, under_input),
            KeyBinding::new("escape", Dismiss, under_input),
            KeyBinding::new("down", HighlightNext, Some(COMMAND_PALETTE_CONTEXT)),
            KeyBinding::new("up", HighlightPrevious, Some(COMMAND_PALETTE_CONTEXT)),
            KeyBinding::new("enter", Choose, Some(COMMAND_PALETTE_CONTEXT)),
            KeyBinding::new("escape", Dismiss, Some(COMMAND_PALETTE_CONTEXT)),
        ]);
    }
}

impl Render for CommandPaletteState {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Restores the trigger's focus when the palette closes.
        let overlay_focus = self.query.read(cx).focus_handle();
        self.id
            .track_overlay_focus(&overlay_focus, self.open, window, cx);

        if !self.open {
            return div().into_any_element();
        }

        let (colors, radius, shadows, titlebar_height) = {
            let theme = cx.theme();
            (
                theme.colors,
                theme.radius,
                theme.elevations.dialog.shadows(),
                theme.titlebar_height,
            )
        };

        let id = self.id.clone();
        let state = cx.entity();
        let query_focus = self.query.read(cx).focus_handle();

        let rows: AnyElement = if self.matches.is_empty() {
            EmptyState::compact("No matches", cx).into_any_element()
        } else {
            let matches = self.matches.clone();
            let highlight = self.highlight;
            let state = state.clone();
            let rows_id = id.clone();
            let radius_sm = radius.sm;
            let foreground = colors.foreground;
            let muted_foreground = colors.muted_foreground;

            // Rows are `min_h_8`, so the uniform row height is two rems.
            let row_height = rems(2.0).to_pixels(window.rem_size());
            let count = matches.len();
            let list_height = (row_height * count as f32).min(POPUP_MAX_HEIGHT);

            uniform_list((rows_id.clone(), "rows"), count, move |range, _, cx| {
                range
                    .map(|row| {
                        let Some(command) = matches.get(row) else {
                            return div().into_any_element();
                        };

                        let highlighted = highlight == Some(row);
                        let row_index = row;

                        h_flex()
                            .id((ElementId::from(command.id.clone()), "command"))
                            .debug_selector({
                                let id = rows_id.clone();
                                move || format!("{id}-command-{row}")
                            })
                            .role(Role::ListBoxOption)
                            .aria_label(command.label.clone())
                            .gap_2()
                            .px_2()
                            .py_1()
                            .min_h_8()
                            .text_sm()
                            .line_height(relative(1.0))
                            .rounded(radius_sm)
                            .highlighted_row(cx, highlighted, false)
                            .on_mouse_move({
                                let state = state.clone();
                                move |_, _window, cx| {
                                    if state.read(cx).highlight != Some(row_index) {
                                        state.update(cx, |state, cx| {
                                            state.highlight = Some(row_index);
                                            cx.notify();
                                        });
                                    }
                                }
                            })
                            .on_click({
                                let state = state.clone();
                                move |_, window, cx| {
                                    state.update(cx, |state, cx| {
                                        state.choose_row(row_index, window, cx)
                                    });
                                }
                            })
                            .when_some(command.icon.clone(), |row, icon| {
                                row.child(icon.size(rems(1.0)).color(foreground))
                            })
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(command.label.clone()),
                            )
                            .when_some(command.hint.clone(), |row, hint| {
                                row.child(
                                    div()
                                        .flex_shrink_0()
                                        .text_xs()
                                        .text_color(muted_foreground)
                                        .child(hint),
                                )
                            })
                            .into_any_element()
                    })
                    .collect()
            })
            .track_scroll(&self.rows_scroll)
            .h(list_height)
            .into_any_element()
        };

        deferred(
            div()
                .absolute()
                .inset_0()
                .flex()
                .justify_center()
                .items_start()
                .pt(titlebar_height)
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .bg(colors.overlay)
                        .occlude()
                        .on_mouse_down(MouseButton::Left, {
                            let state = state.clone();
                            move |_, _window, cx| {
                                state.update(cx, |state, cx| state.close(cx));
                                cx.stop_propagation();
                            }
                        }),
                )
                .child(
                    v_flex()
                        .id((id.clone(), "panel"))
                        .debug_selector({
                            let id = id.clone();
                            move || format!("{id}-panel")
                        })
                        .key_context(COMMAND_PALETTE_CONTEXT)
                        .role(Role::Dialog)
                        .aria_label(self.name.clone())
                        .w_full()
                        .max_w(relative(0.6))
                        .p_2()
                        .gap_2()
                        .text_color(colors.foreground)
                        .floating_panel(cx, radius.overlay, shadows)
                        .overflow_hidden()
                        .refine_style(&self.style)
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_action({
                            let state = state.clone();
                            move |_: &HighlightNext, _window, cx| {
                                state.update(cx, |state, cx| state.move_highlight(1, cx));
                            }
                        })
                        .on_action({
                            let state = state.clone();
                            move |_: &HighlightPrevious, _window, cx| {
                                state.update(cx, |state, cx| state.move_highlight(-1, cx));
                            }
                        })
                        .on_action({
                            let state = state.clone();
                            move |_: &Choose, window, cx| {
                                state.update(cx, |state, cx| state.choose(window, cx));
                            }
                        })
                        .on_action({
                            let state = state.clone();
                            move |_: &Dismiss, window, cx| {
                                state.update(cx, |state, cx| state.dismiss(window, cx));
                            }
                        })
                        .child(
                            Input::new((id.clone(), "query"), self.query.clone())
                                .placeholder(self.placeholder.clone()),
                        )
                        .child(rows)
                        .child(focus_capture(query_focus.clone(), move |window, cx| {
                            !query_focus.contains_focused(window, cx)
                        })),
                ),
        )
        .with_priority(DIALOG_LAYER)
        .into_any_element()
    }
}

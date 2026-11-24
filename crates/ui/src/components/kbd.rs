//! Renders keyboard keys and shortcuts as small key caps.
//!
//! # Examples
//!
//! ```no_run
//! use gpui::Keystroke;
//! use ui::kbd::Kbd;
//!
//! let save = Kbd::new(Keystroke::parse("cmd-s").unwrap());
//! ```

use gpui::{
    Action, App, InteractiveElement as _, IntoElement, KeyBinding, KeyContext, Keystroke,
    OperatingSystem, ParentElement as _, RenderOnce, SharedString, StyleRefinement, Styled as _,
    Window, div, relative,
};

use crate::components::common::StyledExt as _;
use crate::theme::ActiveTheme as _;

/// A key or shortcut shown as a small key cap, like `⌘S` or `Ctrl+S`.
#[derive(IntoElement, Clone, Debug)]
#[must_use]
pub struct Kbd {
    stroke: Keystroke,
    style: StyleRefinement,
}

impl From<Keystroke> for Kbd {
    fn from(stroke: Keystroke) -> Self {
        Self::new(stroke)
    }
}

impl Kbd {
    /// Creates a key cap showing `stroke`.
    pub fn new(stroke: Keystroke) -> Self {
        Self {
            stroke,
            style: StyleRefinement::default(),
        }
    }

    /// The keystroke this key cap shows.
    pub fn keystroke(&self) -> &Keystroke {
        &self.stroke
    }

    /// Returns a key cap for the highest-precedence binding of `action`.
    pub fn binding_for_action(
        action: &dyn Action,
        context: Option<&str>,
        window: &Window,
    ) -> Option<Self> {
        let binding = match context.and_then(|context| KeyContext::parse(context).ok()) {
            Some(context) => {
                window.highest_precedence_binding_for_action_in_context(action, context)
            }
            None => window.highest_precedence_binding_for_action(action),
        }?;
        Self::from_binding(&binding)
    }

    /// Shows the binding's keystroke as the keymap holds it.
    fn from_binding(binding: &KeyBinding) -> Option<Self> {
        binding.keystrokes().first().map(|stroke| {
            Self::new(Keystroke {
                modifiers: *stroke.modifiers(),
                key: stroke.key().into(),
                key_char: None,
            })
        })
    }

    /// Formats `stroke` for `os`: modifier symbols on macOS, `Ctrl+` words elsewhere.
    fn format(stroke: &Keystroke, os: OperatingSystem) -> String {
        let mac = os == OperatingSystem::Mac;
        let separator = if mac { "" } else { "+" };
        let mut parts: Vec<&str> = Vec::new();
        // macOS reads ⌃⌥⇧⌘; other platforms read Ctrl+Alt+Shift+Win.
        if stroke.modifiers.control {
            parts.push(if mac { "⌃" } else { "Ctrl" });
        }
        if stroke.modifiers.alt {
            parts.push(if mac { "⌥" } else { "Alt" });
        }
        if stroke.modifiers.shift {
            parts.push(if mac { "⇧" } else { "Shift" });
        }
        if stroke.modifiers.platform {
            parts.push(match os {
                OperatingSystem::Mac => "⌘",
                OperatingSystem::Windows => "Win",
                _ => "Super",
            });
        }

        let key = Self::key_label(&stroke.key, os);
        parts.push(&key);
        parts.join(separator)
    }

    /// The label for a key, with modifier glyphs on macOS and words elsewhere.
    fn key_label(key: &str, os: OperatingSystem) -> String {
        let mac = os == OperatingSystem::Mac;
        let label: &str = match key {
            "cmd" | "platform" => {
                if mac {
                    "⌘"
                } else {
                    "Win"
                }
            }
            "ctrl" | "control" => {
                if mac {
                    "⌃"
                } else {
                    "Ctrl"
                }
            }
            "alt" => {
                if mac {
                    "⌥"
                } else {
                    "Alt"
                }
            }
            "shift" => {
                if mac {
                    "⇧"
                } else {
                    "Shift"
                }
            }
            "function" => "Fn",
            "backspace" => {
                if mac {
                    "⌫"
                } else {
                    "Backspace"
                }
            }
            "delete" => {
                if mac {
                    "⌦"
                } else {
                    "Delete"
                }
            }
            "enter" | "return" => {
                if mac {
                    "⏎"
                } else {
                    "Enter"
                }
            }
            "escape" => {
                if mac {
                    "⎋"
                } else {
                    "Esc"
                }
            }
            "tab" => {
                if mac {
                    "⇥"
                } else {
                    "Tab"
                }
            }
            "space" => "Space",
            "pageup" => "Page Up",
            "pagedown" => "Page Down",
            "left" => {
                if mac {
                    "←"
                } else {
                    "Left"
                }
            }
            "right" => {
                if mac {
                    "→"
                } else {
                    "Right"
                }
            }
            "up" => {
                if mac {
                    "↑"
                } else {
                    "Up"
                }
            }
            "down" => {
                if mac {
                    "↓"
                } else {
                    "Down"
                }
            }
            key => key,
        };

        let mut chars = label.chars();
        match chars.next() {
            Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
            None => String::new(),
        }
    }
}

impl gpui::Styled for Kbd {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for Kbd {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let os = cx.platform_info().os;
        let selector = format!("kbd:{}", self.stroke.unparse(os));
        let label = SharedString::from(Self::format(&self.stroke, os));

        div()
            .debug_selector(move || selector)
            .flex_none()
            .bg(theme.colors.muted)
            .text_color(theme.colors.muted_foreground)
            .py_0p5()
            .px_1()
            .min_w_5()
            .rounded(theme.radius.sm)
            .text_center()
            .line_height(relative(1.))
            .text_xs()
            .refine_style(&self.style)
            .child(label)
    }
}

//! The theme engine: reads the running system theme, resolves it through
//! presets and overrides, and installs the tokens every component paints
//! from.
//!
//! Pipeline (plan §4.5): `ThemeSource` snapshot → `resolve` overlay chain →
//! `Theme` global → all windows repaint. Watchers re-run the pipeline on
//! real OS change events.

mod colors;
mod definition;
mod presets;
mod ramps;
mod resolve;
mod source;
mod system;

use std::time::Duration;

pub use colors::{
    ColorTokens, MaterialTokens, WINDOWS_CLOSE_FOREGROUND, WINDOWS_CLOSE_HOVER,
    WINDOWS_CLOSE_PRESSED,
};
pub use definition::ThemeDefinition;
use gpui::{
    App, Global, Hsla, Pixels, Rems, SpringConfig, WindowAppearance, WindowBackgroundAppearance,
    px, rems,
};
pub use ramps::{AccentShades, Oklch};
pub use resolve::{Ramps, ResolvedTheme};
pub use system::{AccessibilityPreferences, SourceKind, SystemTheme};

/// The hairline width of the control border every button and field paints.
pub const BORDER_WIDTH: Pixels = px(1.0);
/// The side of glyphs inside titlebar overlay controls.
pub const OVERLAY_ICON_SIZE: Rems = rems(0.75);
/// The draggable width of a resize seam and its panel-row gutter.
pub const RESIZE_HANDLE_WIDTH: Pixels = px(8.0);
/// The widest a titlebar tab grows before its label fades.
pub const TAB_MAX_WIDTH: Pixels = px(240.0);
/// The narrowest a titlebar tab shrinks, keeping short labels clear of the fade.
pub const TAB_MIN_WIDTH: Pixels = px(140.0);
/// Opacity applied to disabled controls.
pub const DISABLED_OPACITY: f32 = 0.5;
/// The focus ring's ink alpha.
pub const FOCUS_RING_ALPHA: f32 = 0.5;
/// The narrowest a popover may grow.
pub const POPOVER_MIN_WIDTH: Pixels = px(128.0);
/// The default minimum width of a dialog.
pub const DIALOG_MIN_WIDTH: Pixels = px(384.0);
/// The default minimum width of a menu.
pub const MENU_MIN_WIDTH: Pixels = px(224.0);
/// The width of the pushed-alert stack.
pub const ALERT_STACK_WIDTH: Pixels = px(320.0);
/// The accent edge of an inline alert.
pub const ALERT_EDGE_WIDTH: Pixels = px(6.0);
/// The default width of a combobox trigger.
pub const COMBOBOX_WIDTH: Pixels = px(192.0);
/// The narrowest a combobox trigger may shrink.
pub const COMBOBOX_MIN_WIDTH: Pixels = px(96.0);
/// The width of a code view's line-number gutter.
pub const CODE_GUTTER_WIDTH: Pixels = px(40.0);
/// The width of the speech waveform strip.
pub const SPEECH_WAVEFORM_WIDTH: Pixels = px(96.0);
/// The line-height ratio of monospace code.
pub const CODE_LINE_HEIGHT: f32 = 1.5;
/// The tallest an anchored popup's list may grow.
pub const POPUP_MAX_HEIGHT: Pixels = px(320.0);
/// The widest a tooltip may grow.
pub const TOOLTIP_MAX_WIDTH: Pixels = px(360.0);
/// The caret's thickness.
pub const CARET_WIDTH: Pixels = px(1.5);
/// The IME preedit underline's thickness.
pub const UNDERLINE_WIDTH: Pixels = px(1.0);
/// How far one wheel line scrolls, as a multiple of the line height.
pub const WHEEL_LINE_HEIGHT: f32 = 20.0;
/// How far one keyboard arrow step moves a resize seam.
pub const RESIZE_KEYBOARD_STEP: Pixels = px(32.0);
/// The inset around a window's edge that starts a resize gesture.
pub const WINDOW_RESIZE_INSET: Pixels = px(4.0);
/// The pad macOS reserves for its traffic lights at a strip's leading edge.
pub const TRAFFIC_LIGHT_PAD: Pixels = px(88.0);
/// The width of a Windows caption button.
pub const WINDOWS_CAPTION_WIDTH: Pixels = px(36.0);
/// The width a sidebar opens at.
pub const SIDEBAR_DEFAULT_WIDTH: Pixels = px(240.0);
/// The narrowest a sidebar may be dragged.
pub const SIDEBAR_MIN_WIDTH: Pixels = px(160.0);
/// The widest a sidebar may be dragged.
pub const SIDEBAR_MAX_WIDTH: Pixels = px(480.0);
/// The size a pane starts at before its group has measured the extent.
pub const PANE_DEFAULT_SIZE: Pixels = px(200.0);
/// The floor a pane keeps while dragging unless the caller lowers it.
pub const PANE_MIN_SIZE: Pixels = px(48.0);
/// The hairline a resize seam paints on a shared boundary.
pub const HAIRLINE_WIDTH: Pixels = px(0.5);
/// How much of the window shadow's strength survives losing focus.
pub const INACTIVE_SHADOW_OPACITY: f32 = 0.7;
/// The spread of the keyboard focus ring.
pub const FOCUS_RING_SPREAD: Pixels = px(3.0);
/// How far a pushed alert slides in on entry.
pub const ENTER_SLIDE_DISTANCE: Pixels = px(24.0);
/// The widest a toast may grow.
pub const TOAST_MAX_WIDTH: Pixels = px(480.0);
/// One revolution of the spinner.
pub const SPINNER_PERIOD: Duration = Duration::from_millis(800);
/// One sweep of the indeterminate progress bar.
pub const PROGRESS_ANIMATION_PERIOD: Duration = Duration::from_millis(1500);
/// How long an autohide scrollbar thumb stays fully visible before fading.
pub const SCROLLBAR_AUTOHIDE_DELAY: Duration = Duration::from_millis(700);
/// How long the autohide scrollbar's fade runs.
pub const SCROLLBAR_FADE_DURATION: Duration = Duration::from_millis(250);
/// The soft wash that marks the active code row.
pub const WASH_ALPHA_SOFT: f32 = 0.45;
/// How far a scrolling list renders past its viewport.
pub const CODE_OVERSCROLL_MAX: Pixels = px(400.0);
/// The spring every control's toggle and segment animations ride.
pub const CONTROL_SPRING: SpringConfig = SpringConfig::new(500., 35., 1.);
/// The monospace advance estimate as a share of the text size.
pub const MONO_ADVANCE_RATIO: f32 = 0.62;
/// How much a pixel avatar's tint washes over its background.
pub const PIXEL_AVATAR_TINT_ALPHA: f32 = 0.16;
/// Pixel avatar cell lightness multipliers.
pub const PIXEL_AVATAR_LIGHT_LIFT: f32 = 1.6;
pub const PIXEL_AVATAR_LIGHT_CAP: f32 = 0.95;
pub const PIXEL_AVATAR_DARK_FACTOR: f32 = 0.45;
pub const PIXEL_AVATAR_DARK_FLOOR: f32 = 0.18;
/// The default window material transparency, as a percent (0 opaque, 100 the lightest fill).
pub const DEFAULT_TRANSPARENCY: u8 = 80;
/// The light-mode floor of the material fill.
pub const MIN_MATERIAL_FILL_LIGHT: f32 = 0.45;
/// The dark-mode floor of the material fill.
pub const MIN_MATERIAL_FILL_DARK: f32 = 0.15;
/// The opacity of the grain overlay's noise.
pub const GRAIN_ALPHA: f32 = 0.08;

/// Light or dark, resolved from the system appearance.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeVariant {
    #[default]
    Light,
    Dark,
}

impl ThemeVariant {
    fn from_appearance(appearance: WindowAppearance) -> Self {
        match appearance {
            WindowAppearance::Light | WindowAppearance::VibrantLight => Self::Light,
            WindowAppearance::Dark | WindowAppearance::VibrantDark => Self::Dark,
        }
    }

    /// Classifies a surface by its lightness, for platforms that state no mode.
    pub fn by_lightness(background: Hsla) -> Self {
        if background.l >= 0.5 {
            Self::Light
        } else {
            Self::Dark
        }
    }
}

/// A named control size class.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Size {
    XSmall,
    Small,
    /// The default.
    #[default]
    Medium,
    Large,
}

/// The resolved theme, installed as the app-wide global.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Theme {
    pub variant: ThemeVariant,
    pub colors: ColorTokens,
    /// The translucent paints over the window's native material.
    pub material: MaterialTokens,
    pub ramps: Ramps,
    pub radius: Radius,
    pub elevations: Elevations,
    /// Overlay transition timing; gpui's animation primitives do the rest.
    pub motion: Motion,
    /// Overlay scrollbar geometry.
    pub scrollbar: Scrollbar,
    /// The height the window's titlebar strips draw at.
    pub titlebar_height: Pixels,
    /// The theme's root font size; rem-based sizing scales with it.
    pub root_font_size: Pixels,
    /// The monospace family, for code surfaces.
    pub mono_font: &'static str,
    pub accessibility: AccessibilityPreferences,
    /// The material a window should open with, after accessibility.
    pub window_background: WindowBackgroundAppearance,
}

impl Global for Theme {}

impl Theme {
    /// Reads the theme global. Panics before `init`.
    pub fn global(cx: &App) -> &Self {
        cx.global::<Self>()
    }

    /// The system UI family, for chrome surfaces.
    pub fn ui_font(&self) -> &'static str {
        ".SystemUIFont"
    }

    fn from_resolved(resolved: ResolvedTheme) -> Self {
        Self {
            variant: resolved.variant,
            colors: resolved.colors,
            material: resolved.material,
            ramps: resolved.ramps,
            radius: resolved.radius,
            elevations: resolved.elevations,
            motion: resolved.motion,
            scrollbar: resolved.scrollbar,
            titlebar_height: resolved.titlebar_height,
            root_font_size: resolved.root_font_size,
            mono_font: resolved.mono_font,
            accessibility: resolved.accessibility,
            window_background: resolved.window_background,
        }
    }

    /// Installs the engine global, its platform watchers, and the initial resolved theme.
    pub fn init(cx: &mut App) {
        let mut engine = ThemeEngine {
            source: source::platform_source(),
            mode_override: None,
            preset: None,
            custom: None,
        };
        engine.source.install_watchers(cx.to_async());
        cx.set_global(engine);
        reapply(cx);
    }

    /// Synchronizes the theme from the window's appearance.
    pub fn sync_from_window(window: &mut gpui::Window, cx: &mut App) {
        let window_variant = ThemeVariant::from_appearance(window.appearance());
        let theme = cx.global::<Theme>();
        let engine = cx.global::<ThemeEngine>();
        if engine.mode_override.is_none() && theme.variant != window_variant {
            reapply(cx);
        }
    }

    /// Sizes the window's rem to the theme's root font size.
    pub fn sync_window_rem_size(window: &mut gpui::Window, cx: &App) {
        let theme = cx.global::<Theme>();
        window.set_rem_size(theme.root_font_size);
    }
}

/// Corner radii components paint from; grows one class at a time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Radius {
    /// Small controls: checkbox and radio indicators.
    pub sm: Pixels,
    pub control: Pixels,
    /// Floating layers: popovers, menus, dialogs.
    pub overlay: Pixels,
    /// Pill and circle shapes.
    pub full: Pixels,
}

impl Default for Radius {
    fn default() -> Self {
        Self {
            sm: px(4.),
            control: px(6.),
            overlay: px(10.),
            full: px(9999.),
        }
    }
}

/// One layer of a floating panel's drop shadow.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shadow {
    pub offset_x: Pixels,
    pub offset_y: Pixels,
    pub blur_radius: Pixels,
    pub spread_radius: Pixels,
    /// Translucent black, like every native overlay shadow.
    pub color: Hsla,
}

impl Shadow {
    fn none() -> Self {
        Self {
            offset_x: px(0.),
            offset_y: px(0.),
            blur_radius: px(0.),
            spread_radius: px(0.),
            color: colors::scrim(0.),
        }
    }

    fn black(offset_x: f32, offset_y: f32, blur_radius: f32, alpha: f32) -> Self {
        Self {
            offset_x: px(offset_x),
            offset_y: px(offset_y),
            blur_radius: px(blur_radius),
            spread_radius: px(0.),
            color: colors::scrim(alpha),
        }
    }
}

/// The drop shadow stack of a floating layer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Elevation {
    /// Key shadow first, ambient grounding second.
    pub layers: [Shadow; 2],
}

impl Elevation {
    /// No shadow: dark floating layers separate by border.
    fn none() -> Self {
        Self {
            layers: [Shadow::none(), Shadow::none()],
        }
    }

    fn single(key: Shadow) -> Self {
        Self {
            layers: [key, Shadow::none()],
        }
    }

    fn layered(key: Shadow, ambient: Shadow) -> Self {
        Self {
            layers: [key, ambient],
        }
    }

    /// How far the shadow layers paint past the element's bounds.
    pub fn bleed(self) -> Pixels {
        self.layers.iter().fold(px(0.), |bleed, layer| {
            if layer.color.a > 0. {
                bleed.max(
                    layer.offset_x.abs().max(layer.offset_y.abs())
                        + layer.blur_radius
                        + layer.spread_radius,
                )
            } else {
                bleed
            }
        })
    }

    /// The gpui shadow list an overlay panel paints with.
    pub fn shadows(self) -> Vec<gpui::BoxShadow> {
        self.layers
            .iter()
            .filter(|layer| layer.color.a > 0.)
            .map(|layer| gpui::BoxShadow {
                color: layer.color,
                offset: gpui::point(layer.offset_x, layer.offset_y),
                blur_radius: layer.blur_radius,
                spread_radius: layer.spread_radius,
                inset: false,
            })
            .collect()
    }
}

/// The drop shadows of each floating layer class.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Elevations {
    /// Popovers, menus, and pushed alerts ride above content.
    pub popover: Elevation,
    /// Modal dialogs sit higher with a wider one.
    pub dialog: Elevation,
    /// Raised plates inside controls — a selected segmented segment.
    pub control: Elevation,
}

impl Elevations {
    pub(crate) fn for_variant(variant: ThemeVariant) -> Self {
        if variant == ThemeVariant::Dark {
            return Self {
                popover: Elevation::none(),
                dialog: Elevation::none(),
                control: Elevation::single(Shadow::black(0., 1., 2., 0.25)),
            };
        }

        Self {
            popover: Elevation::layered(
                Shadow::black(0., 4., 24., 0.12),
                Shadow::black(0., 1., 1., 0.05),
            ),
            dialog: Elevation::layered(
                Shadow::black(0., 8., 40., 0.14),
                Shadow::black(0., 1., 2., 0.06),
            ),
            control: Elevation::single(Shadow::black(0., 1., 2., 0.1)),
        }
    }
}

/// Overlay transition timings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Motion {
    /// The enter transition of popovers, menus, and dialogs.
    pub overlay: Duration,
}

impl Default for Motion {
    fn default() -> Self {
        Self {
            overlay: Duration::from_millis(200),
        }
    }
}

/// Overlay scrollbar presentation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scrollbar {
    /// The painted thumb's thickness.
    pub width: Pixels,
    /// Edge padding between the thumb and the viewport it floats over.
    pub inset: Pixels,
    /// The thumb never shrinks below this length, so it stays grabbable.
    pub min_thumb: Pixels,
    /// Whether the platform's scrollbars hide while idle.
    pub autohide: bool,
}

impl Default for Scrollbar {
    fn default() -> Self {
        Self {
            width: px(8.),
            inset: px(2.),
            min_thumb: px(24.),
            autohide: false,
        }
    }
}

/// User customizations layered over the resolved theme.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CustomTheme {
    pub root_font_size: Option<Pixels>,
    pub sm_radius: Option<Pixels>,
    pub control_radius: Option<Pixels>,
    pub overlay_radius: Option<Pixels>,
    pub popover_elevation: Option<Elevation>,
    pub dialog_elevation: Option<Elevation>,
    pub control_elevation: Option<Elevation>,
    /// Window material transparency as a percent (0 opaque, 100 pure blur).
    ///
    /// Note: windows platform is not supported
    pub transparency: Option<u8>,
}

/// The engine's knobs and its platform source.
struct ThemeEngine {
    source: Box<dyn source::ThemeSource>,
    mode_override: Option<ThemeVariant>,
    preset: Option<(&'static str, ThemeDefinition)>,
    custom: Option<CustomTheme>,
}

impl Global for ThemeEngine {}

/// Reads the resolved theme global from any app context.
pub trait ActiveTheme {
    fn theme(&self) -> &Theme;
}

impl ActiveTheme for App {
    fn theme(&self) -> &Theme {
        Theme::global(self)
    }
}

/// Reapplies the theme to the application.
pub(crate) fn reapply(cx: &mut App) {
    let (preset, mode_override, custom, system) = {
        let engine = cx.global::<ThemeEngine>();
        let system_appearance = ThemeVariant::from_appearance(cx.window_appearance());
        let preset = engine.preset.as_ref().map(|(_, def)| def.clone());
        let mode_override = engine.mode_override;
        let custom = engine.custom;
        let variant = mode_override
            .or(preset.as_ref().and_then(|def| def.mode()))
            .unwrap_or(system_appearance);
        (
            preset,
            mode_override,
            custom,
            engine.source.snapshot(variant),
        )
    };

    let resolved = resolve::resolve(&system, preset.as_ref(), mode_override, custom);

    cx.set_reduce_motion(resolved.accessibility.reduce_motion);
    let theme = Theme::from_resolved(resolved);
    let theme_changed = cx.try_global::<Theme>() != Some(&theme);
    cx.set_global(theme);
    if theme_changed {
        cx.refresh_windows();
    }
}

/// Forces a variant, independent of the OS. `None` follows the system.
pub fn set_mode_override(variant: Option<ThemeVariant>, cx: &mut App) {
    let engine = cx.global_mut::<ThemeEngine>();
    engine.mode_override = variant;
    reapply(cx);
}

/// Installs a built-in preset, or `None` to follow the system again.
pub fn set_preset(name: Option<&str>, cx: &mut App) -> anyhow::Result<()> {
    let preset = name
        .map(|name| {
            presets::builtin(name).ok_or_else(|| {
                anyhow::anyhow!(
                    "unknown preset: {name}; available: {}",
                    presets::preset_names().collect::<Vec<_>>().join(", ")
                )
            })
        })
        .transpose()?;
    let engine = cx.global_mut::<ThemeEngine>();
    engine.preset = preset;
    reapply(cx);
    Ok(())
}

/// Installs the user's custom theme, or `None` to follow the current scheme.
pub fn set_custom_theme(custom: Option<CustomTheme>, cx: &mut App) {
    let engine = cx.global_mut::<ThemeEngine>();
    engine.custom = custom;
    reapply(cx);
}

/// The variant the mode override forces, if any.
pub fn mode_override(cx: &App) -> Option<ThemeVariant> {
    let engine = cx.global::<ThemeEngine>();
    engine.mode_override
}

/// The installed custom theme, if any.
pub fn custom_theme(cx: &App) -> Option<CustomTheme> {
    let engine = cx.global::<ThemeEngine>();
    engine.custom
}

/// The active preset name, if one is installed.
pub fn active_preset(cx: &App) -> Option<&'static str> {
    let engine = cx.global::<ThemeEngine>();
    engine.preset.as_ref().map(|(name, _)| *name)
}

/// Clears the mode override and preset; the theme follows the system again.
pub fn follow_system(cx: &mut App) {
    let engine = cx.global_mut::<ThemeEngine>();
    engine.mode_override = None;
    engine.preset = None;
    reapply(cx);
}

/// Which adapter produced the current system snapshot.
pub fn detected_source(cx: &App) -> &'static str {
    let engine = cx.global::<ThemeEngine>();
    engine.source.label()
}

use gpui::{WindowBackgroundAppearance, px};

use crate::theme::definition::ThemeDefinition;
use crate::theme::ramps::{AccentShades, Oklch, contrast_foreground, derive_shades, mix};
use crate::theme::system::{SourceKind, SystemTheme};
use crate::theme::{
    ColorTokens, CustomTheme, Elevations, MaterialTokens, Motion, Radius, Scrollbar, SizeTable,
    ThemeVariant, Typography,
};

/// Derived state shades for the colors that get pressed and hovered.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ramps {
    pub primary: AccentShades,
    pub secondary: AccentShades,
    pub destructive: AccentShades,
    pub success: AccentShades,
    pub warning: AccentShades,
    pub info: AccentShades,
}

/// Everything a component paints from, after the overlay chain.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResolvedTheme {
    pub variant: ThemeVariant,
    pub colors: ColorTokens,
    /// The translucent paints over the window's native material.
    pub material: MaterialTokens,
    pub ramps: Ramps,
    pub radius: Radius,
    pub elevations: Elevations,
    pub motion: Motion,
    pub scrollbar: Scrollbar,
    /// The height the window's titlebar strips draw at.
    pub titlebar_height: gpui::Pixels,
    /// The theme's root font size.
    pub root_font_size: gpui::Pixels,
    /// The monospace family, for code surfaces.
    pub mono_font: &'static str,
    /// The type ladder, resolved at the root font size.
    pub typography: Typography,
    pub sizes: SizeTable,
    pub accessibility: crate::theme::system::AccessibilityPreferences,
    /// The material a window should open with, after accessibility.
    pub window_background: WindowBackgroundAppearance,
}

/// Overlay order (later wins):
/// neutral base → system → preset definition → custom theme.
pub(crate) fn resolve(
    system: &SystemTheme,
    preset: Option<&ThemeDefinition>,
    mode_override: Option<ThemeVariant>,
    custom: Option<CustomTheme>,
) -> ResolvedTheme {
    let custom = custom.unwrap_or_default();

    let variant = mode_override
        .or(preset.and_then(|def| def.mode()))
        .unwrap_or(system.variant);

    let base = match variant {
        ThemeVariant::Light => ColorTokens::light(),
        ThemeVariant::Dark => ColorTokens::dark(),
    };

    let def_color = |field: fn(&ThemeDefinition) -> Option<gpui::Hsla>| preset.and_then(field);

    let accent = def_color(|d| d.accent())
        .or(system.accent)
        .unwrap_or(base.accent);
    let primary = def_color(|d| d.primary()).unwrap_or(accent);
    let destructive = def_color(|d| d.destructive()).unwrap_or(base.destructive);
    let success = def_color(|d| d.success()).unwrap_or(base.success);
    let warning = def_color(|d| d.warning()).unwrap_or(base.warning);
    let info = def_color(|d| d.info()).unwrap_or(base.info);

    let mut colors = ColorTokens {
        background: def_color(|d| d.background()).unwrap_or(base.background),
        foreground: def_color(|d| d.foreground()).unwrap_or(base.foreground),
        surface: def_color(|d| d.surface()).unwrap_or(base.surface),
        surface_foreground: def_color(|d| d.surface_foreground())
            .unwrap_or(base.surface_foreground),
        primary,
        primary_foreground: def_color(|d| d.primary_foreground())
            .unwrap_or_else(|| contrast_foreground(primary)),
        secondary: def_color(|d| d.secondary()).unwrap_or(base.secondary),
        secondary_foreground: def_color(|d| d.secondary_foreground())
            .unwrap_or(base.secondary_foreground),
        muted: def_color(|d| d.muted()).unwrap_or(base.muted),
        muted_hover: def_color(|d| d.muted_hover()).unwrap_or(base.muted_hover),
        muted_foreground: def_color(|d| d.muted_foreground()).unwrap_or(base.muted_foreground),
        accent,
        accent_foreground: def_color(|d| d.accent_foreground())
            .unwrap_or_else(|| contrast_foreground(accent)),
        destructive,
        destructive_foreground: def_color(|d| d.destructive_foreground())
            .unwrap_or_else(|| contrast_foreground(destructive)),
        success,
        success_foreground: def_color(|d| d.success_foreground())
            .unwrap_or_else(|| contrast_foreground(success)),
        warning,
        warning_foreground: def_color(|d| d.warning_foreground())
            .unwrap_or_else(|| contrast_foreground(warning)),
        info,
        info_foreground: def_color(|d| d.info_foreground())
            .unwrap_or_else(|| contrast_foreground(info)),
        border: def_color(|d| d.border()).unwrap_or(base.border),
        input: def_color(|d| d.input()).unwrap_or(base.input),
        ring: def_color(|d| d.ring()).unwrap_or(accent),
        selection: def_color(|d| d.selection()).unwrap_or(base.selection),
        overlay: def_color(|d| d.overlay()).unwrap_or(base.overlay),
        scrollbar_thumb: def_color(|d| d.scrollbar_thumb()).unwrap_or(base.scrollbar_thumb),
    };

    // The knob scales the fill over a platform-stated blur.
    let transparency = custom
        .transparency
        .map(|level| level.min(100))
        .filter(|_| system.source != SourceKind::Windows && system.window_background.is_some());

    // Reduced transparency forces the window opaque.
    let window_background = if system.accessibility.reduce_transparency || transparency == Some(0) {
        WindowBackgroundAppearance::Opaque
    } else {
        system
            .window_background
            .unwrap_or(WindowBackgroundAppearance::Opaque)
    };

    let accessibility = system.accessibility;
    if accessibility.high_contrast {
        colors.border = mix(
            Oklch::from_hsla(colors.border),
            Oklch::from_hsla(colors.foreground),
            0.35,
        )
        .to_hsla();
    }

    // A custom theme's stated font size wins over the 16px design root.
    let root_font_size = custom.root_font_size.unwrap_or(px(16.));
    let sizes = SizeTable::with_root_font_size(root_font_size);
    let titlebar_height = root_font_size * 2.5;

    let mut radius = Radius::default();

    if let Some(control) = custom
        .control_radius
        .or(preset.and_then(|def| def.control_radius()).map(px))
    {
        radius.control = control;
    }

    if let Some(overlay) = custom
        .overlay_radius
        .or(preset.and_then(|def| def.overlay_radius()).map(px))
    {
        radius.overlay = overlay;
    }

    if let Some(sm) = custom
        .sm_radius
        .or(preset.and_then(|def| def.sm_radius()).map(px))
    {
        radius.sm = sm;
    }

    let ramps = Ramps {
        primary: derive_shades(primary, colors.background, variant == ThemeVariant::Dark),
        secondary: derive_shades(
            colors.secondary,
            colors.background,
            variant == ThemeVariant::Dark,
        ),
        destructive: derive_shades(
            destructive,
            colors.background,
            variant == ThemeVariant::Dark,
        ),
        success: derive_shades(success, colors.background, variant == ThemeVariant::Dark),
        warning: derive_shades(warning, colors.background, variant == ThemeVariant::Dark),
        info: derive_shades(info, colors.background, variant == ThemeVariant::Dark),
    };

    if def_color(|d| d.selection()).is_none() {
        colors.selection = ramps.primary.subtle;
    }

    let material = MaterialTokens::derive(
        &colors,
        ramps.primary.base,
        variant,
        window_background != WindowBackgroundAppearance::Opaque,
        transparency,
    );

    let default_elevations = Elevations::for_variant(variant);
    let elevations = Elevations {
        popover: custom
            .popover_elevation
            .unwrap_or(default_elevations.popover),
        dialog: custom.dialog_elevation.unwrap_or(default_elevations.dialog),
        control: custom
            .control_elevation
            .unwrap_or(default_elevations.control),
    };

    ResolvedTheme {
        variant,
        ramps,
        colors,
        material,
        radius,
        elevations,
        motion: Motion::default(),
        scrollbar: Scrollbar {
            autohide: system.source.scrollbar_autohide(),
            ..Scrollbar::default()
        },
        titlebar_height,
        root_font_size,
        mono_font: mono_font(),
        typography: Typography::resolve(root_font_size),
        sizes,
        accessibility,
        window_background,
    }
}

/// The platform's monospace family, for code surfaces.
fn mono_font() -> &'static str {
    if cfg!(target_os = "macos") {
        "Menlo"
    } else if cfg!(target_os = "windows") {
        "Consolas"
    } else {
        "monospace"
    }
}

//! Built-in `ThemeDefinition` presets (D11).

use crate::theme::definition::ThemeDefinition;

/// The bundled presets: `(name, TOML source)`. Enumeration and lookup both
/// read this one table.
const PRESETS: &[(&str, &str)] = &[("tokyo-night", include_str!("presets/tokyo-night.toml"))];

/// Every bundled preset's name.
pub fn preset_names() -> impl Iterator<Item = &'static str> {
    PRESETS.iter().map(|(name, _)| *name)
}

/// Parses the named preset, returning its static name and definition.
pub fn builtin(name: &str) -> Option<(&'static str, ThemeDefinition)> {
    let (name, source) = PRESETS
        .iter()
        .find(|(candidate, _)| *candidate == name)
        .copied()?;
    match ThemeDefinition::parse_toml(source) {
        Ok(definition) => Some((name, definition)),
        Err(error) => {
            log::error!("bundled preset {name} failed to parse: {error}");
            None
        }
    }
}

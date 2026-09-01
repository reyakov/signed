//! Persisted application settings for Signed.
//!
//! The [`Settings`] model holds the user-configurable values that survive
//! restarts, and [`SettingsStore`] loads them from and saves them to a JSON
//! file on disk (see [`paths::settings_file`]).

mod settings;
mod store;

pub use settings::*;
pub use store::*;

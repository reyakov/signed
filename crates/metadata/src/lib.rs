mod profile;

use gpui::{App, AppContext};
pub use profile::{Profile, ProfileDirectory, ProfileEvent, ProfileStore};

/// Initialize the global profile store.
pub fn init(cx: &mut App) {
    ProfileStore::set_global(cx.new(ProfileStore::new), cx);
}

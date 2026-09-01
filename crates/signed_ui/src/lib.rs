//! Reusable UI components and elements for Signed.
//!
//! Everything here is presentation-only: the components read theme tokens
//! through [`gpui_component::ActiveTheme`] and render with plain GPUI
//! elements, so any view in the app can use them without depending on app
//! state. They are built on the unstyled `gpui-base` primitives and the
//! styled `gpui-component` library.
//!
//! The crate is organized by component:
//!
//! - [`PixelAvatar`] — deterministic, offline "pixel art" avatar
//! - [`NavItem`] — sidebar navigation row (leading element + label + suffix)
//! - [`DropdownButton`] — split button: an action plus a caret that opens a
//!   [`PopupMenu`], wired through `gpui_base::Popover`
//! - [`SegmentButton`] / [`CountBadge`] — segmented filter/tab button with an
//!   optional count badge
//! - [`UserAvatar`] — user picture avatar with a name-initials fallback
//! - [`status_badge`] — NIP-34 issue/PR status badge
//! - [`placeholder`] — centered muted placeholder message
//! - [`copy_row`] / [`menu_copy_row`] — rows with a copy-to-clipboard button
//! - [`tree_row`] — one row of a file tree
//! - [`setting_row`] / [`setting_block`] — label + description rows for a
//!   settings dialog, with the control on the right (row) or below (block)
//! - [`SelectOption`] — dropdown option with a display label and a stored
//!   value
//! - [`title_bar_drag_handlers`] — make an element behave like a window
//!   title bar (drag moves the window, double-click zooms)
//! - [`image_cache`] — per-view LRU image cache provider
//! - [`middle_truncate`] — `[head]...[tail]` string truncation

mod dropdown_button;
mod nav_item;
mod pixel_avatar;
mod placeholder;
mod segment_button;
mod setting;
mod status_badge;
mod title_bar;
mod tree_row;
mod user_avatar;

pub mod copy_row;
pub mod image_cache;
pub mod util;

pub use copy_row::{copy_row, menu_copy_row};
pub use dropdown_button::DropdownButton;
pub use image_cache::{MAX_IMAGES, image_cache};
pub use nav_item::NavItem;
pub use pixel_avatar::PixelAvatar;
pub use placeholder::placeholder;
pub use segment_button::{CountBadge, SegmentButton};
pub use setting::{SelectOption, setting_block, setting_row};
pub use status_badge::status_badge;
pub use title_bar::title_bar_drag_handlers;
pub use tree_row::tree_row;
pub use user_avatar::UserAvatar;
pub use util::middle_truncate;

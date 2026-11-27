//! Application window layout: a titlebar over a panel stack.
//!
//! The shared flex and focus helpers live in [`crate::components::common`];
//! they are re-exported here for the `ui::layout::…` paths.

pub use crate::components::common::{StyledExt, h_flex, v_flex};

pub mod panel;
pub mod resizable;
pub mod resize_handle;
pub mod root;
pub mod titlebar;
pub mod window_controls;

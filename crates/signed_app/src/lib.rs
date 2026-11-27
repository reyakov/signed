mod app;
mod explore;
mod identity;
#[expect(dead_code, reason = "unlinked while the shell migrates")]
mod inbox;
mod issues;
mod pull_requests;
mod repo;
mod settings;
mod shared;
mod titlebar;

pub use app::AppRoot;

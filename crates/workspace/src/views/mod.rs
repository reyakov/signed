mod commit_diff;
mod dialog_state;
pub(crate) mod discussion;
mod inbox;
mod issues;
mod pull_requests;
mod repo;
mod repo_list;
mod send_patch;
pub(crate) mod sidebar;
pub(crate) mod tree;

pub use inbox::InboxView;
pub use repo::RepoDetailView;
pub(crate) use repo::{RepoItem, open_repo_item, open_repo_panel};
pub use repo_list::RepoListView;
pub use sidebar::SidebarPanel;

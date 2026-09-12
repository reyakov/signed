mod dialog_state;
mod inbox;
mod repo_detail;
mod repo_list;
pub(crate) mod sidebar;

pub use inbox::InboxView;
pub use repo_detail::RepoDetailView;
pub(crate) use repo_detail::{RepoItem, open_repo_item, open_repo_panel};
pub use repo_list::RepoListView;
pub use sidebar::SidebarPanel;

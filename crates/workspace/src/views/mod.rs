mod repo_detail;
mod repo_list;
pub(crate) mod sidebar;

pub use repo_detail::RepoDetailView;
pub(crate) use repo_detail::open_repo_panel;
pub use repo_list::RepoListView;
pub use sidebar::SidebarPanel;

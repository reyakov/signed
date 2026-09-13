mod cache;
mod diff;
mod history;
mod patch;
mod remote;
mod repo;
mod scan;
mod worktree;

#[cfg(test)]
mod tests;

pub use cache::{GitCache, fork_namespace, sanitize_path_component};
pub use diff::{
    CommitDiff, DiffHunk, DiffLine, DiffLineKind, DiffStatus, FileDiff, worktree_commit_diff,
    worktree_commit_range_diff,
};
pub use history::{
    CommitList, FileCommit, MAX_LISTED_COMMITS, all_commits, head_commit, worktree_all_commits,
    worktree_commit, worktree_commit_range_commits, worktree_last_commits,
};
pub use patch::{
    apply_patch, format_patch_between, patch_commits, patch_diffs, split_patch_series,
};
pub use remote::{
    clone_repo, ensure_origin, fetch_all, fetch_repo_refs, origin_url, push_all, push_commit_ref,
    push_main, remote_has_refs, set_origin,
};
pub use repo::{
    RepoRefState, commits_since, current_branch, delete_refs_with_prefix, fast_forward_branches,
    head_commit_id, init_repository, merge_base, refs_with_prefix, repo_branches, repo_ref_state,
    repo_tags, root_commit, worktree_branches, worktree_current_branch, worktree_ref_exists,
    worktree_ref_state,
};
pub use scan::find_git_repos;
pub use worktree::{
    WorktreeSnapshot, find_readme, worktree_checkout_branch, worktree_checkout_tag,
    worktree_commits_ahead, worktree_dirty, worktree_entries, worktree_read, worktree_snapshot,
};

/// The terminal prompt is disabled so a credential request fails instead of hanging.
#[cfg(test)]
fn git_in(dir: &std::path::Path, args: &[&str]) -> anyhow::Result<String> {
    let output = remote::git_output(dir, args, "git")?;

    if !output.status.success() {
        anyhow::bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

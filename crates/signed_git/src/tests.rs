use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use nostr::prelude::*;
use signed_core::{Announcement, repo_addr};

use super::*;

#[test]
fn keeps_plain_ids() {
    assert_eq!(sanitize_path_component("my-repo"), "my-repo");
    assert_eq!(sanitize_path_component("repo.v2"), "repo.v2");
    assert_eq!(sanitize_path_component("a_b-c"), "a_b-c");
}

#[test]
fn replaces_unsafe_characters() {
    assert_eq!(sanitize_path_component("a/b\\c:d"), "a_b_c_d");
    assert_eq!(sanitize_path_component(""), "");
}

#[test]
fn blocks_parent_components() {
    assert_eq!(sanitize_path_component(".."), "_");
    assert_eq!(sanitize_path_component("."), "_");
    // Separators are neutralized before the check, so these stay safe.
    assert_eq!(sanitize_path_component("../.."), ".._..");
    assert_eq!(sanitize_path_component("a/../b"), "a_.._b");
}

#[test]
fn fork_namespace_combines_owner_and_sanitized_id() {
    let keys = Keys::generate();
    let event = EventBuilder::new(Kind::GitRepoAnnouncement, "")
        .tags([Tag::parse(["d", "my/repo"]).expect("valid tag")])
        .finalize(&keys)
        .expect("signed event");
    let announcement = Announcement::from_event(&event).expect("parses");

    assert_eq!(
        fork_namespace(&announcement),
        format!("{}/my_repo", keys.public_key().to_hex())
    );
}

#[test]
fn repo_path_stays_inside_root() {
    let cache = GitCache::new("/cache".into());
    let owner = Keys::generate().public_key();

    let path = cache.repo_path(&repo_addr(owner, ".."));
    assert!(path.starts_with("/cache"));
    assert_eq!(
        path.file_name().map(|n| n.to_string_lossy().into_owned()),
        Some("_".into())
    );
}

#[test]
fn find_git_repos_discovers_repositories_recursively() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    // Repositories are found at any depth.
    // A linked worktree, with a `.git` file instead of a directory, counts too.
    let nested = root.join("a/b/project");
    std::fs::create_dir_all(nested.join(".git")).unwrap();
    let worktree = root.join("wt");
    std::fs::create_dir_all(&worktree).unwrap();
    std::fs::write(
        worktree.join(".git"),
        "gitdir: ../a/b/project/.git/worktrees/wt",
    )
    .unwrap();

    // Plain directories are not repositories.
    std::fs::create_dir_all(root.join("plain")).unwrap();

    // A `.gitignore` at the root excludes dependency caches.
    std::fs::write(root.join(".gitignore"), "node_modules/\n").unwrap();
    std::fs::create_dir_all(root.join("node_modules/pkg/.git")).unwrap();

    // Hidden entries are skipped.
    std::fs::create_dir_all(root.join(".hidden/repo/.git")).unwrap();

    // A repository is not descended into.
    // Repositories inside it, like submodule worktrees, are not reported.
    let outer = root.join("outer");
    std::fs::create_dir_all(outer.join(".git")).unwrap();
    std::fs::create_dir_all(outer.join("sub/other/.git")).unwrap();

    let mut found = find_git_repos(root);
    found.sort();

    let mut expected = vec![
        nested.canonicalize().unwrap(),
        worktree.canonicalize().unwrap(),
        outer.canonicalize().unwrap(),
    ];
    expected.sort();
    assert_eq!(found, expected);
}

#[test]
fn root_commit_reports_the_first_ancestor() {
    let (dir, repo) = fixture(&[("a.txt", b"one")]);
    commit_all(&repo, "initial");

    let dir = dir.path();
    let root = root_commit(dir).expect("root").expect("commit");
    assert_eq!(root.len(), 40);

    // The root commit does not change when history grows.
    std::fs::write(dir.join("b.txt"), b"two").expect("write");
    commit_all(&repo, "second");
    assert_eq!(
        root_commit(dir).expect("root").as_deref(),
        Some(root.as_str())
    );
}

#[test]
fn root_commit_is_none_without_commits() {
    let (_dir, repo) = fixture(&[("a.txt", b"one")]);
    let workdir = repo.workdir().expect("workdir");
    assert_eq!(root_commit(workdir).expect("root"), None);
}

#[test]
fn push_all_mirrors_branches_and_tags() {
    // A bare server repository reachable via a `file://` URL.
    // Mirrors a grasp server's `{base}/{owner}/{repo-id}.git` layout.
    let server = tempfile::tempdir().unwrap();
    let server_repo = server.path().join("npub1test").join("my-repo.git");
    std::fs::create_dir_all(server_repo.parent().unwrap()).unwrap();
    let init_status = Command::new("git")
        .args(["init", "--bare", "-q"])
        .arg(&server_repo)
        .status()
        .expect("spawn git init --bare");
    assert!(init_status.success());

    let (dir, repo) = fixture(&[("a.txt", b"one")]);
    commit_all(&repo, "initial");
    let dir = dir.path();

    // Two branches plus a tag are all mirrored.
    git_run(dir, &["checkout", "-b", "feature"]);
    std::fs::write(dir.join("b.txt"), b"two").expect("write");
    commit_all(&repo, "feature work");
    git_run(dir, &["checkout", "-"]);
    git_run(dir, &["tag", "v1.0"]);

    let base_url = format!("file://{}", server.path().display());
    push_all(dir, &base_url, "npub1test", "my-repo").expect("push");

    let refs = git_in(&server_repo, &["show-ref"]).expect("server refs");
    assert!(refs.contains("refs/heads/main"));
    assert!(refs.contains("refs/heads/feature"));
    assert!(refs.contains("refs/tags/v1.0"));
}

#[test]
fn push_all_tolerates_a_missing_ref_kind() {
    // A repository with only tags and no branches still pushes.
    // Wildcard refspecs without a local match are ignored.
    let server = tempfile::tempdir().unwrap();
    let server_repo = server.path().join("npub1test").join("my-repo.git");
    std::fs::create_dir_all(server_repo.parent().unwrap()).unwrap();
    let init_status = Command::new("git")
        .args(["init", "--bare", "-q"])
        .arg(&server_repo)
        .status()
        .expect("spawn git init --bare");
    assert!(init_status.success());

    let (dir, repo) = fixture(&[("a.txt", b"one")]);
    commit_all(&repo, "initial");
    let dir = dir.path();
    git_run(dir, &["tag", "v1.0"]);
    git_run(dir, &["update-ref", "-d", "refs/heads/main"]);

    let base_url = format!("file://{}", server.path().display());
    push_all(dir, &base_url, "npub1test", "my-repo").expect("push");

    let refs = git_in(&server_repo, &["show-ref"]).expect("server refs");
    assert!(refs.contains("refs/tags/v1.0"));
    assert!(!refs.contains("refs/heads/"));
}

#[test]
fn remote_has_refs_reports_whether_pushed_refs_landed() {
    let server = tempfile::tempdir().unwrap();
    let server_repo = server.path().join("npub1test").join("my-repo.git");
    std::fs::create_dir_all(server_repo.parent().unwrap()).unwrap();
    let init_status = Command::new("git")
        .args(["init", "--bare", "-q"])
        .arg(&server_repo)
        .status()
        .expect("spawn git init --bare");
    assert!(init_status.success());

    let (dir, repo) = fixture(&[("a.txt", b"one")]);
    commit_all(&repo, "initial");
    let dir = dir.path();
    let main = git_in(dir, &["rev-parse", "refs/heads/main"]).expect("main oid");
    let url = format!("file://{}/npub1test/my-repo.git", server.path().display());
    let expected = vec![("refs/heads/main".to_owned(), main.clone())];

    // Nothing pushed yet: the ref is absent.
    assert!(!remote_has_refs(dir, &url, &expected).expect("probe"));

    push_all(
        dir,
        &format!("file://{}", server.path().display()),
        "npub1test",
        "my-repo",
    )
    .expect("push");

    // The pushed ref is advertised at the expected commit.
    assert!(remote_has_refs(dir, &url, &expected).expect("probe"));

    // A stale expectation - the exact race a retry resolves - is false.
    let stale = vec![("refs/heads/main".to_owned(), "0".repeat(40))];
    assert!(!remote_has_refs(dir, &url, &stale).expect("probe"));

    // Extra remote refs (e.g. a tag pushed later) do not invalidate the
    // refs this push wanted to land.
    git_run(dir, &["tag", "v1.0"]);
    push_all(
        dir,
        &format!("file://{}", server.path().display()),
        "npub1test",
        "my-repo",
    )
    .expect("push");
    assert!(remote_has_refs(dir, &url, &expected).expect("probe"));
}

#[test]
fn repo_ref_state_lists_branches_tags_and_head() {
    let (_dir, repo) = fixture(&[("a.txt", b"hello")]);
    commit_all(&repo, "initial");
    let workdir = repo.workdir().expect("workdir").to_path_buf();

    let state = repo_ref_state(&repo).expect("refs");

    let branch = current_branch(&repo).expect("branch").expect("on a branch");
    assert_eq!(state.head.as_deref(), Some(branch.as_str()));
    assert_eq!(state.refs.len(), 1);
    assert_eq!(state.refs[0].0, format!("refs/heads/{branch}"));
    assert_eq!(state.refs[0].1.len(), 40);

    // Additional branches and tags are listed alongside.
    git_run(&workdir, &["branch", "feature"]);
    git_run(&workdir, &["tag", "v1.0"]);

    let state = repo_ref_state(&repo).expect("refs");
    let mut expected: Vec<String> = vec![
        format!("refs/heads/{branch}"),
        "refs/heads/feature".to_owned(),
        "refs/tags/v1.0".to_owned(),
    ];
    expected.sort();
    assert_eq!(
        state
            .refs
            .iter()
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>(),
        expected
    );

    // A detached HEAD yields no head branch.
    git_run(&workdir, &["checkout", "--detach"]);
    let state = repo_ref_state(&repo).expect("refs");
    assert!(state.head.is_none());
    assert_eq!(state.refs.len(), 3);
}

/// Build a throwaway non-bare repository from `(rel, bytes)` file pairs.
fn fixture(files: &[(&str, &[u8])]) -> (tempfile::TempDir, gix::Repository) {
    let dir = tempfile::tempdir().expect("tempdir");
    let repo = gix::init(&dir).expect("init");

    for (rel, bytes) in files {
        let path = dir.path().join(rel);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(&path, bytes).expect("write");
    }

    (dir, repo)
}

#[test]
fn worktree_entries_lists_all_files_and_dirs() {
    let (_dir, repo) = fixture(&[
        ("README.md", b"# Hi"),
        ("src/main.rs", b"fn main() {}"),
        ("src/lib.rs", b""),
        ("docs/guide.md", b"guide"),
    ]);

    let entries = worktree_entries(&repo).expect("entries");
    let entries: Vec<String> = entries
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();

    assert_eq!(
        entries,
        vec![
            "docs",
            "src",
            "README.md",
            "docs/guide.md",
            "src/lib.rs",
            "src/main.rs"
        ]
    );
}

#[test]
fn worktree_read_returns_bytes_or_none() {
    let (_dir, repo) = fixture(&[("a.txt", b"hello"), ("sub/b.bin", b"\x00\x01")]);

    assert_eq!(
        worktree_read(&repo, Path::new("a.txt")).expect("read"),
        Some(b"hello".to_vec())
    );
    assert_eq!(
        worktree_read(&repo, Path::new("sub/b.bin")).expect("read"),
        Some(vec![0x00, 0x01])
    );
    assert_eq!(
        worktree_read(&repo, Path::new("missing.txt")).expect("read"),
        None
    );
}

/// Stage everything and create a commit with the git CLI.
/// Like [`apply_patch`], the crate already shells out to the CLI.
fn commit_all(repo: &gix::Repository, message: &str) {
    git_run(repo.workdir().expect("workdir"), &["add", "-A"]);
    git_run(repo.workdir().expect("workdir"), &["commit", "-m", message]);
}

#[test]
fn merge_base_finds_the_fork_point_and_reports_unrelated_history() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("repo");
    let initial = init_repository(&path, "My Repo", "desc").expect("init");

    // A feature branch and a mainline commit diverge from the initial commit.
    // The initial commit is their merge base.
    git_run(&path, &["checkout", "-b", "feature"]);
    std::fs::write(path.join("feature.txt"), "feature\n").expect("write");
    commit_all(&gix::open(&path).expect("open"), "feature commit");
    git_run(&path, &["checkout", "main"]);
    std::fs::write(path.join("main.txt"), "main\n").expect("write");
    commit_all(&gix::open(&path).expect("open"), "mainline commit");

    assert_eq!(
        merge_base(&path, "feature", "main")
            .expect("merge base")
            .as_deref(),
        Some(initial.as_str())
    );

    // An orphan branch shares no history with main, so `Ok(None)`.
    git_run(&path, &["checkout", "--orphan", "orphan"]);
    std::fs::write(path.join("orphan.txt"), "orphan\n").expect("write");
    commit_all(&gix::open(&path).expect("open"), "orphan commit");
    assert_eq!(merge_base(&path, "orphan", "main").expect("ok"), None);

    // An unresolvable revision is an error, not a missing ancestor.
    assert!(merge_base(&path, "orphan", "no-such-ref").is_err());
}

#[test]
fn format_patch_between_produces_the_series_and_rejects_empty_ranges() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("repo");
    let initial = init_repository(&path, "My Repo", "desc").expect("init");

    git_run(&path, &["checkout", "-b", "feature"]);
    std::fs::write(path.join("feature.txt"), "feature\n").expect("write");
    commit_all(&gix::open(&path).expect("open"), "feature commit");

    let patch = format_patch_between(&path, &initial, "feature").expect("patch");
    assert!(patch.contains("Subject: [PATCH] feature commit"));
    assert!(patch.contains("feature.txt"));

    // An empty range has no commits to send.
    assert!(format_patch_between(&path, "feature", "feature").is_err());
}

#[test]
fn push_commit_ref_pushes_to_the_event_namespace() {
    // A bare server repository reachable via a `file://` URL.
    // Mirrors a grasp server's `{base}/{owner}/{repo-id}.git` layout.
    let server = tempfile::tempdir().unwrap();
    let server_repo = server.path().join("npub1test").join("my-repo.git");
    std::fs::create_dir_all(server_repo.parent().unwrap()).unwrap();
    let init_status = Command::new("git")
        .args(["init", "--bare", "-q"])
        .arg(&server_repo)
        .status()
        .expect("spawn git init --bare");
    assert!(init_status.success());

    let (dir, repo) = fixture(&[("a.txt", b"one")]);
    commit_all(&repo, "initial");
    let dir = dir.path();
    let tip = git_in(dir, &["rev-parse", "HEAD"]).expect("tip");

    let url = format!("file://{}/npub1test/my-repo.git", server.path().display());
    push_commit_ref(dir, &url, &tip, "refs/nostr/abcd1234").expect("push");

    let refs = git_in(&server_repo, &["show-ref"]).expect("server refs");
    assert!(refs.contains("refs/nostr/abcd1234"));
}

#[test]
fn split_patch_series_splits_real_multi_commit_mboxes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("repo");
    let initial = init_repository(&path, "My Repo", "desc").expect("init");

    git_run(&path, &["checkout", "-b", "feature"]);
    std::fs::write(path.join("one.txt"), "one\n").expect("write");
    commit_all(&gix::open(&path).expect("open"), "first commit");
    std::fs::write(path.join("two.txt"), "two\n").expect("write");
    commit_all(&gix::open(&path).expect("open"), "second commit");

    let series = format_patch_between(&path, &initial, "feature").expect("series");
    let parts = split_patch_series(&series);

    assert_eq!(parts.len(), 2);
    assert!(parts[0].contains("Subject: [PATCH 1/2] first commit"));
    assert!(parts[1].contains("Subject: [PATCH 2/2] second commit"));
    // Each part starts its own mbox message with its own commit id.
    let first = parts[0].lines().next().expect("first header");
    let second = parts[1].lines().next().expect("second header");
    assert!(first.starts_with("From ") && first.len() >= 45);
    assert_ne!(first, second);
}

#[test]
fn split_patch_series_keeps_single_patches_whole() {
    let patch = "From abcdefabcdefabcdefabcdefabcdefabcdefab Mon Sep 17 00:00:00 2001\nFrom: A <a@b>\nSubject: [PATCH] fix\n\n---\n";
    let parts = split_patch_series(patch);
    assert_eq!(parts.len(), 1);
    assert_eq!(parts[0], patch);
}

#[test]
fn head_commit_and_commits_since_track_applied_commits() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("repo");
    let initial = init_repository(&path, "My Repo", "desc").expect("init");

    assert_eq!(
        head_commit_id(&path).expect("head").as_deref(),
        Some(initial.as_str())
    );
    // No commits yet, `HEAD` alone.
    assert_eq!(
        commits_since(&path, None).expect("commits"),
        vec![initial.clone()]
    );

    std::fs::write(path.join("one.txt"), "one\n").expect("write");
    commit_all(&gix::open(&path).expect("open"), "first commit");
    let first = head_commit_id(&path).expect("head").expect("on a branch");

    std::fs::write(path.join("two.txt"), "two\n").expect("write");
    commit_all(&gix::open(&path).expect("open"), "second commit");
    let second = head_commit_id(&path).expect("head").expect("on a branch");

    // Oldest first, like the order `git am` creates them.
    assert_eq!(
        commits_since(&path, Some(&initial)).expect("commits"),
        vec![first.clone(), second.clone()]
    );
    assert_eq!(
        commits_since(&path, Some(&first)).expect("commits"),
        vec![second]
    );
}

#[test]
fn head_commit_reports_unborn_repositories() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("repo");
    let status = Command::new("git")
        .args(["init", "-q"])
        .arg(&path)
        .status()
        .expect("spawn git init");
    assert!(status.success());

    assert_eq!(head_commit_id(&path).expect("head"), None);
    assert_eq!(
        commits_since(&path, None).expect("commits"),
        Vec::<String>::new()
    );
}

#[test]
fn init_repository_creates_main_branch_and_readme() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("my-repo");

    let commit = init_repository(&path, "My Repo", "Does things.\n\nCool.").expect("init");
    assert_eq!(commit.len(), 40);

    let repo = gix::open(&path).expect("open");
    let workdir = repo.workdir().expect("workdir");

    assert_eq!(
        std::fs::read_to_string(workdir.join("README.md")).expect("read"),
        "# My Repo\n\nDoes things.\n\nCool.\n"
    );

    let branch = current_branch(&repo).expect("branch").expect("on a branch");
    assert_eq!(branch, "main");
    // [`FileCommit`] carries the short id, the full id is 40 chars.
    assert_eq!(
        head_commit(&repo).expect("head").expect("commit").id,
        &commit[..7]
    );

    let state = repo_ref_state(&repo).expect("refs");
    assert_eq!(state.head.as_deref(), Some("main"));
    assert_eq!(state.refs, vec![("refs/heads/main".to_owned(), commit)]);

    // The index matches the committed tree, so the fresh repo is clean.
    assert!(!worktree_dirty(workdir));
}

#[test]
fn init_repository_omits_description_when_empty() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("my-repo");

    init_repository(&path, "My Repo", "   ").expect("init");
    let repo = gix::open(&path).expect("open");
    let workdir = repo.workdir().expect("workdir");

    assert_eq!(
        std::fs::read_to_string(workdir.join("README.md")).expect("read"),
        "# My Repo\n"
    );
}

#[test]
fn ensure_origin_adds_remote_only_once() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("my-repo");
    init_repository(&path, "My Repo", "").expect("init");

    ensure_origin(&path, "https://gitnostr.com/npub1test/repo.git").expect("add");
    assert_eq!(
        git_in(&path, &["remote", "get-url", "origin"]).expect("url"),
        "https://gitnostr.com/npub1test/repo.git"
    );
    // The standard fetch mapping is configured with the remote.
    // Later `git fetch origin` updates `refs/remotes/origin/*`.
    assert_eq!(
        git_in(&path, &["config", "remote.origin.fetch"]).expect("refspec"),
        "+refs/heads/*:refs/remotes/origin/*"
    );

    // A second call must not override the existing remote.
    ensure_origin(&path, "https://other.example/repo.git").expect("keep");
    assert_eq!(
        git_in(&path, &["remote", "get-url", "origin"]).expect("url"),
        "https://gitnostr.com/npub1test/repo.git"
    );
}

#[test]
fn origin_url_reads_the_remote_or_reports_none() {
    let (dir, _repo) = fixture(&[("a.txt", b"one")]);
    commit_all(&_repo, "initial");
    let dir = dir.path();

    // No remote configured yet.
    assert_eq!(origin_url(dir).expect("read"), None);

    ensure_origin(dir, "https://gitnostr.com/npub1test/repo.git").expect("add");
    assert_eq!(
        origin_url(dir).expect("read").as_deref(),
        Some("https://gitnostr.com/npub1test/repo.git")
    );
}

#[test]
fn set_origin_creates_or_replaces_the_remote() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("my-repo");
    init_repository(&path, "My Repo", "").expect("init");

    // No origin yet, so one is added.
    set_origin(&path, "https://gitnostr.com/npub1test/repo.git").expect("add");
    assert_eq!(
        origin_url(&path).expect("url").as_deref(),
        Some("https://gitnostr.com/npub1test/repo.git")
    );

    // An existing origin is replaced, not duplicated.
    // A clone's origin points at the cloned-from path.
    // It is re-targeted at the grasp server.
    set_origin(&path, "https://grasp.example/npub1test/repo.git").expect("replace");
    assert_eq!(
        origin_url(&path).expect("url").as_deref(),
        Some("https://grasp.example/npub1test/repo.git")
    );
}

#[test]
fn working_copy_cloned_from_the_mirror_matches_head_and_origin() {
    // The mirror is a freshly initialized repository, standing in for
    // the grasp server. Its `origin` points at the (fake) grasp server.
    // `GitCache::ensure_clone` lazily clones from a URL shaped like this
    // the first time a repository is opened.
    let dir = tempfile::tempdir().expect("tempdir");
    let mirror = dir.path().join("mirror");
    let commit = init_repository(&mirror, "My Repo", "Does things.").expect("init");
    ensure_origin(&mirror, "https://gitnostr.com/npub1test/my-repo.git").expect("origin");

    // The working copy is cloned from the mirror.
    // It then shares the announced history exactly.
    // `origin` is re-pointed at the grasp server instead of the mirror path.
    let destination = dir.path().join("folder").join("My_Repo");
    std::fs::create_dir_all(destination.parent().unwrap()).expect("parent");
    clone_repo(&[format!("file://{}", mirror.display())], &destination).expect("clone");
    set_origin(&destination, "https://gitnostr.com/npub1test/my-repo.git").expect("set origin");

    assert_eq!(
        origin_url(&destination).expect("url").as_deref(),
        Some("https://gitnostr.com/npub1test/my-repo.git")
    );
    assert_eq!(
        head_commit_id(&destination).expect("head").as_deref(),
        Some(commit.as_str())
    );
    assert!(destination.join("README.md").is_file());
}

#[test]
fn fast_forward_branches_moves_the_mirror_and_keeps_local_work() {
    // A bare server, like a grasp server's `{base}/{owner}/{repo}.git` layout.
    let dir = tempfile::tempdir().expect("tempdir");
    let base_server = dir.path().join("npub1test").join("repo.git");
    std::fs::create_dir_all(base_server.parent().unwrap()).unwrap();
    let init_status = Command::new("git")
        .args(["init", "--bare", "-q"])
        .arg(&base_server)
        .status()
        .expect("spawn git init --bare");
    assert!(init_status.success());

    // The owner's working repo pushes the initial commit.
    let (work_dir, work_repo) = fixture(&[("a.txt", b"one")]);
    commit_all(&work_repo, "initial");
    let work = work_dir.path();
    let base_url = format!("file://{}", dir.path().display());
    push_all(work, &base_url, "npub1test", "repo").expect("push");

    // A mirror clone, like the app's GitCache clones.
    let mirror = dir.path().join("mirror");
    git_run(
        dir.path(),
        &[
            "clone",
            "-q",
            &format!("{base_url}/npub1test/repo.git"),
            mirror.to_str().unwrap(),
        ],
    );
    let initial = git_in(&mirror, &["rev-parse", "HEAD"]).expect("initial");

    // The owner pushes a new commit.
    // The mirror fetches it, but its local `main` and worktree stay behind.
    std::fs::write(work.join("new.txt"), b"new\n").expect("write");
    commit_all(&gix::open(work).expect("open"), "new commit");
    push_all(work, &base_url, "npub1test", "repo").expect("push");
    git_run(&mirror, &["fetch", "origin"]);
    let remote = git_in(&mirror, &["rev-parse", "refs/remotes/origin/main"]).expect("remote");
    assert_eq!(
        git_in(&mirror, &["rev-parse", "HEAD"]).expect("local"),
        initial
    );
    assert_ne!(remote, initial);

    // Fast-forwarding catches the branch and its worktree up.
    // The second call has nothing left to move.
    assert!(fast_forward_branches(&mirror).expect("ff"));
    assert_eq!(
        git_in(&mirror, &["rev-parse", "HEAD"]).expect("local"),
        remote
    );
    assert!(mirror.join("new.txt").is_file());
    assert!(!fast_forward_branches(&mirror).expect("idle"));

    // A branch with local commits of its own is never touched.
    git_run(&mirror, &["checkout", "-b", "wip"]);
    std::fs::write(mirror.join("wip.txt"), b"wip\n").expect("write");
    commit_all(&gix::open(&mirror).expect("open"), "local wip");
    let wip = git_in(&mirror, &["rev-parse", "HEAD"]).expect("wip");
    assert!(!fast_forward_branches(&mirror).expect("wip skipped"));
    assert_eq!(
        git_in(&mirror, &["rev-parse", "HEAD"]).expect("wip kept"),
        wip
    );
}

#[test]
fn fetch_repo_refs_imports_heads_under_a_prefix() {
    let dir = tempfile::tempdir().expect("tempdir");

    // A bare base server holding the initial commit.
    // Like a grasp server's `{base}/{owner}/{repo-id}.git` layout.
    let base_server = dir.path().join("npub1base").join("base.git");
    std::fs::create_dir_all(base_server.parent().unwrap()).unwrap();
    let init_status = Command::new("git")
        .args(["init", "--bare", "-q"])
        .arg(&base_server)
        .status()
        .expect("spawn git init --bare");
    assert!(init_status.success());

    let (upstream_dir, upstream_repo) = fixture(&[("a.txt", b"one")]);
    commit_all(&upstream_repo, "initial");
    let upstream_path = upstream_dir.path();
    let initial = git_in(upstream_path, &["rev-parse", "HEAD"]).expect("initial");
    push_all(
        upstream_path,
        &format!("file://{}", dir.path().display()),
        "npub1base",
        "base",
    )
    .expect("push");

    // The base mirror is a plain clone of the base server.
    let base_url = format!("file://{}", base_server.display());
    let mirror = dir.path().join("mirror");
    git_run(
        dir.path(),
        &["clone", "-q", &base_url, mirror.to_str().unwrap()],
    );

    // The fork server has the same initial commit.
    // It also carries a feature commit on its own `feature` branch.
    let fork_work = dir.path().join("fork-work");
    git_run(
        dir.path(),
        &["clone", "-q", &base_url, fork_work.to_str().unwrap()],
    );
    git_run(&fork_work, &["checkout", "-b", "feature"]);
    std::fs::write(fork_work.join("feature.txt"), "feature\n").expect("write");
    commit_all(&gix::open(&fork_work).expect("open"), "feature commit");
    let tip = git_in(&fork_work, &["rev-parse", "HEAD"]).expect("tip");

    let fork_server = dir.path().join("npub1fork").join("fork.git");
    std::fs::create_dir_all(fork_server.parent().unwrap()).unwrap();
    let init_status = Command::new("git")
        .args(["init", "--bare", "-q"])
        .arg(&fork_server)
        .status()
        .expect("spawn git init --bare");
    assert!(init_status.success());
    push_commit_ref(
        &fork_work,
        &format!("file://{}", fork_server.display()),
        &tip,
        "refs/heads/feature",
    )
    .expect("push");

    // Import the fork's heads into the mirror under a private prefix.
    // The first dead URL is skipped, the second works.
    let dead = format!("file://{}/missing.git", dir.path().display());
    fetch_repo_refs(
        &mirror,
        &[dead, format!("file://{}", fork_server.display())],
        "+refs/heads/*:refs/fork/npub1fork/fork/*",
    )
    .expect("fetch");

    // The imported refs are listed under the prefix only.
    assert_eq!(
        refs_with_prefix(&mirror, "refs/fork/npub1fork/fork").expect("refs"),
        vec!["refs/fork/npub1fork/fork/feature"]
    );
    // Nothing leaked into the normal ref namespaces.
    assert_eq!(
        refs_with_prefix(&mirror, "refs/heads/fork").expect("refs"),
        Vec::<String>::new()
    );

    // The mirror can now range across both histories.
    // The fork point is the shared initial commit, the proposal covers the fork commit.
    assert_eq!(
        merge_base(
            &mirror,
            "refs/remotes/origin/main",
            "refs/fork/npub1fork/fork/feature",
        )
        .expect("merge base")
        .as_deref(),
        Some(initial.as_str())
    );
    let patch =
        format_patch_between(&mirror, &initial, "refs/fork/npub1fork/fork/feature").expect("patch");
    assert!(patch.contains("Subject: [PATCH] feature commit"));
    assert!(patch.contains("feature.txt"));

    // Pruning the prefix removes the import again.
    delete_refs_with_prefix(&mirror, "refs/fork/npub1fork/fork").expect("delete");
    assert_eq!(
        refs_with_prefix(&mirror, "refs/fork/npub1fork/fork").expect("refs"),
        Vec::<String>::new()
    );
}

#[test]
fn fetch_repo_refs_fails_when_every_url_fails() {
    let (_dir, repo) = fixture(&[("a.txt", b"one")]);
    commit_all(&repo, "initial");
    let dir = _dir.path();

    let dead = format!("file://{}/missing.git", dir.display());
    let err =
        fetch_repo_refs(dir, &[dead], "+refs/heads/*:refs/fork/x/*").expect_err("all URLs fail");
    assert!(err.to_string().contains("failed to fetch"));

    // Without any URL there is nothing to try.
    let err =
        fetch_repo_refs(dir, &[] as &[String], "+refs/heads/*:refs/fork/x/*").expect_err("no URLs");
    assert!(err.to_string().contains("no clone URLs"));
}

#[test]
fn delete_refs_with_prefix_is_a_noop_without_matches() {
    let (_dir, repo) = fixture(&[("a.txt", b"one")]);
    commit_all(&repo, "initial");
    delete_refs_with_prefix(_dir.path(), "refs/fork/nothing").expect("noop");
}

/// Run a git command in `dir`, asserting success.
fn git_run(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "Test Author")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test Author")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .env("GIT_EDITOR", "true")
        .args(args)
        .status()
        .expect("spawn git");
    assert!(status.success(), "git {args:?} failed");
}

#[test]
fn last_commit_returns_most_recent_change() {
    let (dir, repo) = fixture(&[("a.txt", b"one")]);
    commit_all(&repo, "initial");

    std::fs::write(dir.path().join("a.txt"), b"two").expect("write");
    commit_all(&repo, "change a");

    // A commit touching another file must not be reported for a.txt.
    std::fs::write(dir.path().join("b.txt"), b"other").expect("write");
    commit_all(&repo, "add b");

    let commit = last_commit(&repo, Path::new("a.txt"))
        .expect("lookup")
        .expect("found");
    assert_eq!(commit.summary, "change a");
    assert_eq!(commit.author, "Test Author");
    assert!(!commit.id.is_empty());
    assert!(commit.time > 0);
}

#[test]
fn all_commits_lists_every_commit() {
    let (dir, repo) = fixture(&[("a.txt", b"one")]);
    commit_all(&repo, "initial");

    std::fs::write(dir.path().join("a.txt"), b"two").expect("write");
    commit_all(&repo, "second");
    std::fs::write(dir.path().join("b.txt"), b"b").expect("write");
    commit_all(&repo, "third");

    let list = all_commits(&repo).expect("commits");
    assert_eq!(list.total, 3);
    let mut summaries: Vec<&str> = list.commits.iter().map(|c| c.summary.as_str()).collect();
    summaries.sort();
    assert_eq!(summaries, vec!["initial", "second", "third"]);
    assert!(
        list.commits
            .iter()
            .all(|c| c.author == "Test Author" && !c.id.is_empty() && c.time > 0)
    );
}

#[test]
fn all_commits_returns_empty_without_head() {
    let (_dir, repo) = fixture(&[("a.txt", b"one")]);

    let list = all_commits(&repo).expect("commits");
    assert!(list.commits.is_empty());
    assert_eq!(list.total, 0);
}

#[test]
fn last_commit_returns_none_for_untracked_files() {
    let (dir, repo) = fixture(&[("a.txt", b"one")]);
    commit_all(&repo, "initial");
    std::fs::write(dir.path().join("untracked.txt"), b"x").expect("write");

    let commit = last_commit(&repo, Path::new("untracked.txt")).expect("lookup");
    assert!(commit.is_none());
}

#[test]
fn last_commit_reports_merge_commits() {
    let (dir, repo) = fixture(&[("a.txt", b"base")]);
    commit_all(&repo, "initial");

    let run = |args: &[&str]| {
        let status = Command::new("git")
            .current_dir(dir.path())
            .env("GIT_AUTHOR_NAME", "Test Author")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test Author")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .env("GIT_EDITOR", "true")
            .args(args)
            .status()
            .expect("spawn git");
        assert!(status.success(), "git {args:?} failed");
    };
    run(&["checkout", "-b", "feature"]);
    std::fs::write(dir.path().join("a.txt"), b"feature").expect("write");
    commit_all(&repo, "feature change");
    run(&["checkout", "-"]);
    // `--no-ff` forces a merge commit, it is the latest commit changing a.txt.
    run(&["merge", "--no-ff", "--no-edit", "feature"]);

    let commit = last_commit(&repo, Path::new("a.txt"))
        .expect("lookup")
        .expect("found");
    assert_eq!(
        commit.id,
        repo.head_id().expect("head").shorten_or_id().to_string()
    );
    assert!(commit.summary.starts_with("Merge branch"));
}

#[test]
fn last_commits_batches_multiple_paths() {
    let (dir, repo) = fixture(&[("a.txt", b"one"), ("b.txt", b"b")]);
    commit_all(&repo, "initial");

    std::fs::write(dir.path().join("a.txt"), b"two").expect("write");
    commit_all(&repo, "change a");
    std::fs::write(dir.path().join("b.txt"), b"bb").expect("write");
    commit_all(&repo, "change b");

    let found = worktree_last_commits(
        dir.path(),
        &[
            PathBuf::from("a.txt"),
            PathBuf::from("b.txt"),
            // Untracked paths are absent from the result.
            PathBuf::from("missing.txt"),
        ],
    )
    .expect("commits");
    let by_path: HashMap<&Path, &FileCommit> = found
        .iter()
        .map(|(path, commit)| (path.as_path(), commit))
        .collect();
    assert_eq!(by_path.len(), 2);
    assert_eq!(by_path[Path::new("a.txt")].summary, "change a");
    assert_eq!(by_path[Path::new("b.txt")].summary, "change b");
}

#[test]
fn find_readme_prefers_markdown() {
    let (_dir, repo) = fixture(&[("readme.txt", b"txt"), ("README.md", b"md")]);

    let readme = find_readme(&repo).expect("find");
    assert_eq!(
        readme.map(|p| p.to_string_lossy().into_owned()),
        Some("README.md".into())
    );
}

#[test]
fn find_readme_falls_back_to_any_readme() {
    let (_dir, repo) = fixture(&[("README.rst", b"rst")]);

    let readme = find_readme(&repo).expect("find");
    assert_eq!(
        readme.map(|p| p.to_string_lossy().into_owned()),
        Some("README.rst".into())
    );
}

#[test]
fn find_readme_returns_none_without_one() {
    let (_dir, repo) = fixture(&[("main.rs", b"")]);
    assert!(find_readme(&repo).expect("find").is_none());
}

#[test]
fn head_commit_reports_head() {
    let (_dir, repo) = fixture(&[("a.txt", b"one")]);

    // Unborn HEAD means no commit yet.
    assert!(head_commit(&repo).expect("head").is_none());

    commit_all(&repo, "initial");
    let head = head_commit(&repo).expect("head").expect("commit");
    assert_eq!(
        head.id,
        repo.head_id().expect("head id").shorten_or_id().to_string()
    );
    assert_eq!(head.summary, "initial");
    assert_eq!(head.author, "Test Author");
}

#[test]
fn worktree_branches_and_tags_list_short_names() {
    let (dir, repo) = fixture(&[("a.txt", b"one")]);
    commit_all(&repo, "initial");
    let dir = dir.path();

    git_run(dir, &["checkout", "-b", "feature"]);
    git_run(dir, &["tag", "v0.9"]);
    git_run(dir, &["tag", "v1.0"]);

    // The initial branch name depends on git configuration.
    // Only the branch we created is fixed.
    let branches = worktree_branches(dir).expect("branches");
    assert_eq!(branches.len(), 2);
    assert!(branches.contains(&"feature".to_string()));
    assert!(branches.windows(2).all(|pair| pair[0] <= pair[1]), "sorted");

    assert_eq!(
        repo_tags(&repo).expect("tags"),
        vec!["v0.9".to_string(), "v1.0".to_string()]
    );
}

#[test]
fn current_branch_tracks_checkout() {
    let (dir, repo) = fixture(&[("a.txt", b"one")]);
    commit_all(&repo, "initial");
    let dir = dir.path();

    let default = worktree_branches(dir)
        .expect("branches")
        .into_iter()
        .next()
        .expect("default branch");
    assert_eq!(
        current_branch(&repo).expect("branch").as_deref(),
        Some(default.as_str())
    );

    git_run(dir, &["checkout", "-b", "feature"]);
    assert_eq!(
        current_branch(&repo).expect("branch").as_deref(),
        Some("feature")
    );

    // Tags detach HEAD.
    git_run(dir, &["tag", "v1.0"]);
    worktree_checkout_tag(dir, "v1.0").expect("checkout tag");
    assert_eq!(current_branch(&repo).expect("branch"), None);

    // Branches re-attach HEAD.
    worktree_checkout_branch(dir, &default).expect("checkout branch");
    assert_eq!(
        current_branch(&repo).expect("branch").as_deref(),
        Some(default.as_str())
    );
}

#[test]
fn worktree_snapshot_reflects_checked_out_ref() {
    let (dir, repo) = fixture(&[("README.md", b"# main"), ("a.txt", b"one")]);
    commit_all(&repo, "initial");
    let dir = dir.path();

    git_run(dir, &["checkout", "-b", "feature"]);
    std::fs::write(dir.join("README.md"), b"# feature").expect("write");
    std::fs::write(dir.join("b.txt"), b"b").expect("write");
    commit_all(&repo, "feature work");

    let snapshot = worktree_snapshot(dir).expect("snapshot");
    assert_eq!(snapshot.current_branch.as_deref(), Some("feature"));
    assert_eq!(
        snapshot.head_commit.as_ref().expect("head commit").summary,
        "feature work"
    );
    assert_eq!(
        String::from_utf8(snapshot.readme.expect("readme")).expect("utf8"),
        "# feature"
    );
    let entries: Vec<String> = snapshot
        .entries
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    assert!(entries.contains(&"b.txt".to_string()));

    let default = worktree_branches(dir)
        .expect("branches")
        .into_iter()
        .find(|name| name != "feature")
        .expect("default branch");
    worktree_checkout_branch(dir, &default).expect("checkout");

    let snapshot = worktree_snapshot(dir).expect("snapshot");
    assert_eq!(snapshot.current_branch.as_deref(), Some(default.as_str()));
    assert_eq!(
        snapshot.head_commit.as_ref().expect("head commit").summary,
        "initial"
    );
    assert_eq!(
        String::from_utf8(snapshot.readme.expect("readme")).expect("utf8"),
        "# main"
    );
    assert!(
        !snapshot
            .entries
            .iter()
            .any(|p| p.to_string_lossy() == "b.txt")
    );
}

#[test]
fn commit_diff_lists_added_modified_and_deleted_files() {
    let (dir, repo) = fixture(&[("keep.txt", b"keep"), ("mod.txt", b"one\ntwo\nthree\n")]);
    commit_all(&repo, "initial");

    std::fs::write(dir.path().join("mod.txt"), b"one\ntwo!\nthree\n").expect("write");
    std::fs::write(dir.path().join("new.txt"), b"hello\n").expect("write");
    std::fs::remove_file(dir.path().join("keep.txt")).expect("remove");
    commit_all(&repo, "changes");

    let head = repo.head_id().expect("head").shorten_or_id().to_string();
    let diff = worktree_commit_diff(dir.path(), &head).expect("diff");

    let by_path: HashMap<&str, &FileDiff> = diff
        .files
        .iter()
        .map(|file| (file.path.as_str(), file))
        .collect();
    assert_eq!(by_path.len(), 3);

    let added = by_path["new.txt"];
    assert_eq!(added.status, DiffStatus::Added);
    assert_eq!(added.insertions, 1);
    assert_eq!(added.deletions, 0);
    assert_eq!(added.hunks.len(), 1);
    assert_eq!(added.hunks[0].lines.len(), 1);
    assert_eq!(added.hunks[0].lines[0].kind, DiffLineKind::Addition);
    assert_eq!(added.hunks[0].lines[0].old, None);
    assert_eq!(added.hunks[0].lines[0].new, Some(1));
    assert_eq!(added.hunks[0].lines[0].text, "hello");

    let modified = by_path["mod.txt"];
    assert_eq!(modified.status, DiffStatus::Modified);
    assert_eq!(modified.insertions, 1);
    assert_eq!(modified.deletions, 1);
    assert!(!modified.binary);
    let lines = &modified.hunks[0].lines;
    // One hunk with context around the single-line change.
    // The removed line is old 2, the added line is new 2.
    assert!(lines.iter().any(|line| {
        line.kind == DiffLineKind::Deletion
            && line.old == Some(2)
            && line.new.is_none()
            && line.text == "two"
    }));
    assert!(lines.iter().any(|line| {
        line.kind == DiffLineKind::Addition
            && line.old.is_none()
            && line.new == Some(2)
            && line.text == "two!"
    }));
    assert!(lines.iter().any(|line| {
        line.kind == DiffLineKind::Context && line.old == Some(1) && line.new == Some(1)
    }));

    let deleted = by_path["keep.txt"];
    assert_eq!(deleted.status, DiffStatus::Deleted);
    assert_eq!(deleted.deletions, 1);
    assert_eq!(deleted.hunks[0].lines[0].kind, DiffLineKind::Deletion);
    assert_eq!(deleted.hunks[0].lines[0].old, Some(1));
    assert_eq!(deleted.hunks[0].lines[0].new, None);
}

#[test]
fn commit_range_diff_lists_changes_between_two_commits() {
    let (dir, repo) = fixture(&[("a.txt", b"a\n"), ("b.txt", b"b\n")]);
    commit_all(&repo, "first");
    let base = repo.head_id().expect("head").to_string();

    std::fs::write(dir.path().join("a.txt"), b"changed\n").expect("write");
    std::fs::write(dir.path().join("c.txt"), b"new\n").expect("write");
    commit_all(&repo, "second");
    let tip = repo.head_id().expect("head").to_string();

    let diff = worktree_commit_range_diff(dir.path(), &base, &tip).expect("diff");

    let by_path: HashMap<&str, &FileDiff> = diff
        .files
        .iter()
        .map(|file| (file.path.as_str(), file))
        .collect();
    assert_eq!(by_path.len(), 2);
    assert_eq!(by_path["a.txt"].status, DiffStatus::Modified);
    assert_eq!(by_path["a.txt"].insertions, 1);
    assert_eq!(by_path["a.txt"].deletions, 1);
    assert_eq!(by_path["c.txt"].status, DiffStatus::Added);
    // b.txt is unchanged between the two commits.
    assert!(diff.files.iter().all(|file| file.path != "b.txt"));
}

#[test]
fn commit_range_commits_lists_only_new_commits_newest_first() {
    let (dir, repo) = fixture(&[("a.txt", b"one\n")]);
    commit_all(&repo, "one");
    let base = repo.head_id().expect("head").to_string();

    std::fs::write(dir.path().join("a.txt"), b"two\n").expect("write");
    commit_all(&repo, "two");
    std::fs::write(dir.path().join("a.txt"), b"three\n").expect("write");
    commit_all(&repo, "three");
    let tip = repo.head_id().expect("head").to_string();

    let commits = worktree_commit_range_commits(dir.path(), &base, &tip).expect("commits");

    assert_eq!(commits.len(), 2);
    assert_eq!(commits[0].summary, "three");
    assert_eq!(commits[1].summary, "two");
}

#[test]
fn commit_diff_reports_binary_files_without_hunks() {
    let (_dir, repo) = fixture(&[("blob.bin", b"\x00\x01\x02")]);
    commit_all(&repo, "initial");

    std::fs::write(_dir.path().join("blob.bin"), b"\x00\x03").expect("write");
    commit_all(&repo, "binary change");

    let head = repo.head_id().expect("head").shorten_or_id().to_string();
    let diff = worktree_commit_diff(_dir.path(), &head).expect("diff");
    let file = diff
        .files
        .iter()
        .find(|f| f.path == "blob.bin")
        .expect("file");
    assert!(file.binary);
    assert!(file.hunks.is_empty());
    assert_eq!(file.insertions, 0);
    assert_eq!(file.deletions, 0);
}

#[test]
fn commit_diff_resolves_short_ids_and_root_commit() {
    let (dir, repo) = fixture(&[("a.txt", b"one\n")]);
    commit_all(&repo, "initial");

    // The root commit diffs against the empty tree, everything is added.
    let head = repo.head_id().expect("head").shorten_or_id().to_string();
    let diff = worktree_commit_diff(dir.path(), &head).expect("diff");
    assert_eq!(diff.files.len(), 1);
    assert_eq!(diff.files[0].path, "a.txt");
    assert_eq!(diff.files[0].status, DiffStatus::Added);
    assert_eq!(diff.files[0].insertions, 1);
}

#[test]
fn file_commit_includes_message_body() {
    let (_dir, repo) = fixture(&[("a.txt", b"one")]);
    commit_all(&repo, "title");

    // A single-line message has no body.
    let head = head_commit(&repo).expect("head").expect("commit");
    assert_eq!(head.summary, "title");
    assert_eq!(head.description, None);

    // A message with a body exposes it, trimmed.
    let dir = _dir.path();
    let status = Command::new("git")
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "Test Author")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test Author")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .env("GIT_EDITOR", "true")
        .args([
            "commit",
            "--allow-empty",
            "-m",
            "title two",
            "-m",
            "line one\n\nline two",
        ])
        .status()
        .expect("spawn git");
    assert!(status.success(), "git commit failed");

    let head = head_commit(&repo).expect("head").expect("commit");
    assert_eq!(head.summary, "title two");
    assert_eq!(head.description.as_deref(), Some("line one\n\nline two"));
}

#[test]
fn commit_diff_reports_renames() {
    let (_dir, repo) = fixture(&[("old.txt", b"same content\n")]);
    commit_all(&repo, "initial");

    std::fs::rename(_dir.path().join("old.txt"), _dir.path().join("new.txt")).expect("rename");
    commit_all(&repo, "rename");

    let head = repo.head_id().expect("head").shorten_or_id().to_string();
    let diff = worktree_commit_diff(_dir.path(), &head).expect("diff");
    let file = diff
        .files
        .iter()
        .find(|f| f.path == "new.txt")
        .expect("file");
    assert_eq!(file.status, DiffStatus::Renamed);
    assert_eq!(file.old_path.as_deref(), Some("old.txt"));
    // A pure rename has no content change, the file is still listed.
    assert!(file.hunks.is_empty());
    assert_eq!(file.insertions, 0);
    assert_eq!(file.deletions, 0);
}

#[test]
fn parses_format_patch_output() {
    let patch = r#"From 1f6c0c5f3f1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a Mon Sep 17 00:00:00 2001
From: A <a@b.c>
Subject: [PATCH] fix

fix the thing

---
 src/lib.rs | 2 +-
 1 file changed, 1 insertion(+), 1 deletion(-)

diff --git a/src/lib.rs b/src/lib.rs
index 1234567..89abcde 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,3 +1,3 @@
 fn main() {
-    println!("old");
+    println!("new");
 }
"#;
    let diff = patch_diffs(patch).expect("parse");

    assert_eq!(diff.files.len(), 1);
    let file = &diff.files[0];
    assert_eq!(file.path, "src/lib.rs");
    assert_eq!(file.old_path, None);
    assert_eq!(file.status, DiffStatus::Modified);
    assert_eq!(file.insertions, 1);
    assert_eq!(file.deletions, 1);

    let hunk = &file.hunks[0];
    assert_eq!(hunk.old_start, 1);
    assert_eq!(hunk.old_lines, 3);
    assert_eq!(hunk.new_start, 1);
    assert_eq!(hunk.new_lines, 3);
    assert_eq!(hunk.lines.len(), 4);
    assert_eq!(hunk.lines[0].kind, DiffLineKind::Context);
    assert_eq!(hunk.lines[0].old, Some(1));
    assert_eq!(hunk.lines[0].new, Some(1));
    assert_eq!(hunk.lines[1].kind, DiffLineKind::Deletion);
    assert_eq!(hunk.lines[1].old, Some(2));
    assert_eq!(hunk.lines[1].new, None);
    assert_eq!(hunk.lines[2].kind, DiffLineKind::Addition);
    assert_eq!(hunk.lines[2].old, None);
    assert_eq!(hunk.lines[2].new, Some(2));
    assert_eq!(hunk.lines[3].kind, DiffLineKind::Context);
    assert_eq!(hunk.lines[3].old, Some(3));
    assert_eq!(hunk.lines[3].new, Some(3));
}

#[test]
fn parses_new_file_as_added() {
    let patch = r#"diff --git a/README.md b/README.md
new file mode 100644
index 0000000..1234567
--- /dev/null
+++ b/README.md
@@ -0,0 +1 @@
+# hello
"#;
    let diff = patch_diffs(patch).expect("parse");

    let file = &diff.files[0];
    assert_eq!(file.path, "README.md");
    assert_eq!(file.status, DiffStatus::Added);
    assert_eq!(file.old_path, None);
    assert_eq!(file.insertions, 1);
    assert_eq!(file.deletions, 0);
    assert_eq!(file.hunks[0].old_start, 0);
    assert_eq!(file.hunks[0].old_lines, 0);
    assert_eq!(file.hunks[0].new_start, 1);
}

#[test]
fn parses_renames_with_old_path() {
    let patch = r#"diff --git a/old.rs b/new.rs
similarity index 85%
rename from old.rs
rename to new.rs
index 123..456 100644
--- a/old.rs
+++ b/new.rs
@@ -1 +1 @@
-fn main() {}
+fn main() { println!("hi"); }
"#;
    let diff = patch_diffs(patch).expect("parse");

    let file = &diff.files[0];
    assert_eq!(file.path, "new.rs");
    assert_eq!(file.old_path.as_deref(), Some("old.rs"));
    assert_eq!(file.status, DiffStatus::Renamed);
    assert_eq!(file.insertions, 1);
    assert_eq!(file.deletions, 1);
}

#[test]
fn parses_patch_series_and_skips_envelope() {
    let patch = r#"From aaaa Mon Sep 17 00:00:00 2001
From: A <a@b.c>
Subject: [PATCH 1/2] one

---
 a.txt | 1 +
 1 file changed, 1 insertion(+)

diff --git a/a.txt b/a.txt
index 1..2 100644
--- a/a.txt
+++ b/a.txt
@@ -1 +1,2 @@
 a
+b

From bbbb Mon Sep 17 00:00:00 2001
From: A <a@b.c>
Subject: [PATCH 2/2] two

diff --git a/b.txt b/b.txt
index 3..4 100644
--- a/b.txt
+++ b/b.txt
@@ -1 +1 @@
-x
+y
"#;
    let diff = patch_diffs(patch).expect("parse");

    assert_eq!(diff.files.len(), 2);
    assert_eq!(diff.files[0].path, "a.txt");
    assert_eq!(diff.files[0].insertions, 1);
    assert_eq!(diff.files[1].path, "b.txt");
    assert_eq!(diff.files[1].deletions, 1);
}

#[test]
fn patch_commits_lists_every_patch_in_order() {
    let patch = r#"From 1111111111111111111111111111111111111111 Mon Sep 17 00:00:00 2001
From: Alice <alice@example.com>
Date: Tue, 1 Aug 2023 10:00:00 +0200
Subject: [PATCH 1/2] first

body one
---
 a.txt | 1 +
 1 file changed, 1 insertion(+)

diff --git a/a.txt b/a.txt
@@ -1 +1,2 @@
 a
+b

From 2222222222222222222222222222222222222222 Mon Sep 17 00:00:00 2001
From: Bob <bob@example.com>
Date: Wed, 2 Aug 2023 11:30:00 +0000
Subject: [PATCH 2/2] second

body two
---
 b.txt | 1 +
 1 file changed, 1 insertion(+)

diff --git a/b.txt b/b.txt
@@ -1 +1,2 @@
 x
+y
"#;

    let commits = patch_commits(patch);
    assert_eq!(commits.len(), 2);

    assert_eq!(commits[0].id, "1111111111111111111111111111111111111111");
    assert_eq!(commits[0].summary, "first");
    assert_eq!(commits[0].author, "Alice");
    assert_eq!(commits[0].time, 1690876800);

    assert_eq!(commits[1].id, "2222222222222222222222222222222222222222");
    assert_eq!(commits[1].summary, "second");
    assert_eq!(commits[1].author, "Bob");
    assert_eq!(commits[1].time, 1690975800);
}

#[test]
fn patch_commits_strips_patch_subject_prefixes() {
    let patch = r#"From 1111111111111111111111111111111111111111 Mon Sep 17 00:00:00 2001
From: A <a@b.c>
Subject: [RFC PATCH v3 4/7] the real title

---
"#;

    let commits = patch_commits(patch);
    assert_eq!(commits.len(), 1);
    assert_eq!(commits[0].summary, "the real title");
}

#[test]
fn patch_commits_handles_missing_headers() {
    // A hand-written patch without author or date headers still lists a commit.
    // Time stays 0 and the author stays empty.
    let patch = r#"From 1111111111111111111111111111111111111111 Mon Sep 17 00:00:00 2001
Subject: [PATCH] plain

---
"#;

    let commits = patch_commits(patch);
    assert_eq!(commits.len(), 1);
    assert_eq!(commits[0].summary, "plain");
    assert_eq!(commits[0].author, "");
    assert_eq!(commits[0].time, 0);
}

#[test]
fn patch_commits_ignores_non_patch_lines() {
    assert!(patch_commits("").is_empty());
    assert!(patch_commits("just some text\nFrom 123\n").is_empty());
    // A diff-only body without an mbox envelope has no commits.
    let patch = "diff --git a/x b/x\n--- a/x\n+++ b/x\n";
    assert!(patch_commits(patch).is_empty());
}

#[test]
fn marks_binary_sections() {
    let patch = r#"diff --git a/img.png b/img.png
index 123..456 100644
Binary files a/img.png and b/img.png differ
"#;
    let diff = patch_diffs(patch).expect("parse");

    assert!(diff.files[0].binary);
    assert!(diff.files[0].hunks.is_empty());
}

#[test]
fn unquotes_quoted_paths() {
    let patch = r#"diff --git "a/weird file.rs" "b/weird file.rs"
index 123..456 100644
--- "a/weird file.rs"
+++ "b/weird file.rs"
@@ -1 +1 @@
-x
+y
"#;
    let diff = patch_diffs(patch).expect("parse");

    assert_eq!(diff.files[0].path, "weird file.rs");
    assert_eq!(diff.files[0].status, DiffStatus::Modified);
}

#[test]
fn unquotes_non_ascii_quoted_paths() {
    let patch = r#"diff --git "a/说明.md" "b/说明.md"
index 123..456 100644
--- "a/说明.md"
+++ "b/说明.md"
@@ -1 +1 @@
-x
+y
"#;
    let diff = patch_diffs(patch).expect("parse");

    assert_eq!(diff.files[0].path, "说明.md");
    assert_eq!(diff.files[0].status, DiffStatus::Modified);
}

#[test]
fn unquotes_octal_escaped_paths() {
    let patch = r#"diff --git "a/\345\270\226.md" "b/\345\270\226.md"
index 123..456 100644
--- "a/\345\270\226.md"
+++ "b/\345\270\226.md"
@@ -1 +1 @@
-x
+y
"#;
    let diff = patch_diffs(patch).expect("parse");

    assert_eq!(diff.files[0].path, "帖.md");
    assert_eq!(diff.files[0].status, DiffStatus::Modified);
}

#[test]
fn empty_or_unparseable_patch_yields_no_files() {
    assert_eq!(patch_diffs("").expect("parse").files.len(), 0);
    assert_eq!(patch_diffs("just some text").expect("parse").files.len(), 0);
    assert_eq!(
        patch_diffs("---\nnot a patch\n")
            .expect("parse")
            .files
            .len(),
        0
    );
}

#[test]
fn parses_real_format_patch_output() {
    // Build a commit touching a mix of file kinds.
    // Feed genuine `git format-patch` output through the parser.
    // It covers quoted and octal-escaped paths.
    // There are also a rename-free modification, an addition and a binary deletion.
    let (dir, repo) = fixture(&[
        ("src/main.rs", b"fn main() {\n    println!(\"one\");\n}\n"),
        ("my file.txt", b"hello\n"),
        ("\u{8bf4}\u{660e}.md", "# \u{8bf4}\u{660e}\n".as_bytes()),
        ("img.png", b"\x89PNG\r\n\x1a\n\x00binary"),
    ]);
    commit_all(&repo, "initial");

    std::fs::write(
        dir.path().join("src/main.rs"),
        b"fn main() {\n    println!(\"two\");\n    println!(\"three\");\n}\n",
    )
    .expect("write");
    std::fs::write(dir.path().join("my file.txt"), b"hello world\n").expect("write");
    std::fs::write(
        dir.path().join("\u{8bf4}\u{660e}.md"),
        "# \u{8bf4}\u{660e}\nupdated\n",
    )
    .expect("write");
    std::fs::remove_file(dir.path().join("img.png")).expect("remove");
    std::fs::write(dir.path().join("new file.md"), b"# new\n").expect("write");
    commit_all(&repo, "changes");

    let output = Command::new("git")
        .current_dir(dir.path())
        .env("GIT_AUTHOR_NAME", "Test Author")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test Author")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .args(["format-patch", "-1", "--stdout"])
        .output()
        .expect("spawn git format-patch");
    assert!(output.status.success(), "git format-patch failed");
    let patch = String::from_utf8(output.stdout).expect("patch is utf-8");

    let diff = patch_diffs(&patch).expect("parse real format-patch output");

    let by_path = |path: &str| {
        diff.files
            .iter()
            .find(|file| file.path == path)
            .unwrap_or_else(|| panic!("missing file {path:?}"))
    };

    // Space in the name makes git quote the path in the header.
    let file = by_path("my file.txt");
    assert_eq!(file.status, DiffStatus::Modified);
    assert_eq!(file.insertions, 1);

    // UTF-8 names are emitted as octal escapes.
    let file = by_path("\u{8bf4}\u{660e}.md");
    assert_eq!(file.status, DiffStatus::Modified);
    assert_eq!(file.insertions, 1);

    let file = by_path("src/main.rs");
    assert_eq!(file.status, DiffStatus::Modified);
    assert_eq!(file.insertions, 2);
    assert_eq!(file.deletions, 1);
    assert!(!file.hunks.is_empty());

    let file = by_path("new file.md");
    assert_eq!(file.status, DiffStatus::Added);
    assert_eq!(file.insertions, 1);

    // A binary deletion emits no `---` or `+++` lines.
    // Only the mode line and the `Binary files` marker remain.
    let file = by_path("img.png");
    assert_eq!(file.status, DiffStatus::Deleted);
    assert!(file.binary);
    assert!(file.hunks.is_empty());
}

#[test]
fn worktree_dirty_tracks_changes_and_untracked_files() {
    let (dir, repo) = fixture(&[("tracked.txt", b"one")]);
    commit_all(&repo, "initial");
    let workdir = dir.path();

    assert!(!worktree_dirty(workdir));

    // A modified tracked file is dirty.
    std::fs::write(workdir.join("tracked.txt"), b"two").expect("write");
    assert!(worktree_dirty(workdir));

    // After restoring, an untracked file alone is dirty as well.
    git_run(workdir, &["checkout", "--", "tracked.txt"]);
    assert!(!worktree_dirty(workdir));
    std::fs::write(workdir.join("untracked.txt"), b"new").expect("write");
    assert!(worktree_dirty(workdir));

    // A staged change counts too.
    git_run(workdir, &["rm", "--cached", "tracked.txt"]);
    assert!(worktree_dirty(workdir));

    // A missing directory is clean, not an error.
    assert!(!worktree_dirty(&dir.path().join("missing")));
}

#[test]
fn worktree_dirty_reports_unborn_worktrees_with_files() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("repo");
    let status = Command::new("git")
        .args(["init", "-q"])
        .arg(&path)
        .status()
        .expect("spawn git init");
    assert!(status.success());

    // No commits and no files: porcelain is empty.
    assert!(!worktree_dirty(&path));
    // An unborn repository holding files is dirty.
    std::fs::write(path.join("README.md"), "# hello\n").expect("write");
    assert!(worktree_dirty(&path));
}

#[test]
fn worktree_commits_ahead_counts_branch_only_commits() {
    let (dir, repo) = fixture(&[("a.txt", b"one")]);
    commit_all(&repo, "initial");
    let path = dir.path();

    git_run(path, &["checkout", "-q", "-b", "feature"]);
    std::fs::write(path.join("f.txt"), b"f\n").expect("write");
    commit_all(&gix::open(path).expect("open"), "feature work");

    assert_eq!(worktree_commits_ahead(path, "main", "feature"), 1);
    assert_eq!(worktree_commits_ahead(path, "feature", "main"), 0);

    git_run(path, &["checkout", "-q", "main"]);
    assert_eq!(worktree_current_branch(path).as_deref(), Some("main"));
    assert!(worktree_ref_exists(path, "refs/heads/feature"));
    assert!(!worktree_ref_exists(path, "refs/heads/nope"));
    assert_eq!(worktree_commits_ahead(path, "main", "feature"), 1);
}

use std::path::{Path, PathBuf};
use std::process::Command;

use super::*;

#[test]
fn blocks_parent_components() {
    assert_eq!(GitCache::sanitize_path_component(".."), "_");
    assert_eq!(GitCache::sanitize_path_component("."), "_");
    assert_eq!(GitCache::sanitize_path_component("../.."), ".._..");
    assert_eq!(GitCache::sanitize_path_component("a/../b"), "a_.._b");
}

#[test]
fn root_commit_reports_the_first_ancestor() {
    let (dir, repo) = fixture(&[("a.txt", b"one")]);
    commit_all(&repo, "initial");

    let dir = dir.path();
    let root = repo.root_commit().expect("root").expect("commit");
    assert_eq!(root.len(), 40);

    std::fs::write(dir.join("b.txt"), b"two").expect("write");
    commit_all(&repo, "second");
    assert_eq!(
        repo.root_commit().expect("root").as_deref(),
        Some(root.as_str())
    );
}

#[test]
fn push_all_mirrors_branches_and_tags() {
    let server = tempfile::tempdir().unwrap();
    let server_repo = bare_server(server.path(), "npub1test", "my-repo");

    let (dir, repo) = fixture(&[("a.txt", b"one")]);
    commit_all(&repo, "initial");
    let dir = dir.path();

    git_run(dir, &["checkout", "-b", "feature"]);
    std::fs::write(dir.join("b.txt"), b"two").expect("write");
    commit_all(&repo, "feature work");
    git_run(dir, &["checkout", "-"]);
    git_run(dir, &["tag", "v1.0"]);

    let base_url = format!("file://{}", server.path().display());
    Repo::open(dir)
        .expect("open")
        .push_all(&base_url, "npub1test", "my-repo")
        .expect("push");

    let refs = git_in(&server_repo, &["show-ref"]).expect("server refs");
    assert!(refs.contains("refs/heads/main"));
    assert!(refs.contains("refs/heads/feature"));
    assert!(refs.contains("refs/tags/v1.0"));
}

#[test]
fn remote_has_refs_reports_whether_pushed_refs_landed() {
    let server = tempfile::tempdir().unwrap();
    bare_server(server.path(), "npub1test", "my-repo");
    let (dir, repo) = fixture(&[("a.txt", b"one")]);
    commit_all(&repo, "initial");
    let dir = dir.path();
    let main = git_in(dir, &["rev-parse", "refs/heads/main"]).expect("main oid");
    let url = format!("file://{}/npub1test/my-repo.git", server.path().display());
    let expected = vec![("refs/heads/main".to_owned(), main.clone())];
    let repo = Repo::open(dir).expect("open");

    assert!(!repo.remote_has_refs(&url, &expected).expect("probe"));

    repo.push_all(
        &format!("file://{}", server.path().display()),
        "npub1test",
        "my-repo",
    )
    .expect("push");

    assert!(repo.remote_has_refs(&url, &expected).expect("probe"));

    let stale = vec![("refs/heads/main".to_owned(), "0".repeat(40))];
    assert!(!repo.remote_has_refs(&url, &stale).expect("probe"));

    git_run(dir, &["tag", "v1.0"]);
    repo.push_all(
        &format!("file://{}", server.path().display()),
        "npub1test",
        "my-repo",
    )
    .expect("push");
    assert!(repo.remote_has_refs(&url, &expected).expect("probe"));
}

#[test]
fn repo_ref_state_lists_branches_tags_and_head() {
    let (_dir, repo) = fixture(&[("a.txt", b"hello")]);
    commit_all(&repo, "initial");
    let workdir = repo.workdir().expect("workdir").to_path_buf();

    let state = repo.ref_state().expect("refs");

    let branch = repo.current_branch().expect("on a branch");
    assert_eq!(state.head.as_deref(), Some(branch.as_str()));
    assert_eq!(state.refs.len(), 1);
    assert_eq!(state.refs[0].0, format!("refs/heads/{branch}"));
    assert_eq!(state.refs[0].1.len(), 40);

    git_run(&workdir, &["branch", "feature"]);
    git_run(&workdir, &["tag", "v1.0"]);

    let state = repo.ref_state().expect("refs");
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

    git_run(&workdir, &["checkout", "--detach"]);
    let state = repo.ref_state().expect("refs");
    assert!(state.head.is_none());
    assert_eq!(state.refs.len(), 3);
}

fn fixture(files: &[(&str, &[u8])]) -> (tempfile::TempDir, Repo) {
    let dir = tempfile::tempdir().expect("tempdir");
    gix::init(&dir).expect("init");

    for (rel, bytes) in files {
        let path = dir.path().join(rel);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(&path, bytes).expect("write");
    }

    let repo = Repo::open(dir.path()).expect("open");
    (dir, repo)
}

fn commit_all(repo: &Repo, message: &str) {
    git_run(repo.workdir().expect("workdir"), &["add", "-A"]);
    git_run(repo.workdir().expect("workdir"), &["commit", "-m", message]);
}

#[test]
fn merge_base_finds_the_fork_point_and_reports_unrelated_history() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("repo");
    let initial = Repo::init(&path, "My Repo", "desc").expect("init");
    let repo = Repo::open(&path).expect("open");

    git_run(&path, &["checkout", "-b", "feature"]);
    std::fs::write(path.join("feature.txt"), "feature\n").expect("write");
    commit_all(&repo, "feature commit");
    git_run(&path, &["checkout", "main"]);
    std::fs::write(path.join("main.txt"), "main\n").expect("write");
    commit_all(&repo, "mainline commit");

    assert_eq!(
        repo.merge_base("feature", "main")
            .expect("merge base")
            .as_deref(),
        Some(initial.as_str())
    );

    git_run(&path, &["checkout", "--orphan", "orphan"]);
    std::fs::write(path.join("orphan.txt"), "orphan\n").expect("write");
    commit_all(&repo, "orphan commit");
    assert_eq!(repo.merge_base("orphan", "main").expect("ok"), None);

    assert!(repo.merge_base("orphan", "no-such-ref").is_err());
}

#[test]
fn split_patch_series_splits_real_multi_commit_mboxes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("repo");
    let initial = Repo::init(&path, "My Repo", "desc").expect("init");
    let repo = Repo::open(&path).expect("open");

    git_run(&path, &["checkout", "-b", "feature"]);
    std::fs::write(path.join("one.txt"), "one\n").expect("write");
    commit_all(&repo, "first commit");
    std::fs::write(path.join("two.txt"), "two\n").expect("write");
    commit_all(&repo, "second commit");

    let series = repo
        .format_patch_between(&initial, "feature")
        .expect("series");
    let parts = PatchParser::split_patch_series(&series);

    assert_eq!(parts.len(), 2);
    assert!(parts[0].contains("Subject: [PATCH 1/2] first commit"));
    assert!(parts[1].contains("Subject: [PATCH 2/2] second commit"));
    let first = parts[0].lines().next().expect("first header");
    let second = parts[1].lines().next().expect("second header");
    assert!(first.starts_with("From ") && first.len() >= 45);
    assert_ne!(first, second);
}

#[test]
fn head_commit_and_commits_since_track_applied_commits() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("repo");
    let initial = Repo::init(&path, "My Repo", "desc").expect("init");
    let repo = Repo::open(&path).expect("open");

    assert_eq!(repo.head().as_deref(), Some(initial.as_str()));
    assert_eq!(
        repo.commits_since(None).expect("commits"),
        vec![initial.clone()]
    );

    std::fs::write(path.join("one.txt"), "one\n").expect("write");
    commit_all(&repo, "first commit");
    let first = repo.head().expect("on a branch");

    std::fs::write(path.join("two.txt"), "two\n").expect("write");
    commit_all(&repo, "second commit");
    let second = repo.head().expect("on a branch");

    assert_eq!(
        repo.commits_since(Some(&initial)).expect("commits"),
        vec![first.clone(), second.clone()]
    );
    assert_eq!(
        repo.commits_since(Some(&first)).expect("commits"),
        vec![second]
    );
}

#[test]
fn working_copy_cloned_from_the_mirror_matches_head_and_origin() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mirror = dir.path().join("mirror");
    let commit = Repo::init(&mirror, "My Repo", "Does things.").expect("init");
    Repo::open(&mirror)
        .expect("open")
        .ensure_origin("https://gitnostr.com/npub1test/my-repo.git")
        .expect("origin");

    let destination = dir.path().join("folder").join("My_Repo");
    std::fs::create_dir_all(destination.parent().unwrap()).expect("parent");
    Repo::clone(&[format!("file://{}", mirror.display())], &destination).expect("clone");
    Repo::open(&destination)
        .expect("open")
        .set_origin("https://gitnostr.com/npub1test/my-repo.git")
        .expect("set origin");
    let working = Repo::open(&destination).expect("open");

    assert_eq!(
        working.origin_url().expect("url").as_deref(),
        Some("https://gitnostr.com/npub1test/my-repo.git")
    );
    assert_eq!(working.head().as_deref(), Some(commit.as_str()));
    assert!(destination.join("README.md").is_file());
}

#[test]
fn fast_forward_branches_moves_the_mirror_and_keeps_local_work() {
    let dir = tempfile::tempdir().expect("tempdir");
    bare_server(dir.path(), "npub1test", "repo");

    let (work_dir, work_repo) = fixture(&[("a.txt", b"one")]);
    commit_all(&work_repo, "initial");
    let work = work_dir.path();
    let base_url = format!("file://{}", dir.path().display());
    work_repo
        .push_all(&base_url, "npub1test", "repo")
        .expect("push");

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
    let repo = Repo::open(&mirror).expect("open");

    std::fs::write(work.join("new.txt"), b"new\n").expect("write");
    commit_all(&work_repo, "new commit");
    work_repo
        .push_all(&base_url, "npub1test", "repo")
        .expect("push");
    git_run(&mirror, &["fetch", "origin"]);
    let remote = git_in(&mirror, &["rev-parse", "refs/remotes/origin/main"]).expect("remote");
    assert_eq!(
        git_in(&mirror, &["rev-parse", "HEAD"]).expect("local"),
        initial
    );
    assert_ne!(remote, initial);

    assert!(repo.fast_forward_branches().expect("ff"));
    assert_eq!(
        git_in(&mirror, &["rev-parse", "HEAD"]).expect("local"),
        remote
    );
    assert!(mirror.join("new.txt").is_file());
    assert!(!repo.fast_forward_branches().expect("idle"));

    git_run(&mirror, &["checkout", "-b", "wip"]);
    std::fs::write(mirror.join("wip.txt"), b"wip\n").expect("write");
    commit_all(&repo, "local wip");
    let wip = git_in(&mirror, &["rev-parse", "HEAD"]).expect("wip");
    assert!(!repo.fast_forward_branches().expect("wip skipped"));
    assert_eq!(
        git_in(&mirror, &["rev-parse", "HEAD"]).expect("wip kept"),
        wip
    );
}

#[test]
fn fetch_repo_refs_imports_heads_under_a_prefix() {
    let dir = tempfile::tempdir().expect("tempdir");

    let base_server = bare_server(dir.path(), "npub1base", "base");

    let (upstream_dir, upstream_repo) = fixture(&[("a.txt", b"one")]);
    commit_all(&upstream_repo, "initial");
    let upstream_path = upstream_dir.path();
    let initial = git_in(upstream_path, &["rev-parse", "HEAD"]).expect("initial");
    upstream_repo
        .push_all(
            &format!("file://{}", dir.path().display()),
            "npub1base",
            "base",
        )
        .expect("push");

    let base_url = format!("file://{}", base_server.display());
    let mirror = dir.path().join("mirror");
    git_run(
        dir.path(),
        &["clone", "-q", &base_url, mirror.to_str().unwrap()],
    );
    let mirror_repo = Repo::open(&mirror).expect("open");

    let fork_work = dir.path().join("fork-work");
    git_run(
        dir.path(),
        &["clone", "-q", &base_url, fork_work.to_str().unwrap()],
    );
    git_run(&fork_work, &["checkout", "-b", "feature"]);
    std::fs::write(fork_work.join("feature.txt"), "feature\n").expect("write");
    commit_all(&Repo::open(&fork_work).expect("open"), "feature commit");
    let tip = git_in(&fork_work, &["rev-parse", "HEAD"]).expect("tip");

    let fork_server = bare_server(dir.path(), "npub1fork", "fork");
    Repo::open(&fork_work)
        .expect("open")
        .push_ref(
            &format!("file://{}", fork_server.display()),
            &tip,
            "refs/heads/feature",
        )
        .expect("push");

    let dead = format!("file://{}/missing.git", dir.path().display());
    mirror_repo
        .fetch_refs(
            &[dead, format!("file://{}", fork_server.display())],
            "+refs/heads/*:refs/fork/npub1fork/fork/*",
        )
        .expect("fetch");

    assert_eq!(
        mirror_repo
            .refs_with_prefix("refs/fork/npub1fork/fork")
            .expect("refs"),
        vec!["refs/fork/npub1fork/fork/feature"]
    );
    assert_eq!(
        mirror_repo
            .refs_with_prefix("refs/heads/fork")
            .expect("refs"),
        Vec::<String>::new()
    );

    assert_eq!(
        mirror_repo
            .merge_base(
                "refs/remotes/origin/main",
                "refs/fork/npub1fork/fork/feature",
            )
            .expect("merge base")
            .as_deref(),
        Some(initial.as_str())
    );
    let patch = mirror_repo
        .format_patch_between(&initial, "refs/fork/npub1fork/fork/feature")
        .expect("patch");
    assert!(patch.contains("Subject: [PATCH] feature commit"));
    assert!(patch.contains("feature.txt"));

    mirror_repo
        .delete_refs_with_prefix("refs/fork/npub1fork/fork")
        .expect("delete");
    assert_eq!(
        mirror_repo
            .refs_with_prefix("refs/fork/npub1fork/fork")
            .expect("refs"),
        Vec::<String>::new()
    );
}

fn git_run(dir: &Path, args: &[&str]) -> std::process::Output {
    let output = Command::new("git")
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "Test Author")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test Author")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .env("GIT_EDITOR", "true")
        .args(args)
        .output()
        .expect("spawn git");
    assert!(output.status.success(), "git {args:?} failed");
    output
}

fn bare_server(base: &Path, owner: &str, name: &str) -> PathBuf {
    let repo = base.join(owner).join(format!("{name}.git"));
    std::fs::create_dir_all(repo.parent().expect("parent")).expect("mkdir");
    git_run(
        base,
        &["init", "--bare", "-q", repo.to_str().expect("utf8 path")],
    );
    repo
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

    let commits = PatchParser::patch_commits(patch);
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
fn parses_real_format_patch_output() {
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

    let output = git_run(dir.path(), &["format-patch", "-1", "--stdout"]);
    let patch = String::from_utf8(output.stdout).expect("patch is utf-8");

    let diff = PatchParser::patch_diffs(&patch).expect("parse real format-patch output");

    let by_path = |path: &str| {
        diff.files
            .iter()
            .find(|file| file.path == path)
            .unwrap_or_else(|| panic!("missing file {path:?}"))
    };

    let file = by_path("my file.txt");
    assert_eq!(file.status, DiffStatus::Modified);
    assert_eq!(file.insertions, 1);

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

    let file = by_path("img.png");
    assert_eq!(file.status, DiffStatus::Deleted);
    assert!(file.binary);
    assert!(file.hunks.is_empty());
    assert_eq!(file.insertions, 0);
    assert_eq!(file.deletions, 0);
}

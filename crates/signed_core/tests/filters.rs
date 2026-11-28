mod common;

use common::{keys, kind_tag, signed};
use nostr::prelude::*;
use signed_core::GitEvent;

#[test]
fn comment_activity_depends_on_the_uppercase_k_tag() {
    let on_git = signed(&keys(1), Kind::Comment, vec![kind_tag("K", Kind::GitIssue)]);
    let on_repo = signed(
        &keys(1),
        Kind::Comment,
        vec![kind_tag("K", Kind::GitRepoAnnouncement)],
    );
    let on_note = signed(&keys(1), Kind::Comment, vec![kind_tag("K", Kind::TextNote)]);

    assert!(on_git.is_git_activity());
    assert!(on_repo.is_git_activity());
    assert!(!on_note.is_git_activity());
    assert!(!signed(&keys(1), Kind::Comment, Vec::new()).is_git_activity());
}

#[test]
fn status_activity_depends_on_the_lowercase_k_tag() {
    let status = signed(
        &keys(1),
        Kind::GitStatusClosed,
        vec![kind_tag("k", Kind::GitPullRequest)],
    );
    let unrelated = signed(
        &keys(1),
        Kind::GitStatusClosed,
        vec![kind_tag("k", Kind::Metadata)],
    );

    assert!(status.is_git_activity());
    assert!(!unrelated.is_git_activity());
    assert!(!signed(&keys(1), Kind::GitStatusClosed, Vec::new()).is_git_activity());
}

use nostr::prelude::*;

pub trait GitEvent {
    /// Returns the NIP-34 subject or the first non-empty content line.
    fn activity_subject(&self) -> String;

    /// Returns the current commit id.
    fn current_commit(&self) -> Option<String>;

    /// Returns the merge base commit id.
    fn merge_base(&self) -> Option<String>;

    /// Returns the advertised clone URLs.
    fn clone_urls(&self) -> Option<Vec<Url>>;

    /// Returns the branch name.
    fn branch_name(&self) -> Option<String>;

    /// Returns whether the event is git activity.
    fn is_git_activity(&self) -> bool;

    /// Returns whether an `e` or `E` tag points at the root event.
    fn references_root(&self, root: &EventId) -> bool;
}

impl GitEvent for Event {
    /// Reads the subject tag, then the first content line, then "Untitled".
    fn activity_subject(&self) -> String {
        let subject = self
            .tags
            .iter()
            .find_map(|tag| match Nip34Tag::parse(tag.as_slice()) {
                Ok(Nip34Tag::Subject(subject)) => Some(subject),
                _ => None,
            });

        subject
            .or_else(|| {
                self.content
                    .lines()
                    .map(str::trim)
                    .find(|line| !line.is_empty())
                    .map(|value| value.to_string())
            })
            .unwrap_or("Untitled".to_string())
    }

    /// Reads the `commit` tag.
    fn current_commit(&self) -> Option<String> {
        self.tags
            .iter()
            .find_map(|tag| match Nip34Tag::parse(tag.as_slice()) {
                Ok(Nip34Tag::CurrentCommit(commit)) => Some(commit.to_string()),
                _ => None,
            })
    }

    /// Reads the `merge-base` tag.
    fn merge_base(&self) -> Option<String> {
        self.tags
            .iter()
            .find_map(|tag| match Nip34Tag::parse(tag.as_slice()) {
                Ok(Nip34Tag::MergeBase(commit)) => Some(commit.to_string()),
                _ => None,
            })
    }

    /// Reads the `clone` tag.
    fn clone_urls(&self) -> Option<Vec<Url>> {
        self.tags
            .iter()
            .find_map(|tag| match Nip34Tag::parse(tag.as_slice()) {
                Ok(Nip34Tag::Clone(urls)) => Some(urls),
                _ => None,
            })
    }

    /// Reads the `branch-name` tag.
    fn branch_name(&self) -> Option<String> {
        self.tags
            .iter()
            .find_map(|tag| match Nip34Tag::parse(tag.as_slice()) {
                Ok(Nip34Tag::BranchName(name)) => Some(name),
                _ => None,
            })
    }

    /// Classifies git issue, patch, pull request, comment, and status kinds.
    fn is_git_activity(&self) -> bool {
        match self.kind {
            Kind::GitIssue | Kind::GitPatch | Kind::GitPullRequest => true,
            Kind::Comment => crate::filters::is_git_comment(self),
            Kind::GitStatusOpen
            | Kind::GitStatusApplied
            | Kind::GitStatusClosed
            | Kind::GitStatusDraft => crate::filters::is_git_status(self),
            _ => false,
        }
    }

    /// Compares `e` and `E` tag contents against the root id.
    fn references_root(&self, root: &EventId) -> bool {
        let root = root.to_hex();
        self.tags
            .iter()
            .any(|tag| matches!(tag.kind(), "e" | "E") && tag.content() == Some(root.as_str()))
    }
}

impl<T: GitEvent + ?Sized> GitEvent for &T {
    /// Delegates to the referenced event.
    fn activity_subject(&self) -> String {
        (*self).activity_subject()
    }

    /// Delegates to the referenced event.
    fn current_commit(&self) -> Option<String> {
        (*self).current_commit()
    }

    /// Delegates to the referenced event.
    fn merge_base(&self) -> Option<String> {
        (*self).merge_base()
    }

    /// Delegates to the referenced event.
    fn clone_urls(&self) -> Option<Vec<Url>> {
        (*self).clone_urls()
    }

    /// Delegates to the referenced event.
    fn branch_name(&self) -> Option<String> {
        (*self).branch_name()
    }

    /// Delegates to the referenced event.
    fn is_git_activity(&self) -> bool {
        (*self).is_git_activity()
    }

    /// Delegates to the referenced event.
    fn references_root(&self, root: &EventId) -> bool {
        (*self).references_root(root)
    }
}

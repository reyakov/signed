pub const MAX_LISTED_COMMITS: usize = 20_000;

pub struct CommitList {
    pub total: usize,
    pub commits: Vec<FileCommit>,
}

impl CommitList {
    /// Creates a list holding `total` commits of which `commits` are materialized.
    pub fn new(total: usize, commits: Vec<FileCommit>) -> Self {
        Self { total, commits }
    }
}

#[derive(Debug, Clone)]
pub struct FileCommit {
    pub id: String,
    pub summary: String,
    pub description: Option<String>,
    pub author: String,
    pub time: i64,
}

impl FileCommit {
    /// Creates a commit entry from its id, message parts, author, and timestamp.
    pub fn new(
        id: String,
        summary: String,
        description: Option<String>,
        author: String,
        time: i64,
    ) -> Self {
        Self {
            id,
            summary,
            description,
            author,
            time,
        }
    }

    /// Converts a gix commit including its message body.
    pub(crate) fn from_commit(commit: &gix::Commit<'_>) -> anyhow::Result<Self> {
        Self::from_commit_with_description(commit, true)
    }

    /// Converts a gix commit, skipping the message body since lists never show it.
    pub(crate) fn from_commit_summary(commit: &gix::Commit<'_>) -> anyhow::Result<Self> {
        Self::from_commit_with_description(commit, false)
    }

    /// Converts a gix commit, optionally skipping the message body.
    fn from_commit_with_description(
        commit: &gix::Commit<'_>,
        include_description: bool,
    ) -> anyhow::Result<Self> {
        use crate::GixResultExt as _;

        let author = commit.author().into_anyhow()?;
        let message = commit.message().into_anyhow()?;

        Ok(FileCommit::new(
            commit.id().shorten_or_id().to_string(),
            String::from_utf8_lossy(message.title).trim().to_string(),
            if include_description {
                message
                    .body
                    .map(|body| String::from_utf8_lossy(body).trim().to_string())
                    .filter(|body| !body.is_empty())
            } else {
                None
            },
            String::from_utf8_lossy(author.name).trim().to_string(),
            author.time().into_anyhow()?.seconds,
        ))
    }
}

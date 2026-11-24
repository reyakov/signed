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
    /// Date in the commit's offset, e.g. "Nov 21, 2026 at 4:40 AM GMT+7".
    pub date_label: String,
    /// Full hex parent ids; empty for patch-parsed commits.
    pub parents: Vec<String>,
}

impl FileCommit {
    /// Creates a commit entry from its id, message parts, author, timestamp, date label, and parents.
    pub fn new(
        id: String,
        summary: String,
        description: Option<String>,
        author: String,
        time: i64,
        date_label: String,
        parents: Vec<String>,
    ) -> Self {
        Self {
            id,
            summary,
            description,
            author,
            time,
            date_label,
            parents,
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
        let time = author.time().into_anyhow()?;

        Ok(FileCommit::new(
            commit.id().to_string(),
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
            time.seconds,
            format_commit_date(time),
            commit.parent_ids().map(|id| id.to_string()).collect(),
        ))
    }
}

/// Formats a gix time as "Nov 21, 2026 at 4:40 AM GMT+7" in the commit's own offset.
pub(crate) fn format_commit_date(time: gix::date::Time) -> String {
    let Ok(timestamp) = jiff::Timestamp::from_second(time.seconds) else {
        return String::new();
    };
    let Ok(offset) = jiff::tz::Offset::from_seconds(time.offset) else {
        return String::new();
    };
    let datetime = offset.to_datetime(timestamp);

    let label = datetime.strftime("%b %-d, %Y at %-I:%M %p").to_string();
    format!("{label} {}", gmt_label(time.offset))
}

/// The GMT offset label for a seconds offset, e.g. "GMT", "GMT+7", "GMT-5:30".
fn gmt_label(offset: i32) -> String {
    if offset == 0 {
        return "GMT".to_owned();
    }

    let sign = if offset < 0 { '-' } else { '+' };
    let total_minutes = offset.abs() / 60;
    let hours = total_minutes / 60;
    let minutes = total_minutes % 60;
    if minutes == 0 {
        format!("GMT{sign}{hours}")
    } else {
        format!("GMT{sign}{hours}:{minutes:02}")
    }
}

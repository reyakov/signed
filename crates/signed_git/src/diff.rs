use gix::diff::blob::unified_diff::{ConsumeHunk, DiffLineKind as GixLineKind, HunkHeader};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffLineKind {
    Context,
    Addition,
    Deletion,
}

#[derive(Debug, Clone)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    pub old: Option<u32>,
    pub new: Option<u32>,
    pub text: String,
}

impl DiffLine {
    /// Creates a diff line with its kind, optional line numbers, and text.
    pub fn new(kind: DiffLineKind, old: Option<u32>, new: Option<u32>, text: String) -> Self {
        Self {
            kind,
            old,
            new,
            text,
        }
    }
}

#[derive(Debug, Clone)]
pub struct DiffHunk {
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    pub lines: Vec<DiffLine>,
}

impl DiffHunk {
    /// Creates a hunk spanning the given old and new ranges.
    pub fn new(
        old_start: u32,
        old_lines: u32,
        new_start: u32,
        new_lines: u32,
        lines: Vec<DiffLine>,
    ) -> Self {
        Self {
            old_start,
            old_lines,
            new_start,
            new_lines,
            lines,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
}

#[derive(Debug, Clone)]
pub struct FileDiff {
    pub path: String,
    pub old_path: Option<String>,
    pub status: DiffStatus,
    pub insertions: usize,
    pub deletions: usize,
    pub binary: bool,
    pub hunks: Vec<DiffHunk>,
}

impl FileDiff {
    /// Creates a file diff; `old_path` is the rename or copy source and `binary` files have no hunks.
    pub fn new(
        path: String,
        old_path: Option<String>,
        status: DiffStatus,
        insertions: usize,
        deletions: usize,
        binary: bool,
        hunks: Vec<DiffHunk>,
    ) -> Self {
        Self {
            path,
            old_path,
            status,
            insertions,
            deletions,
            binary,
            hunks,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CommitDiff {
    pub files: Vec<FileDiff>,
}

impl CommitDiff {
    /// Creates a commit diff from its files.
    pub fn new(files: Vec<FileDiff>) -> Self {
        Self { files }
    }
}

pub(crate) struct HunkBuilder {
    old_start: u32,
    old_lines: u32,
    new_start: u32,
    new_lines: u32,
    old: u32,
    new: u32,
    lines: Vec<DiffLine>,
    insertions: usize,
    deletions: usize,
}

impl HunkBuilder {
    /// Starts a hunk spanning the given old and new ranges.
    pub(crate) fn new(old_start: u32, old_lines: u32, new_start: u32, new_lines: u32) -> Self {
        Self {
            old_start,
            old_lines,
            new_start,
            new_lines,
            old: old_start,
            new: new_start,
            lines: Vec::new(),
            insertions: 0,
            deletions: 0,
        }
    }

    /// Appends a line, numbering it and counting insertions and deletions.
    pub(crate) fn push(&mut self, kind: DiffLineKind, text: impl Into<String>) {
        let (old_no, new_no) = match kind {
            DiffLineKind::Context => {
                let numbers = (Some(self.old), Some(self.new));
                self.old += 1;
                self.new += 1;
                numbers
            }
            DiffLineKind::Addition => {
                self.insertions += 1;
                let number = Some(self.new);
                self.new += 1;
                (None, number)
            }
            DiffLineKind::Deletion => {
                self.deletions += 1;
                let number = Some(self.old);
                self.old += 1;
                (number, None)
            }
        };

        self.lines
            .push(DiffLine::new(kind, old_no, new_no, text.into()));
    }

    /// Finishes the hunk and returns it with its insertion and deletion counts.
    pub(crate) fn finish(self) -> (DiffHunk, usize, usize) {
        let hunk = DiffHunk::new(
            self.old_start,
            self.old_lines,
            self.new_start,
            self.new_lines,
            self.lines,
        );
        (hunk, self.insertions, self.deletions)
    }
}

pub(crate) struct HunkCollector<'a> {
    hunks: &'a mut Vec<DiffHunk>,
    insertions: &'a mut usize,
    deletions: &'a mut usize,
}

impl<'a> HunkCollector<'a> {
    /// Collects hunks into `hunks` while counting `insertions` and `deletions`.
    pub(crate) fn new(
        hunks: &'a mut Vec<DiffHunk>,
        insertions: &'a mut usize,
        deletions: &'a mut usize,
    ) -> Self {
        Self {
            hunks,
            insertions,
            deletions,
        }
    }
}

impl ConsumeHunk for HunkCollector<'_> {
    type Out = ();

    /// Consumes one hunk, tracking line numbers and insertion/deletion counts.
    fn consume_hunk(
        &mut self,
        header: HunkHeader,
        lines: &[(GixLineKind, &[u8])],
    ) -> std::io::Result<()> {
        let mut builder = HunkBuilder::new(
            header.before_hunk_start,
            header.before_hunk_len,
            header.after_hunk_start,
            header.after_hunk_len,
        );

        for (kind, content) in lines {
            let kind = match kind {
                GixLineKind::Context => DiffLineKind::Context,
                GixLineKind::Remove => DiffLineKind::Deletion,
                GixLineKind::Add => DiffLineKind::Addition,
            };
            builder.push(kind, String::from_utf8_lossy(content).into_owned());
        }

        let (hunk, insertions, deletions) = builder.finish();
        *self.insertions += insertions;
        *self.deletions += deletions;
        self.hunks.push(hunk);

        Ok(())
    }

    /// Marks the end of the unified diff stream.
    fn finish(self) {}
}

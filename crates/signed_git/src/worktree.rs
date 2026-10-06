use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use gix::progress::Discard;

use crate::GixResultExt as _;
use crate::history::FileCommit;
use crate::repo::Repo;

pub struct WorktreeSnapshot {
    pub entries: Vec<PathBuf>,
    pub readme_path: Option<PathBuf>,
    pub readme: Option<Vec<u8>>,
    // `None` when detached, for example on a tag.
    pub current_branch: Option<String>,
    pub head_commit: Option<FileCommit>,
    pub branches: Vec<String>,
    pub tags: Vec<String>,
}

impl WorktreeSnapshot {
    fn new(
        entries: Vec<PathBuf>,
        readme_path: Option<PathBuf>,
        readme: Option<Vec<u8>>,
        current_branch: Option<String>,
        head_commit: Option<FileCommit>,
        branches: Vec<String>,
        tags: Vec<String>,
    ) -> Self {
        Self {
            entries,
            readme_path,
            readme,
            current_branch,
            head_commit,
            branches,
            tags,
        }
    }
}

impl Repo {
    pub fn is_dirty(&self) -> bool {
        match self.inner.is_dirty() {
            Ok(true) => return true,
            Ok(false) => {}
            Err(_) => return false,
        }

        let Ok(platform) = self.inner.status(Discard) else {
            return false;
        };

        let Ok(mut changes) = platform.into_index_worktree_iter(Vec::<gix::bstr::BString>::new())
        else {
            return false;
        };

        for change in changes.by_ref() {
            match change {
                Ok(gix::status::index_worktree::Item::DirectoryContents { .. }) => return true,
                Ok(_) => {}
                Err(_) => return false,
            }
        }

        false
    }

    pub fn commits_ahead(&self, base: &str, branch: &str) -> u32 {
        let (Some(base), Some(branch)) = (self.resolve_commit(base), self.resolve_commit(branch))
        else {
            return 0;
        };

        let Ok(walk) = self.inner.rev_walk([branch]).with_hidden([base]).all() else {
            return 0;
        };

        walk.filter_map(Result::ok).count().min(u32::MAX as usize) as u32
    }

    fn resolve_commit<'a>(&'a self, rev: &str) -> Option<gix::Id<'a>> {
        self.inner.rev_parse_single(rev.as_bytes()).ok()
    }

    pub fn entries(&self) -> Result<Vec<PathBuf>> {
        let workdir = self.inner.workdir().context("repository has no worktree")?;

        let mut entries: Vec<(PathBuf, bool)> = Vec::new();
        Self::collect_entries(workdir, workdir, &mut entries)?;

        entries.sort_by(|(a, a_is_dir), (b, b_is_dir)| {
            b_is_dir
                .cmp(a_is_dir)
                .then_with(|| a.as_os_str().cmp(b.as_os_str()))
        });
        Ok(entries.into_iter().map(|(path, _)| path).collect())
    }

    pub fn read(&self, rel: &Path) -> Result<Option<Vec<u8>>> {
        let workdir = self.inner.workdir().context("repository has no worktree")?;
        let path = workdir.join(rel);

        match std::fs::read(&path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) if e.kind() == std::io::ErrorKind::IsADirectory => Ok(None),
            Err(e) => Err(e).with_context(|| format!("failed to read {}", path.display())),
        }
    }

    pub fn find_readme(&self) -> Result<Option<PathBuf>> {
        let Some(workdir) = self.inner.workdir() else {
            return Ok(None);
        };

        let mut candidates: Vec<PathBuf> = Vec::new();
        for entry in std::fs::read_dir(workdir)? {
            let entry = entry?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if name.to_ascii_lowercase().starts_with("readme") {
                candidates.push(entry.path());
            }
        }

        candidates.sort_by_key(|path| {
            let ext = path
                .extension()
                .map(|e| e.to_string_lossy().to_ascii_lowercase());
            match ext.as_deref() {
                Some("md") => 0,
                Some("markdown") => 1,
                Some("mdown") => 2,
                Some("mkdn") => 3,
                Some(_) => 5,
                None => 4,
            }
        });

        Ok(candidates
            .into_iter()
            .next()
            .and_then(|path| path.strip_prefix(workdir).ok().map(Path::to_path_buf)))
    }

    pub fn snapshot(&self) -> Result<WorktreeSnapshot> {
        let readme_path = self.find_readme()?;
        let readme = match &readme_path {
            Some(path) => self.read(path)?,
            None => None,
        };
        Ok(WorktreeSnapshot::new(
            self.entries()?,
            readme_path,
            readme,
            self.current_branch(),
            self.head_commit().unwrap_or(None),
            self.branches().unwrap_or_default(),
            self.tags().unwrap_or_default(),
        ))
    }

    pub fn checkout_branch(&self, name: &str) -> Result<()> {
        let full = format!("refs/heads/{name}");

        let branch = gix::refs::FullName::try_from(full.as_str())
            .map_err(|e| anyhow::anyhow!("invalid ref name: {e}"))?;

        let mut reference = self.inner.find_reference(&full)?;
        let tree = reference.peel_to_tree()?.id;

        let (signature, mut time_buf) = Self::repository_signature();
        let signature = signature.to_ref(&mut time_buf);

        self.move_head(
            signature,
            gix::refs::Target::Symbolic(branch),
            &format!("checkout: moving to {name}"),
        )?;

        self.force_checkout(&tree)?;

        Ok(())
    }

    pub fn checkout_tag(&self, name: &str) -> Result<()> {
        let full = format!("refs/tags/{name}");

        let mut reference = self.inner.find_reference(&full)?;

        let commit = reference.peel_to_id()?;
        let tree = reference.peel_to_tree()?.id;

        let (signature, mut time_buf) = Self::repository_signature();
        let signature = signature.to_ref(&mut time_buf);

        self.move_head(
            signature,
            gix::refs::Target::Object(commit.detach()),
            &format!("checkout: moving to {name}"),
        )?;

        self.force_checkout(&tree)?;

        Ok(())
    }

    pub(crate) fn force_checkout(&self, tree: &gix::hash::oid) -> Result<()> {
        let workdir = self
            .inner
            .workdir()
            .context("repository has no worktree")?
            .to_path_buf();

        let mut index = self.inner.index_from_tree(tree)?;

        if let Ok(previous) = self.inner.index_or_empty() {
            let keep: HashSet<PathBuf> = index
                .entries()
                .iter()
                .map(|entry| {
                    PathBuf::from(String::from_utf8_lossy(entry.path(&index)).into_owned())
                })
                .collect();
            for entry in previous.entries() {
                let rel = entry.path(&previous);
                let rel = PathBuf::from(String::from_utf8_lossy(rel).into_owned());

                if keep.contains(&rel) {
                    continue;
                }

                let path = workdir.join(&rel);

                match std::fs::remove_file(&path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => {
                        return Err(error)
                            .with_context(|| format!("failed to remove {}", path.display()));
                    }
                }
            }
        }

        let mut options = self
            .inner
            .checkout_options(gix_worktree::stack::state::attributes::Source::IdMapping)?;
        options.overwrite_existing = true;

        let objects = self.inner.objects.clone().into_arc()?;
        let files = gix::progress::Discard;
        let bytes = gix::progress::Discard;

        gix_worktree_state::checkout(
            &mut index,
            workdir,
            objects,
            &files,
            &bytes,
            &gix::interrupt::IS_INTERRUPTED,
            options,
        )
        .into_anyhow()?;

        index
            .write(gix::index::write::Options::default())
            .into_anyhow()?;

        Ok(())
    }

    fn move_head(
        &self,
        signature: gix::actor::SignatureRef<'_>,
        target: gix::refs::Target,
        message: &str,
    ) -> Result<()> {
        use gix::refs::transaction::{Change, LogChange, PreviousValue, RefEdit, RefLog};

        let head = gix::refs::FullName::try_from("HEAD")
            .map_err(|e| anyhow::anyhow!("invalid ref name: {e}"))?;

        self.inner.edit_references_as(
            [RefEdit {
                change: Change::Update {
                    log: LogChange {
                        mode: RefLog::AndReference,
                        force_create_reflog: false,
                        message: message.into(),
                    },
                    expected: PreviousValue::Any,
                    new: target,
                },
                name: head,
                deref: false,
            }],
            Some(signature),
        )?;

        Ok(())
    }

    fn collect_entries(root: &Path, dir: &Path, out: &mut Vec<(PathBuf, bool)>) -> Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            if entry.file_name() == ".git" {
                continue;
            }

            let is_dir = entry.file_type()?.is_dir();
            let path = entry.path();
            let rel = path.strip_prefix(root)?.to_path_buf();
            out.push((rel, is_dir));

            if is_dir {
                Self::collect_entries(root, &path, out)?;
            }
        }
        Ok(())
    }
}

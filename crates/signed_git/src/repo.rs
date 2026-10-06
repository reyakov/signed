use std::path::Path;

use anyhow::{Context, Result};

use crate::GixResultExt as _;

const OBJECT_CACHE_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoRefState {
    pub refs: Vec<(String, String)>,
    pub head: Option<String>,
}

impl RepoRefState {
    fn new(refs: Vec<(String, String)>, head: Option<String>) -> Self {
        Self { refs, head }
    }
}

/// One `open` per operation instead of every helper re-opening by path.
pub struct Repo {
    pub(crate) inner: gix::Repository,
}

impl Repo {
    pub fn open(workdir: &Path) -> Result<Self> {
        Ok(Self {
            inner: gix::open(workdir)?,
        })
    }

    pub fn try_open(workdir: &Path) -> Option<Self> {
        Self::open(workdir).ok()
    }

    pub fn open_cached(workdir: &Path) -> Result<Self> {
        let mut repo = gix::open(workdir)?;
        repo.object_cache_size_if_unset(OBJECT_CACHE_BYTES);
        Ok(Self { inner: repo })
    }

    pub fn init(path: &Path, name: &str, description: &str) -> Result<String> {
        use gix::refs::transaction::{Change, LogChange, PreviousValue, RefEdit, RefLog};

        std::fs::create_dir_all(path)
            .with_context(|| format!("failed to create {}", path.display()))?;

        let repo = gix::init(path)?;

        let (signature, mut time_buf) = Self::repository_signature();
        let signature = signature.to_ref(&mut time_buf);

        let head = gix::refs::FullName::try_from("HEAD")
            .map_err(|e| anyhow::anyhow!("invalid ref name: {e}"))?;

        repo.edit_references_as(
            [RefEdit {
                change: Change::Update {
                    log: LogChange {
                        mode: RefLog::AndReference,
                        force_create_reflog: false,
                        message: "checkout: moving to main".into(),
                    },
                    expected: PreviousValue::Any,
                    new: gix::refs::Target::Symbolic(
                        gix::refs::FullName::try_from("refs/heads/main")
                            .map_err(|e| anyhow::anyhow!("invalid ref name: {e}"))?,
                    ),
                },
                name: head,
                deref: false,
            }],
            Some(signature),
        )?;

        let readme = if description.trim().is_empty() {
            format!("# {name}\n")
        } else {
            format!("# {name}\n\n{description}\n")
        };

        std::fs::write(path.join("README.md"), &readme).context("failed to write README.md")?;

        let blob = repo.write_object(gix::objs::Blob {
            data: readme.into_bytes(),
        })?;

        let tree = repo.write_object(gix::objs::Tree {
            entries: vec![gix::objs::tree::Entry {
                mode: gix::objs::tree::EntryKind::Blob.into(),
                filename: gix::bstr::BString::from("README.md"),
                oid: blob.into(),
            }],
        })?;

        let commit = repo.commit_as(
            signature,
            signature,
            "HEAD",
            "Initial commit",
            tree,
            Vec::<gix::ObjectId>::new(),
        )?;

        // Populate the index so the fresh repository is clean, as `git add`
        // and `git commit` would leave it.
        let mut index = repo.index_from_tree(&tree)?;
        index
            .write(gix::index::write::Options::default())
            .into_anyhow()?;

        Ok(commit.to_string())
    }

    /// Not kept in any cache, unlike `GitCache::ensure_clone`.
    pub fn clone<U: AsRef<str>>(clone_urls: &[U], path: &Path) -> Result<Self> {
        if path.exists() {
            anyhow::bail!("destination {} already exists", path.display());
        }

        let mut last_error = None;

        for url in clone_urls {
            match Self::clone_from(url.as_ref(), path) {
                Ok(repo) => {
                    // The default-refspec clone misses the `refs/nostr/*` PR refs.
                    repo.fetch().ok();
                    return Ok(repo);
                }
                Err(error) => last_error = Some(error),
            }
        }

        match last_error {
            Some(error) => Err(error).context("failed to clone from any mirror"),
            None => anyhow::bail!("no clone URLs provided"),
        }
    }

    fn clone_from(url: &str, path: &Path) -> Result<Self> {
        let url = Self::transport_url(url);
        let url = gix::url::parse(url)
            .into_anyhow()
            .context("invalid clone URL")?;

        let mut prepare = gix::prepare_clone(url, path)?;
        let (mut checkout, _fetch) =
            prepare.fetch_then_checkout(gix::progress::Discard, &gix::interrupt::IS_INTERRUPTED)?;
        let (repo, _checkout) =
            checkout.main_worktree(gix::progress::Discard, &gix::interrupt::IS_INTERRUPTED)?;

        Ok(Self { inner: repo })
    }

    pub fn inner(&self) -> &gix::Repository {
        &self.inner
    }

    pub fn workdir(&self) -> Option<&Path> {
        self.inner.workdir()
    }

    /// `None` for an unborn HEAD.
    pub fn head(&self) -> Option<String> {
        self.inner.head_id().ok().map(|id| id.to_string())
    }

    pub fn merge_base(&self, a: &str, b: &str) -> Result<Option<String>> {
        let a = self.inner.rev_parse_single(a.as_bytes())?;
        let b = self.inner.rev_parse_single(b.as_bytes())?;
        // `merge_base` reports a missing base as an unclassified error, while the
        // many-bases variant keeps the distinction as an empty result.
        let bases = self.inner.merge_bases_many(a.detach(), &[b.detach()])?;
        Ok(bases.first().map(|id| id.to_string()))
    }

    pub fn commits_since(&self, base: Option<&str>) -> Result<Vec<String>> {
        let head = match self.inner.head_id() {
            Ok(head) => head,
            Err(_) if base.is_none() => return Ok(Vec::new()),
            Err(e) => return Err(e).context("repository has no commits"),
        };

        let Some(base) = base else {
            return Ok(vec![head.to_string()]);
        };

        let base = self.inner.rev_parse_single(base.as_bytes())?;
        let mut commits = Vec::new();

        for info in self
            .inner
            .rev_walk([head])
            .sorting(gix::revision::walk::Sorting::ByCommitTime(
                gix::traverse::commit::simple::CommitTimeOrder::NewestFirst,
            ))
            .with_hidden([base])
            .all()?
        {
            commits.push(info?.id().to_string());
        }

        commits.reverse();

        Ok(commits)
    }

    pub fn root_commit(&self) -> Result<Option<String>> {
        let Ok(head) = self.inner.head_id() else {
            return Ok(None);
        };

        for info in self
            .inner
            .rev_walk([head])
            .sorting(gix::revision::walk::Sorting::ByCommitTime(
                gix::traverse::commit::simple::CommitTimeOrder::NewestFirst,
            ))
            .all()?
        {
            let info = info?;
            if info.parent_ids().next().is_none() {
                return Ok(Some(info.id().to_string()));
            }
        }

        Ok(None)
    }

    pub fn refs_with_prefix(&self, prefix: &str) -> Result<Vec<String>> {
        let pattern = prefix.trim_end_matches('/');
        let mut names = Vec::new();

        for reference in self.inner.references()?.all()? {
            let reference = reference.map_err(|error| anyhow::anyhow!("{error}"))?;
            let name = String::from_utf8_lossy(reference.name().as_bstr()).into_owned();

            // Match the pattern itself and everything beneath it.
            let under_pattern = name
                .strip_prefix(pattern)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'));

            if under_pattern {
                names.push(name);
            }
        }

        names.sort();

        Ok(names)
    }

    pub fn delete_refs_with_prefix(&self, prefix: &str) -> Result<()> {
        use gix::refs::transaction::{Change, PreviousValue, RefEdit, RefLog};

        let refs = self.refs_with_prefix(prefix)?;
        if refs.is_empty() {
            return Ok(());
        }

        let edits: Vec<RefEdit> = refs
            .iter()
            .map(|name| {
                let full = gix::refs::FullName::try_from(name.as_str())
                    .map_err(|e| anyhow::anyhow!("invalid ref name {name}: {e}"))?;
                Ok(RefEdit {
                    change: Change::Delete {
                        expected: PreviousValue::Any,
                        log: RefLog::AndReference,
                    },
                    name: full,
                    deref: false,
                })
            })
            .collect::<Result<Vec<_>>>()?;

        self.inner.edit_references(edits)?;

        Ok(())
    }

    pub fn current_branch(&self) -> Option<String> {
        let head = self.inner.head().ok()?;
        let name = head.referent_name()?;
        Some(String::from_utf8_lossy(name.shorten()).into_owned())
    }

    pub fn ref_exists(&self, name: &str) -> bool {
        self.inner.find_reference(name).is_ok()
    }

    pub fn branches(&self) -> Result<Vec<String>> {
        let mut names = Vec::new();
        for reference in self.inner.references()?.local_branches()? {
            let reference = reference.map_err(|error| anyhow::anyhow!("{error}"))?;
            names.push(String::from_utf8_lossy(reference.name().shorten()).into_owned());
        }
        names.sort();
        Ok(names)
    }

    pub fn tags(&self) -> Result<Vec<String>> {
        let mut names = Vec::new();
        for reference in self.inner.references()?.tags()? {
            let reference = reference.map_err(|error| anyhow::anyhow!("{error}"))?;
            names.push(String::from_utf8_lossy(reference.name().shorten()).into_owned());
        }
        names.sort();
        Ok(names)
    }

    pub fn ref_state(&self) -> Result<RepoRefState> {
        let mut refs = Vec::new();

        for reference in self.inner.references()?.local_branches()? {
            let reference = reference.map_err(|error| anyhow::anyhow!("{error}"))?;
            refs.push((
                String::from_utf8_lossy(reference.name().as_bstr()).into_owned(),
                reference.id().to_string(),
            ));
        }

        for reference in self.inner.references()?.tags()? {
            let reference = reference.map_err(|error| anyhow::anyhow!("{error}"))?;
            refs.push((
                String::from_utf8_lossy(reference.name().as_bstr()).into_owned(),
                reference.id().to_string(),
            ));
        }
        refs.sort();

        let head = match self.inner.head() {
            Ok(head) => head
                .referent_name()
                .filter(|name| name.as_bstr().starts_with(b"refs/heads/"))
                .map(|name| String::from_utf8_lossy(name.shorten()).into_owned()),
            Err(_) => None,
        };

        Ok(RepoRefState::new(refs, head))
    }

    pub fn fast_forward_branches(&self) -> Result<bool> {
        use gix::refs::transaction::{Change, LogChange, PreviousValue, RefEdit, RefLog};

        if self.workdir().is_none() {
            return Ok(false);
        }

        let current = self.current_branch();
        let heads = self.refs_with_prefix("refs/heads")?;

        let (signature, mut time_buf) = Self::repository_signature();
        let signature = signature.to_ref(&mut time_buf);

        let mut moved = false;

        for head in heads {
            let Some(branch) = head.strip_prefix("refs/heads/") else {
                continue;
            };

            let remote = format!("refs/remotes/origin/{branch}");
            let Ok(mut remote_reference) = self.inner.find_reference(&remote) else {
                continue;
            };

            let Ok(mut local_reference) = self.inner.find_reference(&head) else {
                continue;
            };

            let Ok(remote_oid) = remote_reference.peel_to_id() else {
                continue;
            };

            let Ok(local_oid) = local_reference.peel_to_id() else {
                continue;
            };

            let remote_oid = remote_oid.detach();
            let local_oid = local_oid.detach();

            if local_oid == remote_oid {
                continue;
            }

            // Only fast-forward: the base must be the local tip.
            let Ok(base) = self.inner.merge_base(local_oid, remote_oid) else {
                continue;
            };

            if base != local_oid {
                continue;
            }

            let full = gix::refs::FullName::try_from(head.as_str())
                .map_err(|e| anyhow::anyhow!("invalid ref name: {e}"))?;

            let edit = |new: gix::refs::Target| RefEdit {
                change: Change::Update {
                    log: LogChange {
                        mode: RefLog::AndReference,
                        force_create_reflog: false,
                        message: format!("merge {remote}: Fast-forward").into(),
                    },
                    expected: PreviousValue::ExistingMustMatch(gix::refs::Target::Object(
                        local_oid,
                    )),
                    new,
                },
                name: full.clone(),
                deref: false,
            };

            if current.as_deref() == Some(branch) {
                // Only proceed on a clean worktree, like `git merge --ff-only`.
                if self.is_dirty() {
                    continue;
                }

                let tree = self.inner.find_object(remote_oid)?.peel_to_tree()?.id;

                self.force_checkout(&tree)?;

                self.inner.edit_references_as(
                    [edit(gix::refs::Target::Object(remote_oid))],
                    Some(signature),
                )?;

                moved = true;
            } else {
                self.inner.edit_references_as(
                    [edit(gix::refs::Target::Object(remote_oid))],
                    Some(signature),
                )?;

                moved = true;
            }
        }

        Ok(moved)
    }

    pub(crate) fn repository_signature() -> (gix::actor::Signature, gix::date::parse::TimeBuf) {
        let seconds = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_secs() as i64)
            .unwrap_or_default();

        let signature = gix::actor::Signature {
            name: gix::bstr::BString::from("Signed"),
            email: gix::bstr::BString::from("signed@localhost"),
            time: gix::date::Time { seconds, offset: 0 },
        };

        (signature, gix::date::parse::TimeBuf::default())
    }

    pub(crate) fn transport_url(url: &str) -> String {
        url.strip_prefix("grasp://")
            .map(|rest| format!("https://{rest}"))
            .unwrap_or_else(|| url.to_owned())
    }
}

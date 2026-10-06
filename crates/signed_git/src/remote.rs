use std::collections::HashMap;
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use gix::interrupt::IS_INTERRUPTED;
use gix::progress::Discard;

use crate::GixResultExt as _;
use crate::repo::Repo;

impl Repo {
    pub fn fetch(&self) -> Result<()> {
        let options = gix::remote::ref_map::Options {
            extra_refspecs: vec![
                gix::refspec::parse(
                    gix::bstr::BStr::new("+refs/nostr/*:refs/nostr/*"),
                    gix::refspec::parse::Operation::Fetch,
                )
                .into_anyhow()?
                .to_owned(),
            ],
            ..Default::default()
        };
        self.inner
            .find_remote("origin")?
            .connect(gix::remote::Direction::Fetch)?
            .prepare_fetch(Discard, options)?
            .receive(Discard, &IS_INTERRUPTED)?;
        Ok(())
    }

    pub fn push_ref(&self, url: &str, commit: &str, reference: &str) -> Result<()> {
        let output = Self::run_git(
            self.workdir_or_dot(),
            &["push", url, &format!("{commit}:{reference}")],
            "git push",
        )?;

        if !output.status.success() {
            bail!(
                "git push failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(())
    }

    pub fn push_main(&self, base_url: &str, owner: &str, repo_id: &str) -> Result<()> {
        self.push_refspecs(
            base_url,
            owner,
            repo_id,
            &["refs/heads/main:refs/heads/main"],
        )
    }

    pub fn push_all(&self, base_url: &str, owner: &str, repo_id: &str) -> Result<()> {
        self.push_refspecs(
            base_url,
            owner,
            repo_id,
            &["refs/heads/*:refs/heads/*", "refs/tags/*:refs/tags/*"],
        )
    }

    fn push_refspecs(
        &self,
        base_url: &str,
        owner: &str,
        repo_id: &str,
        refspecs: &[&str],
    ) -> Result<()> {
        let url = format!("{base_url}/{owner}/{repo_id}.git");

        let mut args: Vec<&str> = Vec::with_capacity(refspecs.len() + 2);
        args.push("push");
        args.push(&url);
        args.extend_from_slice(refspecs);

        let output = Self::run_git(self.workdir_or_dot(), &args, "git push")?;

        if !output.status.success() {
            bail!(
                "git push to {base_url} failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(())
    }

    pub fn remote_has_refs(&self, url: &str, expected: &[(String, String)]) -> Result<bool> {
        if expected.is_empty() {
            return Ok(true);
        }

        let url = Self::transport_url(url);

        let refspecs = expected
            .iter()
            .map(|(name, _)| {
                gix::refspec::parse(
                    gix::bstr::BStr::new(format!("+{name}:{name}").as_bytes()),
                    gix::refspec::parse::Operation::Fetch,
                )
                .map(|spec| spec.to_owned())
            })
            .collect::<Result<Vec<_>, _>>()
            .into_anyhow()
            .context("invalid refspec")?;

        let options = gix::remote::ref_map::Options {
            extra_refspecs: refspecs,
            ..Default::default()
        };

        let (refs, _) = self
            .inner
            .remote_at(url.as_str())
            .with_context(|| format!("cannot use remote {url}"))?
            .connect(gix::remote::Direction::Fetch)
            .with_context(|| format!("cannot connect to {url}"))?
            .ref_map(Discard, options)
            .with_context(|| format!("listing refs of {url} failed"))?;

        // Peeled tag entries carry the tag object in their direct oid,
        // matching `git ls-remote` while skipping the duplicated `^{}` lines.
        let advertised: HashMap<String, String> = refs
            .remote_refs
            .iter()
            .filter_map(|reference| {
                let (name, object, _peeled) = reference.unpack();
                object.map(|oid| (String::from_utf8_lossy(name).into_owned(), oid.to_string()))
            })
            .collect();

        Ok(expected
            .iter()
            .all(|(name, oid)| advertised.get(name.as_str()) == Some(oid)))
    }

    pub fn ensure_origin(&self, url: &str) -> Result<()> {
        if self.inner.find_remote("origin").is_ok() {
            return Ok(());
        }

        // `git remote add` also configures the default fetch refspec.
        self.edit_local_config(|config| {
            config
                .set_raw_value("remote.origin.url", url)
                .into_anyhow()?;
            config
                .set_raw_value("remote.origin.fetch", "+refs/heads/*:refs/remotes/origin/*")
                .into_anyhow()?;
            Ok(())
        })
    }

    // A working copy cloned from a local mirror is re-targeted at the grasp server,
    // a pre-existing fetch refspec is left untouched.
    pub fn set_origin(&self, url: &str) -> Result<()> {
        let had_origin = self.inner.find_remote("origin").is_ok();

        self.edit_local_config(|config| {
            config
                .set_raw_value("remote.origin.url", url)
                .into_anyhow()?;

            if !had_origin {
                config
                    .set_raw_value("remote.origin.fetch", "+refs/heads/*:refs/remotes/origin/*")
                    .into_anyhow()?;
            }

            Ok(())
        })
    }

    pub fn origin_url(&self) -> Result<Option<String>> {
        let Ok(remote) = self.inner.find_remote("origin") else {
            return Ok(None);
        };

        Ok(remote
            .url(gix::remote::Direction::Fetch)
            .map(|url| url.to_string()))
    }

    pub fn fetch_refs<U: AsRef<str>>(&self, urls: &[U], refspec: &str) -> Result<()> {
        let refspec = gix::refspec::parse(
            gix::bstr::BStr::new(refspec),
            gix::refspec::parse::Operation::Fetch,
        )
        .into_anyhow()
        .context("invalid fetch refspec")?
        .to_owned();

        let mut last_error = None;

        for url in urls {
            let url = Self::transport_url(url.as_ref());
            let outcome = (|| -> Result<()> {
                let options = gix::remote::ref_map::Options {
                    extra_refspecs: vec![refspec.clone()],
                    ..Default::default()
                };
                self.inner
                    .remote_at(url.as_str())
                    .with_context(|| format!("fetch from {url} failed"))?
                    .connect(gix::remote::Direction::Fetch)
                    .with_context(|| format!("fetch from {url} failed"))?
                    .prepare_fetch(Discard, options)
                    .with_context(|| format!("fetch from {url} failed"))?
                    .receive(Discard, &IS_INTERRUPTED)
                    .with_context(|| format!("fetch from {url} failed"))?;
                Ok(())
            })();

            match outcome {
                Ok(()) => return Ok(()),
                Err(error) => last_error = Some(error),
            }
        }

        match last_error {
            Some(error) => Err(error).context("failed to fetch from any mirror"),
            None => bail!("no fetch URLs provided"),
        }
    }

    pub(crate) fn edit_local_config(
        &self,
        edit: impl FnOnce(&mut gix::config::File) -> Result<()>,
    ) -> Result<()> {
        let config_path = self.inner.common_dir().join("config");

        let mut lock = gix::lock::File::acquire_to_update_resource(
            &config_path,
            gix::lock::acquire::Fail::Immediately,
            None,
        )
        .into_anyhow()
        .context("failed to lock repository config")?;

        let mut config =
            match gix::config::File::from_path_no_includes(config_path, gix::config::Source::Local)
            {
                Ok(config) => config,
                Err(error) if error.is_not_found() => gix::config::File::default(),
                Err(error) => {
                    return Err(error)
                        .into_anyhow()
                        .context("failed to read repository config");
                }
            };

        edit(&mut config)?;

        config
            .write_to(&mut lock)
            .context("failed to write repository config")?;

        lock.commit().context("failed to save repository config")?;

        Ok(())
    }

    pub(crate) fn workdir_or_dot(&self) -> &Path {
        self.inner.workdir().unwrap_or_else(|| Path::new("."))
    }

    pub(crate) fn run_git(dir: &Path, args: &[&str], what: &str) -> Result<std::process::Output> {
        Self::git_command(dir)
            .args(args)
            .stderr(Stdio::piped())
            .output()
            .with_context(|| format!("failed to spawn `{what}`"))
    }

    pub(crate) fn git_command(dir: &Path) -> Command {
        let mut command = Command::new("git");
        command.arg("-C").arg(dir).env("GIT_TERMINAL_PROMPT", "0");
        command
    }
}

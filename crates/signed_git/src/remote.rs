use std::collections::HashMap;
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use gix::interrupt::IS_INTERRUPTED;
use gix::progress::Discard;

/// Clone into `path` from the first working URL in `clone_urls`.
///
/// Unlike [`GitCache::ensure_clone`], the clone is not kept in any cache.
pub fn clone_repo<U: AsRef<str>>(clone_urls: &[U], path: &Path) -> Result<()> {
    if path.exists() {
        bail!("destination {} already exists", path.display());
    }

    try_each_url(clone_urls, "clone", |url| {
        let repo = clone(url, path)?;
        // The initial clone uses the default refspecs. Also fetch the `refs/nostr/*` PR refs.
        fetch_all(&repo).ok();
        Ok(())
    })
}

/// Fetch all configured refspecs from `origin`, plus the `refs/nostr/*` namespace.
pub fn fetch_all(repo: &gix::Repository) -> Result<()> {
    let options = gix::remote::ref_map::Options {
        extra_refspecs: vec![
            gix::refspec::parse(
                gix::bstr::BStr::new("+refs/nostr/*:refs/nostr/*"),
                gix::refspec::parse::Operation::Fetch,
            )?
            .to_owned(),
        ],
        ..Default::default()
    };
    repo.find_remote("origin")?
        .connect(gix::remote::Direction::Fetch)?
        .prepare_fetch(Discard, options)?
        .receive(Discard, &IS_INTERRUPTED)?;
    Ok(())
}

pub fn push_commit_ref(repo_path: &Path, url: &str, commit: &str, reference: &str) -> Result<()> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .args(["push"])
        .arg(url)
        .arg(format!("{commit}:{reference}"))
        .env("GIT_TERMINAL_PROMPT", "0")
        .stderr(Stdio::piped())
        .output()
        .context("failed to spawn `git push`")?;

    if !output.status.success() {
        bail!(
            "git push failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

/// Rewrite a grasp server URL to the https URL the git transport actually uses.
///
/// GRASP servers announce `grasp://<host>/<owner>/<repo>` clone URLs.
/// The transport is git smart HTTP, so the scheme is rewritten for gix.
fn transport_url(url: &str) -> String {
    url.strip_prefix("grasp://")
        .map(|rest| format!("https://{rest}"))
        .unwrap_or_else(|| url.to_owned())
}

/// Run `attempt` against each URL in `urls` until one succeeds.
///
/// Returns the last error wrapped in `failed to {verb} from any mirror`,
/// or `no clone URLs provided` when the list is empty.
fn try_each_url<U: AsRef<str>, F>(urls: &[U], verb: &str, mut attempt: F) -> Result<()>
where
    F: FnMut(&str) -> Result<()>,
{
    let mut last_err: Option<anyhow::Error> = None;

    for url in urls {
        match attempt(url.as_ref()) {
            Ok(()) => return Ok(()),
            Err(e) => last_err = Some(e),
        }
    }

    match last_err {
        Some(e) => Err(e).context(format!("failed to {verb} from any mirror")),
        None => bail!("no clone URLs provided"),
    }
}

fn clone(url: &str, path: &Path) -> Result<gix::Repository> {
    let url = transport_url(url);
    let url = gix::url::parse(url).context("invalid clone URL")?;

    let mut prepare = gix::prepare_clone(url, path)?;
    let (mut checkout, _fetch) = prepare.fetch_then_checkout(Discard, &IS_INTERRUPTED)?;
    let (repo, _checkout) = checkout.main_worktree(Discard, &IS_INTERRUPTED)?;

    Ok(repo)
}

pub fn push_main(repo_path: &Path, base_url: &str, owner: &str, repo_id: &str) -> Result<()> {
    push_refspecs(
        repo_path,
        base_url,
        owner,
        repo_id,
        &["refs/heads/main:refs/heads/main"],
    )
}

/// Push every local branch and tag of the repository at `repo_path` to a grasp server.
///
/// This mirrors an initialized repository's whole history.
pub fn push_all(repo_path: &Path, base_url: &str, owner: &str, repo_id: &str) -> Result<()> {
    push_refspecs(
        repo_path,
        base_url,
        owner,
        repo_id,
        &["refs/heads/*:refs/heads/*", "refs/tags/*:refs/tags/*"],
    )
}

fn push_refspecs(
    repo_path: &Path,
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

    let output = git_output(repo_path, &args, "git push")?;

    if !output.status.success() {
        bail!(
            "git push to {base_url} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

/// Whether `url` advertises every ref in `expected` at the given commit.
///
/// Extra advertised refs are ignored: the question is whether the data this
/// push wanted to land is already there, not whether the remote is an exact mirror.
/// This is the convergence probe for a push that lost the compare-and-swap race
/// to the grasp server's own background ref alignment.
pub fn remote_has_refs(repo_path: &Path, url: &str, expected: &[(String, String)]) -> Result<bool> {
    if expected.is_empty() {
        return Ok(true);
    }

    let repo = gix::open(repo_path)?;
    let url = transport_url(url);

    // A URL-created remote has no configured fetch refspecs, and `ref_map` only
    // keeps refs that match one. Match each expected ref by its exact name,
    // like `git ls-remote <url> <name>` would; ref maps never write to the repository.
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
        .context("invalid refspec")?;

    let options = gix::remote::ref_map::Options {
        extra_refspecs: refspecs,
        ..Default::default()
    };

    let (refs, _) = repo
        .remote_at(url.as_str())
        .with_context(|| format!("cannot use remote {url}"))?
        .connect(gix::remote::Direction::Fetch)
        .with_context(|| format!("cannot connect to {url}"))?
        .ref_map(Discard, options)
        .with_context(|| format!("listing refs of {url} failed"))?;

    // Peeled tag entries carry the tag object in their direct oid, so mapping
    // each advertised ref to its direct oid matches `git ls-remote` while
    // skipping the duplicated `^{}` lines.
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

/// Add `origin` pointing at `url` when the repository has no remote yet.
///
/// No-op if `origin` already exists.
pub fn ensure_origin(repo_path: &Path, url: &str) -> Result<()> {
    let repo = gix::open(repo_path)?;
    if repo.find_remote("origin").is_ok() {
        return Ok(());
    }

    // `git remote add` also configures the default fetch refspec.
    edit_local_config(&repo, |config| {
        config.set_raw_value("remote.origin.url", url)?;
        config.set_raw_value("remote.origin.fetch", "+refs/heads/*:refs/remotes/origin/*")?;
        Ok(())
    })
}

/// Point `origin` at `url`, replacing an existing remote,
/// used after a clone whose `origin` points at the cloned-from path.
///
/// A working copy cloned from a local mirror is re-targeted at the grasp server.
pub fn set_origin(repo_path: &Path, url: &str) -> Result<()> {
    let repo = gix::open(repo_path)?;
    let had_origin = repo.find_remote("origin").is_ok();

    edit_local_config(&repo, |config| {
        // Replaces the existing url, like `git remote set-url origin <url>`.
        // A pre-existing fetch refspec is left untouched.
        config.set_raw_value("remote.origin.url", url)?;

        if !had_origin {
            config.set_raw_value("remote.origin.fetch", "+refs/heads/*:refs/remotes/origin/*")?;
        }

        Ok(())
    })
}

/// Apply `edit` to the repository-local configuration and persist it.
///
/// The config file is locked while it is read, edited and written back,
/// like git would when running `git config` or `git remote`.
fn edit_local_config(
    repo: &gix::Repository,
    edit: impl FnOnce(&mut gix::config::File) -> Result<()>,
) -> Result<()> {
    let config_path = repo.common_dir().join("config");

    let mut lock = gix::lock::File::acquire_to_update_resource(
        &config_path,
        gix::lock::acquire::Fail::Immediately,
        None,
    )
    .context("failed to lock repository config")?;

    let mut config =
        match gix::config::File::from_path_no_includes(config_path, gix::config::Source::Local) {
            Ok(config) => config,
            // A repository without a config file yet starts from scratch.
            Err(gix::config::file::init::from_paths::Error::Io { source, .. })
                if source.kind() == std::io::ErrorKind::NotFound =>
            {
                gix::config::File::default()
            }
            Err(error) => return Err(error).context("failed to read repository config"),
        };

    edit(&mut config)?;

    config
        .write_to(&mut lock)
        .context("failed to write repository config")?;

    lock.commit().context("failed to save repository config")?;

    Ok(())
}

/// Fetch `refspec` into `repo_path` from the first working URL in `urls`.
/// When no URL works, the last error is returned.
///
/// Never touches the checked-out refs or the worktree.
pub fn fetch_repo_refs<U: AsRef<str>>(repo_path: &Path, urls: &[U], refspec: &str) -> Result<()> {
    let repo = gix::open(repo_path)?;
    let refspec = gix::refspec::parse(
        gix::bstr::BStr::new(refspec),
        gix::refspec::parse::Operation::Fetch,
    )
    .context("invalid fetch refspec")?
    .to_owned();

    try_each_url(urls, "fetch", |url| {
        let url = transport_url(url);
        let options = gix::remote::ref_map::Options {
            extra_refspecs: vec![refspec.clone()],
            ..Default::default()
        };
        repo.remote_at(url.as_str())
            .with_context(|| format!("fetch from {url} failed"))?
            .connect(gix::remote::Direction::Fetch)
            .with_context(|| format!("fetch from {url} failed"))?
            .prepare_fetch(Discard, options)
            .with_context(|| format!("fetch from {url} failed"))?
            .receive(Discard, &IS_INTERRUPTED)
            .with_context(|| format!("fetch from {url} failed"))?;
        Ok(())
    })
}

/// The URL of the `origin` remote of the repository at `workdir`.
///
/// `None` when it has no `origin` yet.
pub fn origin_url(workdir: &Path) -> Result<Option<String>> {
    let Ok(repo) = gix::open(workdir) else {
        return Ok(None);
    };

    let Ok(remote) = repo.find_remote("origin") else {
        return Ok(None);
    };

    Ok(remote
        .url(gix::remote::Direction::Fetch)
        .map(|url| url.to_string()))
}

/// Run `git -C dir args`, disabling the terminal prompt and capturing stderr.
///
/// `what` names the command in the spawn error.
pub(crate) fn git_output(dir: &Path, args: &[&str], what: &str) -> Result<std::process::Output> {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stderr(Stdio::piped())
        .output()
        .with_context(|| format!("failed to spawn `{what}`"))
}

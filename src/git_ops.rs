use git2::{Oid, Repository, StatusOptions};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::model::{AppResult, Impact};

pub fn repo_root(repo: &Repository) -> AppResult<PathBuf> {
    let path = repo
        .workdir()
        .ok_or_else(|| "bare repositories are not supported".to_string())?;
    Ok(path.to_path_buf())
}

pub fn ensure_clean_repo(repo: &Repository) -> AppResult<()> {
    let mut opts = StatusOptions::new();
    opts.include_untracked(false)
        .include_ignored(false)
        .renames_head_to_index(true)
        .renames_index_to_workdir(true);

    let statuses = repo
        .statuses(Some(&mut opts))
        .map_err(|e| format!("failed to read git status: {e}"))?;

    if statuses.is_empty() {
        Ok(())
    } else {
        Err("please commit or stash changes before running bumper".to_string())
    }
}

pub fn current_branch(repo: &Repository) -> AppResult<String> {
    let head = repo.head().map_err(|e| format!("not on a branch: {e}"))?;
    let shorthand = head
        .shorthand()
        .ok_or_else(|| "not on a branch".to_string())?;
    Ok(shorthand.to_string())
}

pub fn latest_tag(repo: &Repository) -> AppResult<(String, Oid)> {
    let tags = repo
        .tag_names(None)
        .map_err(|e| format!("failed to list tags: {e}"))?;

    let mut latest: Option<(String, Oid, i64)> = None;

    for maybe_name in tags.iter() {
        let Some(name) = maybe_name else {
            continue;
        };

        let object = repo
            .revparse_single(&format!("refs/tags/{name}"))
            .or_else(|_| repo.revparse_single(name))
            .map_err(|e| format!("failed to read tag '{name}': {e}"))?;

        let commit = object
            .peel_to_commit()
            .map_err(|e| format!("tag '{name}' does not reference a commit: {e}"))?;
        let time = commit.time().seconds();

        match &latest {
            Some((_, _, latest_time)) if *latest_time >= time => {}
            _ => latest = Some((name.to_string(), commit.id(), time)),
        }
    }

    match latest {
        Some((name, oid, _)) => Ok((name, oid)),
        None => Err("no git tags found, please create a tag first".to_string()),
    }
}

pub fn get_impact(
    repo: &Repository,
    last_tag_commit: Oid,
    major_types: &HashSet<String>,
    minor_types: &HashSet<String>,
    patch_types: &HashSet<String>,
    skip_scopes: &HashSet<String>,
    force: bool,
) -> AppResult<Option<Impact>> {
    let mut impact = if force { Some(Impact::Patch) } else { None };

    let mut walk = repo
        .revwalk()
        .map_err(|e| format!("failed to create revwalk: {e}"))?;
    walk.push_head()
        .map_err(|e| format!("failed to walk from HEAD: {e}"))?;
    walk.hide(last_tag_commit)
        .map_err(|e| format!("failed to hide last tag commit: {e}"))?;

    for oid in walk {
        let oid = oid.map_err(|e| format!("failed to walk commit history: {e}"))?;
        let commit = repo
            .find_commit(oid)
            .map_err(|e| format!("failed to load commit {oid}: {e}"))?;

        let Some(summary) = commit.summary() else {
            continue;
        };

        let Some((prefix, _)) = summary.split_once(':') else {
            continue;
        };

        let typ = prefix.split('(').next().unwrap_or(prefix).trim();
        let mut scope = "none";
        if let Some(start) = prefix.find('(')
            && let Some(end) = prefix[start + 1..].find(')')
        {
            scope = &prefix[start + 1..start + 1 + end];
        }

        if skip_scopes.contains(&scope.trim().to_ascii_lowercase()) {
            continue;
        }

        if prefix.trim_end().ends_with('!') || major_types.contains(&typ.to_ascii_lowercase()) {
            impact = Some(Impact::Major);
            break;
        }

        if minor_types.contains(&typ.to_ascii_lowercase()) {
            if impact.unwrap_or(Impact::Patch) < Impact::Minor {
                impact = Some(Impact::Minor);
            }
            continue;
        }

        if patch_types.contains(&typ.to_ascii_lowercase()) && impact.is_none() {
            impact = Some(Impact::Patch);
        }
    }

    Ok(impact)
}

pub fn list_tracked_files_under(
    repo: &Repository,
    repo_root: &Path,
    directory: &Path,
) -> AppResult<Vec<PathBuf>> {
    let dir_relative = directory.strip_prefix(repo_root).unwrap_or(directory);

    let index = repo
        .index()
        .map_err(|e| format!("failed to read git index: {e}"))?;

    let mut files = Vec::new();
    for entry in index.iter() {
        let Ok(path) = std::str::from_utf8(&entry.path) else {
            continue;
        };
        let relative = Path::new(path);
        if relative.parent() == Some(dir_relative) {
            files.push(repo_root.join(relative));
        }
    }

    files.sort();
    files.dedup();
    Ok(files)
}

pub fn stage_path(repo: &Repository, repo_root: &Path, absolute_path: &Path) -> AppResult<()> {
    let relative = absolute_path
        .strip_prefix(repo_root)
        .map_err(|_| format!("file is outside repository: {}", absolute_path.display()))?;

    let mut index = repo
        .index()
        .map_err(|e| format!("failed to open git index: {e}"))?;
    index
        .add_path(relative)
        .map_err(|e| format!("failed to stage '{}': {e}", relative.display()))?;
    index
        .write()
        .map_err(|e| format!("failed to write git index: {e}"))
}

pub fn staged_files(repo: &Repository) -> AppResult<Vec<PathBuf>> {
    let mut opts = StatusOptions::new();
    opts.include_untracked(false)
        .include_ignored(false)
        .renames_head_to_index(true)
        .renames_index_to_workdir(true);

    let statuses = repo
        .statuses(Some(&mut opts))
        .map_err(|e| format!("failed to read git status: {e}"))?;

    let staged = statuses
        .iter()
        .filter_map(|entry| {
            let status = entry.status();
            let indexed = status.is_index_new()
                || status.is_index_modified()
                || status.is_index_deleted()
                || status.is_index_renamed()
                || status.is_index_typechange();
            if indexed {
                entry.path().map(PathBuf::from)
            } else {
                None
            }
        })
        .collect();

    Ok(staged)
}

pub fn git_commit(repo: &Repository, message: &str) -> AppResult<()> {
    let sig = repo
        .signature()
        .map_err(|e| format!("failed to get git signature: {e}"))?;
    let mut index = repo
        .index()
        .map_err(|e| format!("failed to open git index: {e}"))?;
    let tree_oid = index
        .write_tree()
        .map_err(|e| format!("failed to write tree: {e}"))?;
    let tree = repo
        .find_tree(tree_oid)
        .map_err(|e| format!("failed to find tree: {e}"))?;
    let parent = repo
        .head()
        .and_then(|h| h.peel_to_commit())
        .map_err(|e| format!("failed to get HEAD commit: {e}"))?;
    repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &[&parent])
        .map_err(|e| format!("failed to create commit: {e}"))?;
    Ok(())
}

pub fn git_tag(repo: &Repository, tag_name: &str, message: &str) -> AppResult<()> {
    let sig = repo
        .signature()
        .map_err(|e| format!("failed to get git signature: {e}"))?;
    let head_commit = repo
        .head()
        .and_then(|h| h.peel_to_commit())
        .map_err(|e| format!("failed to get HEAD commit: {e}"))?;
    repo.tag(tag_name, head_commit.as_object(), &sig, message, false)
        .map_err(|e| format!("failed to create tag '{tag_name}': {e}"))?;
    Ok(())
}

fn make_remote_callbacks<'a>() -> git2::RemoteCallbacks<'a> {
    let mut callbacks = git2::RemoteCallbacks::new();
    // SSH is retried up to twice: first via the agent, then via a key file.
    // Other credential types are tried once each, tracked by bitmask.
    let ssh_attempts = std::cell::Cell::new(0u8);
    let tried = std::cell::Cell::new(git2::CredentialType::empty());
    callbacks.credentials(move |url, username, allowed| {
        let user = username.unwrap_or("git");
        if allowed.contains(git2::CredentialType::SSH_KEY) {
            let n = ssh_attempts.get();
            ssh_attempts.set(n + 1);
            if n == 0 {
                return git2::Cred::ssh_key_from_agent(user);
            }
            if n == 1
                && let Some(key_path) = find_ssh_key()
            {
                return git2::Cred::ssh_key(user, None, &key_path, None);
            }
            return Err(git2::Error::from_str("SSH authentication failed"));
        }
        let remaining = allowed & !tried.get();
        if remaining.is_empty() {
            return Err(git2::Error::from_str("authentication failed"));
        }
        if remaining.contains(git2::CredentialType::USER_PASS_PLAINTEXT) {
            tried.set(tried.get() | git2::CredentialType::USER_PASS_PLAINTEXT);
            // GITHUB_TOKEN is the standard credential in CI (set by actions/checkout
            // via http.extraheader, which libgit2 does not read natively).
            if let Ok(token) = std::env::var("GITHUB_TOKEN") {
                return git2::Cred::userpass_plaintext("x-access-token", &token);
            }
            if let Ok(config) = git2::Config::open_default() {
                return git2::Cred::credential_helper(&config, url, username);
            }
        }
        if remaining.contains(git2::CredentialType::DEFAULT) {
            tried.set(tried.get() | git2::CredentialType::DEFAULT);
            return git2::Cred::default();
        }
        Err(git2::Error::from_str("no suitable credentials"))
    });
    callbacks
}

fn find_ssh_key() -> Option<std::path::PathBuf> {
    let home = std::env::var_os("HOME")?;
    let ssh_dir = std::path::Path::new(&home).join(".ssh");
    for name in &["id_ed25519", "id_ecdsa", "id_rsa"] {
        let path = ssh_dir.join(name);
        if path.exists() {
            return Some(path);
        }
    }
    None
}

const FETCH_TIMEOUT: Duration = Duration::from_secs(60);

pub fn git_fetch(repo: &Repository) -> AppResult<()> {
    let repo_path = repo_root(repo)?;
    let remotes_array = repo
        .remotes()
        .map_err(|e| format!("failed to list remotes: {e}"))?;
    let remotes: Vec<String> = remotes_array
        .iter()
        .flatten()
        .map(|s: &str| s.to_string())
        .collect();
    for name in remotes {
        let path = repo_path.clone();
        let remote_name = name.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(fetch_from_remote(&path, &remote_name));
        });
        match rx.recv_timeout(FETCH_TIMEOUT) {
            Ok(result) => result?,
            Err(_) => {
                return Err(format!(
                    "fetch from '{name}' timed out after {FETCH_TIMEOUT:?}"
                ));
            }
        }
    }
    Ok(())
}

fn fetch_from_remote(repo_path: &Path, name: &str) -> AppResult<()> {
    let repo =
        git2::Repository::open(repo_path).map_err(|e| format!("failed to open repository: {e}"))?;
    let mut remote = repo
        .find_remote(name)
        .map_err(|e| format!("failed to find remote '{name}': {e}"))?;
    let mut opts = git2::FetchOptions::new();
    opts.remote_callbacks(make_remote_callbacks());
    opts.download_tags(git2::AutotagOption::All);
    remote
        .fetch(&[] as &[&str], Some(&mut opts), None)
        .map_err(|e| format!("failed to fetch from '{name}': {e}"))
}

pub fn git_push(repo: &Repository, branch: &str, tag: &str) -> AppResult<()> {
    let mut remote = repo
        .find_remote("origin")
        .map_err(|e| format!("failed to find remote 'origin': {e}"))?;
    let branch_ref = format!("refs/heads/{branch}:refs/heads/{branch}");
    let tag_ref = format!("refs/tags/{tag}:refs/tags/{tag}");

    let callbacks = make_remote_callbacks();
    let mut push_options = git2::PushOptions::new();
    push_options.remote_callbacks(callbacks);
    push_options.remote_push_options(&["atomic"]);

    remote
        .push(
            &[branch_ref.as_str(), tag_ref.as_str()],
            Some(&mut push_options),
        )
        .map_err(|e| format!("failed to push to origin: {e}"))?;
    Ok(())
}

use git2::{Oid, Repository, StatusOptions};
use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use crate::model::{AppResult, Impact};
use crate::versioning::{Version, parse_version};

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
        .map_err(|e| format!("failed to read branch name: {e}"))?;
    Ok(shorthand.to_string())
}

pub fn latest_tag_or_none(
    repo: &Repository,
    package_path: &Path,
) -> AppResult<Option<(String, Oid)>> {
    let tags = repo
        .tag_names(None)
        .map_err(|e| format!("failed to list tags: {e}"))?;
    let package_scope = package_scope(package_path)?;

    let mut release_tags: HashMap<Oid, Vec<(String, Version)>> = HashMap::new();

    for maybe_name in tags.iter() {
        let Ok(Some(name)) = maybe_name else {
            continue;
        };

        let version_name = if package_scope.is_empty() {
            name.strip_prefix('v')
                .or_else(|| name.strip_prefix('V'))
                .unwrap_or(name)
        } else {
            let Some(version_name) = name
                .strip_prefix(package_scope.as_str())
                .and_then(|name| name.strip_prefix('/'))
                .and_then(|name| name.strip_prefix('v').or_else(|| name.strip_prefix('V')))
            else {
                continue;
            };
            version_name
        };
        let Ok(version) = parse_version(version_name) else {
            continue;
        };

        let object = repo
            .revparse_single(&format!("refs/tags/{name}"))
            .or_else(|_| repo.revparse_single(name))
            .map_err(|e| format!("failed to read tag '{name}': {e}"))?;

        let commit = object
            .peel_to_commit()
            .map_err(|e| format!("tag '{name}' does not reference a commit: {e}"))?;
        release_tags
            .entry(commit.id())
            .or_default()
            .push((name.to_string(), version));
    }

    if release_tags.is_empty() {
        return Ok(None);
    }

    let mut walk = repo
        .revwalk()
        .map_err(|e| format!("failed to create revwalk: {e}"))?;
    walk.push_head()
        .map_err(|e| format!("failed to walk from HEAD: {e}"))?;
    walk.set_sorting(git2::Sort::TOPOLOGICAL | git2::Sort::TIME)
        .map_err(|e| format!("failed to configure revwalk: {e}"))?;

    for oid in walk {
        let oid = oid.map_err(|e| format!("failed to walk commit history: {e}"))?;
        if let Some(tags) = release_tags.get(&oid) {
            let (name, _) = tags
                .iter()
                .max_by_key(|(_, version)| *version)
                .expect("release tag list should not be empty");
            return Ok(Some((name.clone(), oid)));
        }
    }

    let expected_stream = if package_scope.is_empty() {
        "root stream".to_string()
    } else {
        format!("package stream '{package_scope}'")
    };
    Err(format!(
        "no semantic version git tags found for {expected_stream} reachable from HEAD"
    ))
}

fn package_scope(package_path: &Path) -> AppResult<String> {
    package_path
        .iter()
        .map(OsStr::to_str)
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| {
            format!(
                "package path is not valid UTF-8: {}",
                package_path.display()
            )
        })
        .map(|parts| parts.join("/"))
}

pub struct ImpactConfig<'a> {
    pub major_types: &'a HashSet<String>,
    pub minor_types: &'a HashSet<String>,
    pub patch_types: &'a HashSet<String>,
    pub skip_scopes: &'a HashSet<String>,
    pub ignored_directories: &'a [PathBuf],
    pub force: bool,
}

pub fn get_impact_for_package(
    repo: &Repository,
    baseline_commit: Option<Oid>,
    package_path: &Path,
    child_package_paths: &[PathBuf],
    config: &ImpactConfig<'_>,
) -> AppResult<Option<Impact>> {
    let mut impact = if config.force {
        Some(Impact::Patch)
    } else {
        None
    };

    let mut walk = repo
        .revwalk()
        .map_err(|e| format!("failed to create revwalk: {e}"))?;
    walk.push_head()
        .map_err(|e| format!("failed to walk from HEAD: {e}"))?;
    if let Some(baseline_commit) = baseline_commit {
        walk.hide(baseline_commit)
            .map_err(|e| format!("failed to hide release baseline commit: {e}"))?;
    }

    for oid in walk {
        let oid = oid.map_err(|e| format!("failed to walk commit history: {e}"))?;
        let commit = repo
            .find_commit(oid)
            .map_err(|e| format!("failed to load commit {oid}: {e}"))?;

        if !commit_touches_package(
            repo,
            &commit,
            package_path,
            child_package_paths,
            config.ignored_directories,
        )? {
            continue;
        }

        let message = commit
            .message()
            .map_err(|e| format!("failed to read commit {oid} message: {e}"))?;
        let summary = message.lines().next().unwrap_or("");

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

        if config
            .skip_scopes
            .contains(&scope.trim().to_ascii_lowercase())
        {
            continue;
        }

        if prefix.trim_end().ends_with('!')
            || has_breaking_change_footer(message)
            || config.major_types.contains(&typ.to_ascii_lowercase())
        {
            impact = Some(Impact::Major);
            break;
        }

        if config.minor_types.contains(&typ.to_ascii_lowercase()) {
            if impact.unwrap_or(Impact::Patch) < Impact::Minor {
                impact = Some(Impact::Minor);
            }
            continue;
        }

        if config.patch_types.contains(&typ.to_ascii_lowercase()) && impact.is_none() {
            impact = Some(Impact::Patch);
        }
    }

    Ok(impact)
}

fn commit_touches_package(
    repo: &Repository,
    commit: &git2::Commit<'_>,
    package_path: &Path,
    child_package_paths: &[PathBuf],
    ignored_directories: &[PathBuf],
) -> AppResult<bool> {
    let tree = commit
        .tree()
        .map_err(|e| format!("failed to read tree for commit {}: {e}", commit.id()))?;
    let parent_tree = if commit.parent_count() == 0 {
        None
    } else {
        Some(
            commit
                .parent(0)
                .and_then(|parent| parent.tree())
                .map_err(|e| {
                    format!("failed to read parent tree for commit {}: {e}", commit.id())
                })?,
        )
    };
    let diff = repo
        .diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), None)
        .map_err(|e| format!("failed to diff commit {}: {e}", commit.id()))?;

    Ok(diff.deltas().any(|delta| {
        [delta.old_file().path(), delta.new_file().path()]
            .into_iter()
            .flatten()
            .any(|path| {
                path_belongs_to_package(
                    path,
                    package_path,
                    child_package_paths,
                    ignored_directories,
                )
            })
    }))
}

fn path_belongs_to_package(
    path: &Path,
    package_path: &Path,
    child_package_paths: &[PathBuf],
    ignored_directories: &[PathBuf],
) -> bool {
    (package_path.as_os_str().is_empty() || path.starts_with(package_path))
        && !child_package_paths
            .iter()
            .any(|child| path.starts_with(child))
        && !is_ignored_path(path, ignored_directories)
}

fn has_breaking_change_footer(message: &str) -> bool {
    message.lines().any(|line| {
        let trimmed = line.trim_start();
        trimmed.starts_with("BREAKING CHANGE:") || trimmed.starts_with("BREAKING-CHANGE:")
    })
}

pub fn list_tracked_files_under(
    repo: &Repository,
    repo_root: &Path,
    directory: &Path,
    ignored_directories: &[PathBuf],
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
        if is_likely_vendored_path(relative)
            || is_ignored_path(relative, ignored_directories)
            || has_symlink_component(repo_root, relative)
        {
            continue;
        }

        if dir_relative.as_os_str().is_empty() || relative.starts_with(dir_relative) {
            files.push(repo_root.join(relative));
        }
    }

    files.sort();
    files.dedup();
    Ok(files)
}

pub(crate) fn is_ignored_path(relative: &Path, ignored_directories: &[PathBuf]) -> bool {
    ignored_directories
        .iter()
        .any(|directory| directory.as_os_str().is_empty() || relative.starts_with(directory))
}

fn is_likely_vendored_path(relative: &Path) -> bool {
    const VENDORED_DIRS: &[&str] = &["vendor", "node_modules"];

    relative.components().any(|component| {
        let Component::Normal(name) = component else {
            return false;
        };

        VENDORED_DIRS
            .iter()
            .any(|vendored| name == OsStr::new(vendored))
    })
}

fn has_symlink_component(repo_root: &Path, relative: &Path) -> bool {
    let mut path = repo_root.to_path_buf();
    for component in relative.components() {
        path.push(component.as_os_str());
        if fs::symlink_metadata(&path)
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(false)
        {
            return true;
        }
    }

    false
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
                entry.path().ok().map(PathBuf::from)
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

fn make_remote_callbacks(repo: &Repository) -> git2::RemoteCallbacks<'static> {
    let mut callbacks = git2::RemoteCallbacks::new();
    let config = repo.config().ok();
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
            if let Some((user, password)) = http_userpass_from_url(url) {
                return git2::Cred::userpass_plaintext(&user, &password);
            }
            if let Some(config) = &config
                && let Ok(cred) = git2::Cred::credential_helper(config, url, username)
            {
                return Ok(cred);
            }
            // GITHUB_TOKEN is the standard credential in CI. Limit it to GitHub
            // remotes so it does not shadow configured credentials elsewhere.
            if github_token_applies(url)
                && let Some(token) = github_token()
            {
                let user = username.unwrap_or("x-access-token");
                return git2::Cred::userpass_plaintext(user, &token);
            }
        }
        if remaining.contains(git2::CredentialType::USERNAME) {
            tried.set(tried.get() | git2::CredentialType::USERNAME);
            if let Some(user) = username_for_url(url, username) {
                return git2::Cred::username(&user);
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

fn username_for_url(url: &str, username: Option<&str>) -> Option<String> {
    username
        .map(ToOwned::to_owned)
        .or_else(|| http_username_from_url(url))
        .or_else(|| {
            if github_token_applies(url) && github_token().is_some() {
                Some("x-access-token".to_string())
            } else {
                None
            }
        })
        .or_else(|| (!is_http_url(url)).then(|| "git".to_string()))
}

fn github_token() -> Option<String> {
    std::env::var("GITHUB_TOKEN")
        .ok()
        .filter(|token| !token.trim().is_empty())
}

fn github_token_applies(url: &str) -> bool {
    let Some(host) = http_url_host(url) else {
        return false;
    };

    host.eq_ignore_ascii_case("github.com")
        || std::env::var("GITHUB_SERVER_URL")
            .ok()
            .and_then(|server_url| {
                http_url_host(&server_url).map(|server_host| host.eq_ignore_ascii_case(server_host))
            })
            .unwrap_or(false)
}

fn http_username_from_url(url: &str) -> Option<String> {
    let userinfo = http_url_userinfo(url)?;
    let user = userinfo.split(':').next().unwrap_or("");
    (!user.is_empty()).then(|| user.to_string())
}

fn http_userpass_from_url(url: &str) -> Option<(String, String)> {
    let userinfo = http_url_userinfo(url)?;
    let (user, password) = userinfo.split_once(':')?;
    (!user.is_empty()).then(|| (user.to_string(), password.to_string()))
}

fn http_url_userinfo(url: &str) -> Option<&str> {
    http_url_authority(url)?
        .rsplit_once('@')
        .map(|(userinfo, _)| userinfo)
}

fn http_url_host(url: &str) -> Option<&str> {
    let authority = http_url_authority(url)?;
    let host_port = authority.rsplit('@').next().unwrap_or(authority);
    let host = host_port.split(':').next().unwrap_or(host_port);
    (!host.is_empty()).then_some(host)
}

fn http_url_authority(url: &str) -> Option<&str> {
    let rest = strip_prefix_ignore_ascii_case(url, "https://")
        .or_else(|| strip_prefix_ignore_ascii_case(url, "http://"))?;
    Some(rest.split('/').next().unwrap_or(rest))
}

fn is_http_url(url: &str) -> bool {
    strip_prefix_ignore_ascii_case(url, "https://").is_some()
        || strip_prefix_ignore_ascii_case(url, "http://").is_some()
}

fn strip_prefix_ignore_ascii_case<'a>(value: &'a str, prefix: &str) -> Option<&'a str> {
    let (head, rest) = value.split_at_checked(prefix.len())?;
    head.eq_ignore_ascii_case(prefix).then_some(rest)
}

fn configured_http_extra_headers(repo: &Repository, url: Option<&str>) -> Vec<String> {
    let Some(url) = url else {
        return Vec::new();
    };
    let Ok(config) = repo.config() else {
        return Vec::new();
    };
    let Ok(mut entries) = config.entries(None) else {
        return Vec::new();
    };

    let mut headers = Vec::new();
    while let Some(entry) = entries.next() {
        let Ok(entry) = entry else {
            continue;
        };
        let Ok(name) = entry.name() else {
            continue;
        };
        let Ok(value) = entry.value() else {
            continue;
        };
        if http_extra_header_matches(name, url) {
            headers.push(value.to_string());
        }
    }
    headers
}

fn http_extra_header_matches(name: &str, url: &str) -> bool {
    if !is_http_url(url) {
        return false;
    }

    let normalized = name.to_ascii_lowercase();
    if normalized == "http.extraheader" {
        return true;
    }
    if !normalized.starts_with("http.") || !normalized.ends_with(".extraheader") {
        return false;
    }

    let prefix = &name["http.".len()..name.len() - ".extraheader".len()];
    http_url_matches_config_prefix(url, prefix)
}

fn http_url_matches_config_prefix(url: &str, prefix: &str) -> bool {
    let Some(candidate) = url.get(..prefix.len()) else {
        return false;
    };
    if prefix.is_empty() || !candidate.eq_ignore_ascii_case(prefix) {
        return false;
    }
    prefix.ends_with('/') || matches!(url.as_bytes().get(prefix.len()), None | Some(b'/'))
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
        .filter_map(Result::ok)
        .flatten()
        .map(str::to_string)
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
    let headers = configured_http_extra_headers(&repo, remote.url().ok());
    if !headers.is_empty() {
        let header_refs: Vec<&str> = headers.iter().map(String::as_str).collect();
        opts.custom_headers(&header_refs);
    }
    opts.remote_callbacks(make_remote_callbacks(&repo));
    opts.download_tags(git2::AutotagOption::All);
    remote
        .fetch(&[] as &[&str], Some(&mut opts), None)
        .map_err(|e| format!("failed to fetch from '{name}': {e}"))
}

pub fn git_push(repo: &Repository, branch: &str, tags: &[String]) -> AppResult<()> {
    let mut remote = repo
        .find_remote("origin")
        .map_err(|e| format!("failed to find remote 'origin': {e}"))?;
    let branch_ref = format!("refs/heads/{branch}:refs/heads/{branch}");
    let mut refspecs = Vec::with_capacity(tags.len() + 1);
    refspecs.push(branch_ref);
    refspecs.extend(
        tags.iter()
            .map(|tag| format!("refs/tags/{tag}:refs/tags/{tag}")),
    );
    let refspecs = refspecs.iter().map(String::as_str).collect::<Vec<_>>();

    let url = remote
        .pushurl()
        .ok()
        .flatten()
        .or_else(|| remote.url().ok());
    let headers = configured_http_extra_headers(repo, url);
    let callbacks = make_remote_callbacks(repo);
    let mut push_options = git2::PushOptions::new();
    if !headers.is_empty() {
        let header_refs: Vec<&str> = headers.iter().map(String::as_str).collect();
        push_options.custom_headers(&header_refs);
    }
    push_options.remote_callbacks(callbacks);

    remote
        .push(&refspecs, Some(&mut push_options))
        .map_err(|e| format!("failed to push to origin: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_path(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("bumper-{name}-{nanos}"))
    }

    fn add_index_paths(repo: &Repository, paths: &[&str]) {
        let mut index = repo.index().expect("open git index");
        for path in paths {
            index.add_path(Path::new(path)).expect("add index path");
        }
        index.write().expect("write git index");
    }

    fn commit_file(
        repo: &Repository,
        dir: &Path,
        name: &str,
        contents: &str,
        message: &str,
    ) -> Oid {
        if let Some(parent) = dir.join(name).parent() {
            std::fs::create_dir_all(parent).expect("create test file parent");
        }
        std::fs::write(dir.join(name), contents).expect("write test file");
        let mut index = repo.index().expect("open git index");
        index.add_path(Path::new(name)).expect("add index path");
        index.write().expect("write git index");
        let tree_oid = index.write_tree().expect("write tree");
        let tree = repo.find_tree(tree_oid).expect("find tree");
        let sig = git2::Signature::now("Bumper Test", "bumper@example.com").expect("signature");
        let parents = repo
            .head()
            .ok()
            .and_then(|head| head.peel_to_commit().ok())
            .into_iter()
            .collect::<Vec<_>>();
        let parent_refs = parents.iter().collect::<Vec<_>>();
        repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &parent_refs)
            .expect("commit")
    }

    fn lightweight_tag(repo: &Repository, name: &str, oid: Oid) {
        let commit = repo.find_commit(oid).expect("find commit");
        repo.tag_lightweight(name, commit.as_object(), false)
            .expect("create lightweight tag");
    }

    #[test]
    fn latest_tag_uses_nearest_reachable_semver_tag() {
        let dir = temp_path("latest-semver-tag");
        let repo = Repository::init(&dir).expect("init repo");
        let first = commit_file(&repo, &dir, "README.md", "one", "chore: init");
        lightweight_tag(&repo, "v0.1.0", first);
        let second = commit_file(&repo, &dir, "README.md", "two", "fix: bug");
        lightweight_tag(&repo, "deploy-2026-06-02", second);
        let _third = commit_file(&repo, &dir, "README.md", "three", "feat: feature");

        let (name, oid) = latest_tag_or_none(&repo, Path::new(""))
            .expect("read tags")
            .expect("find latest tag");

        assert_eq!(name, "v0.1.0");
        assert_eq!(oid, first);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn latest_tag_isolates_root_and_scoped_streams() {
        let dir = temp_path("isolated-tag-streams");
        let repo = Repository::init(&dir).expect("init repo");
        let root_commit = commit_file(&repo, &dir, "README.md", "one", "chore: init");
        lightweight_tag(&repo, "v1.0.0", root_commit);
        let scoped_commit = commit_file(&repo, &dir, "README.md", "two", "feat: api");
        lightweight_tag(&repo, "packages/api/v9.0.0", scoped_commit);
        commit_file(&repo, &dir, "README.md", "three", "fix: follow-up");

        let root = latest_tag_or_none(&repo, Path::new(""))
            .expect("read root tags")
            .expect("find root tag");
        let scoped = latest_tag_or_none(&repo, Path::new("packages/api"))
            .expect("read scoped tags")
            .expect("find scoped tag");

        assert_eq!(root, ("v1.0.0".to_string(), root_commit));
        assert_eq!(scoped, ("packages/api/v9.0.0".to_string(), scoped_commit));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn latest_tag_finds_multi_level_scoped_stream() {
        let dir = temp_path("multi-level-tag-stream");
        let repo = Repository::init(&dir).expect("init repo");
        let commit = commit_file(&repo, &dir, "README.md", "one", "chore: init");
        lightweight_tag(&repo, "packages/services/api/V2.3.4", commit);

        let tag = latest_tag_or_none(&repo, Path::new("packages/services/api"))
            .expect("read multi-level scoped tags")
            .expect("find multi-level scoped tag");

        assert_eq!(tag, ("packages/services/api/V2.3.4".to_string(), commit));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn latest_tag_ignores_malformed_and_sibling_scoped_tags() {
        let dir = temp_path("ignored-scoped-tags");
        let repo = Repository::init(&dir).expect("init repo");
        let matching_commit = commit_file(&repo, &dir, "README.md", "one", "chore: init");
        lightweight_tag(&repo, "packages/api/v1.0.0", matching_commit);
        let other_commit = commit_file(&repo, &dir, "README.md", "two", "feat: other");
        for tag in [
            "packages/api/2.0.0",
            "packages/api/v2.0",
            "packages/api/v2.0.0/extra",
            "packages/api-client/v9.0.0",
            "packages/web/v9.0.0",
            "v9.0.0",
        ] {
            lightweight_tag(&repo, tag, other_commit);
        }

        let tag = latest_tag_or_none(&repo, Path::new("packages/api"))
            .expect("read scoped tags")
            .expect("find scoped tag");
        let missing =
            latest_tag_or_none(&repo, Path::new("packages/missing")).expect("read missing stream");

        assert_eq!(tag, ("packages/api/v1.0.0".to_string(), matching_commit));
        assert_eq!(missing, None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn latest_tag_uses_nearest_scoped_commit_and_highest_version_on_it() {
        let dir = temp_path("nearest-scoped-tag");
        let repo = Repository::init(&dir).expect("init repo");
        let older = commit_file(&repo, &dir, "README.md", "one", "chore: init");
        lightweight_tag(&repo, "crates/widget/v9.0.0", older);
        let nearer = commit_file(&repo, &dir, "README.md", "two", "feat: widget");
        lightweight_tag(&repo, "crates/widget/v1.0.0", nearer);
        lightweight_tag(&repo, "crates/widget/V2.0.0", nearer);
        commit_file(&repo, &dir, "README.md", "three", "fix: widget");

        let tag = latest_tag_or_none(&repo, Path::new("crates/widget"))
            .expect("read scoped tags")
            .expect("find scoped tag");

        assert_eq!(tag, ("crates/widget/V2.0.0".to_string(), nearer));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn latest_tag_rejects_a_stream_with_only_unreachable_tags() {
        let dir = temp_path("unreachable-scoped-tag");
        let repo = Repository::init(&dir).expect("init repo");
        let baseline = commit_file(&repo, &dir, "README.md", "one", "chore: init");
        let tagged = commit_file(&repo, &dir, "README.md", "two", "fix: package");
        lightweight_tag(&repo, "packages/api/v1.0.0", tagged);
        let baseline = repo.find_object(baseline, None).expect("find baseline");
        repo.reset(&baseline, git2::ResetType::Hard, None)
            .expect("reset to baseline");

        let error = latest_tag_or_none(&repo, Path::new("packages/api"))
            .expect_err("unreachable stream should fail");

        assert!(error.contains("package stream 'packages/api'"));
        assert!(error.contains("reachable from HEAD"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn git_push_pushes_branch_and_all_exact_tags() {
        let dir = temp_path("push-multiple-tags");
        let remote_dir = temp_path("push-multiple-tags-remote");
        let repo = Repository::init(&dir).expect("init repo");
        let commit = commit_file(&repo, &dir, "README.md", "one", "chore: init");
        lightweight_tag(&repo, "packages/api/v1.0.0", commit);
        lightweight_tag(&repo, "packages/web/v2.0.0", commit);
        Repository::init_bare(&remote_dir).expect("init bare remote");
        repo.remote("origin", remote_dir.to_str().expect("UTF-8 remote path"))
            .expect("add origin");
        let branch = current_branch(&repo).expect("current branch");
        let tags = [
            "packages/api/v1.0.0".to_string(),
            "packages/web/v2.0.0".to_string(),
        ];

        git_push(&repo, &branch, &tags).expect("push refs");

        let remote = Repository::open_bare(&remote_dir).expect("open bare remote");
        assert_eq!(
            remote
                .find_reference(&format!("refs/heads/{branch}"))
                .expect("find remote branch")
                .target(),
            Some(commit)
        );
        for tag in tags {
            assert_eq!(
                remote
                    .find_reference(&format!("refs/tags/{tag}"))
                    .expect("find remote tag")
                    .target(),
                Some(commit)
            );
        }
        let _ = std::fs::remove_dir_all(dir);
        let _ = std::fs::remove_dir_all(remote_dir);
    }

    #[test]
    fn impact_detects_breaking_change_footer() {
        let dir = temp_path("breaking-footer");
        let repo = Repository::init(&dir).expect("init repo");
        let first = commit_file(&repo, &dir, "README.md", "one", "chore: init");
        lightweight_tag(&repo, "v0.1.0", first);
        commit_file(
            &repo,
            &dir,
            "README.md",
            "two",
            "feat: change API\n\nBREAKING CHANGE: response shape changed",
        );

        let impact = get_impact_for_package(
            &repo,
            Some(first),
            Path::new(""),
            &[],
            &ImpactConfig {
                major_types: &HashSet::from(["breaking change".to_string()]),
                minor_types: &HashSet::from(["feat".to_string()]),
                patch_types: &HashSet::from(["fix".to_string()]),
                skip_scopes: &HashSet::new(),
                ignored_directories: &[],
                force: false,
            },
        )
        .expect("get impact");

        assert_eq!(impact, Some(Impact::Major));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn impact_ignores_commits_that_only_touch_ignored_directories() {
        let dir = temp_path("ignored-directory-impact");
        let repo = Repository::init(&dir).expect("init repo");
        let baseline = commit_file(&repo, &dir, "README.md", "one", "chore: init");
        commit_file(
            &repo,
            &dir,
            "generated/output.txt",
            "generated",
            "feat: regenerate output",
        );

        let impact = get_impact_for_package(
            &repo,
            Some(baseline),
            Path::new(""),
            &[],
            &ImpactConfig {
                major_types: &HashSet::from(["breaking change".to_string()]),
                minor_types: &HashSet::from(["feat".to_string()]),
                patch_types: &HashSet::from(["fix".to_string()]),
                skip_scopes: &HashSet::new(),
                ignored_directories: &[PathBuf::from("generated")],
                force: false,
            },
        )
        .expect("get impact");

        assert_eq!(impact, None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn impact_is_scoped_to_package_owned_paths() {
        let dir = temp_path("package-impact");
        let repo = Repository::init(&dir).expect("init repo");
        commit_file(&repo, &dir, "README.md", "one", "chore: init");
        commit_file(
            &repo,
            &dir,
            "packages/api/package.json",
            r#"{"name":"api","version":"1.0.0"}"#,
            "chore: add package",
        );
        let baseline = repo.head().unwrap().target().unwrap();
        commit_file(&repo, &dir, "README.md", "two", "feat: root feature");
        commit_file(
            &repo,
            &dir,
            "packages/api/package.json",
            r#"{"name":"api","version":"1.0.1"}"#,
            "fix: api bug",
        );
        let major = HashSet::from(["breaking change".to_string()]);
        let minor = HashSet::from(["feat".to_string()]);
        let patch = HashSet::from(["fix".to_string()]);
        let skipped = HashSet::new();
        let config = ImpactConfig {
            major_types: &major,
            minor_types: &minor,
            patch_types: &patch,
            skip_scopes: &skipped,
            ignored_directories: &[],
            force: false,
        };

        let root = get_impact_for_package(
            &repo,
            Some(baseline),
            Path::new(""),
            &[PathBuf::from("packages/api")],
            &config,
        )
        .expect("root impact");
        let api = get_impact_for_package(
            &repo,
            Some(baseline),
            Path::new("packages/api"),
            &[],
            &config,
        )
        .expect("api impact");

        assert_eq!(root, Some(Impact::Minor));
        assert_eq!(api, Some(Impact::Patch));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn tracked_files_under_skips_likely_vendored_paths() {
        let dir = temp_path("vendored-paths");
        let repo = Repository::init(&dir).expect("init repo");
        std::fs::write(dir.join("README.md"), "version 0.13.0").expect("write README.md");
        std::fs::create_dir_all(dir.join("cmd/vendor")).expect("create vendor dir");
        std::fs::write(dir.join("cmd/vendor/README.md"), "version 0.13.0")
            .expect("write vendored README.md");
        std::fs::create_dir_all(dir.join("node_modules/pkg")).expect("create node_modules dir");
        std::fs::write(
            dir.join("node_modules/pkg/package.json"),
            r#"{"version":"0.13.0"}"#,
        )
        .expect("write vendored package.json");
        add_index_paths(
            &repo,
            &[
                "README.md",
                "cmd/vendor/README.md",
                "node_modules/pkg/package.json",
            ],
        );

        let files = list_tracked_files_under(&repo, &dir, &dir, &[]).expect("list tracked files");

        assert!(files.contains(&dir.join("README.md")));
        assert!(!files.contains(&dir.join("cmd/vendor/README.md")));
        assert!(!files.contains(&dir.join("node_modules/pkg/package.json")));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn tracked_files_under_skips_configured_directories_by_component() {
        let dir = temp_path("configured-ignored-paths");
        let repo = Repository::init(&dir).expect("init repo");
        std::fs::create_dir_all(dir.join("build")).expect("create build dir");
        std::fs::create_dir_all(dir.join("builder")).expect("create builder dir");
        std::fs::write(dir.join("build/README.md"), "ignored").expect("write ignored file");
        std::fs::write(dir.join("builder/README.md"), "included").expect("write included file");
        add_index_paths(&repo, &["build/README.md", "builder/README.md"]);

        let files = list_tracked_files_under(&repo, &dir, &dir, &[PathBuf::from("build")])
            .expect("list tracked files");

        assert!(!files.contains(&dir.join("build/README.md")));
        assert!(files.contains(&dir.join("builder/README.md")));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn tracked_files_under_skips_symlinks() {
        use std::os::unix::fs::symlink;

        let dir = temp_path("symlink-paths");
        let repo = Repository::init(&dir).expect("init repo");
        std::fs::write(dir.join("real-readme.md"), "version 0.13.0").expect("write real file");
        symlink("real-readme.md", dir.join("README.md")).expect("create symlink");
        add_index_paths(&repo, &["real-readme.md", "README.md"]);

        let files = list_tracked_files_under(&repo, &dir, &dir, &[]).expect("list tracked files");

        assert!(files.contains(&dir.join("real-readme.md")));
        assert!(!files.contains(&dir.join("README.md")));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn http_extra_header_matches_actions_checkout_config() {
        assert!(http_extra_header_matches(
            "http.https://trev.zip/.extraheader",
            "https://trev.zip/llc/bumper"
        ));
        assert!(http_extra_header_matches(
            "http.extraheader",
            "https://trev.zip/llc/bumper"
        ));
        assert!(!http_extra_header_matches(
            "http.https://gitlab.com/.extraheader",
            "https://trev.zip/llc/bumper"
        ));
        assert!(!http_extra_header_matches(
            "http.https://trev.zip.extraheader",
            "https://trev.zip.evil/llc/bumper"
        ));
        assert!(!http_extra_header_matches(
            "http.https://trev.zip/.extraheader",
            "git@trev.zip:llc/bumper.git"
        ));
    }

    #[test]
    fn configured_http_extra_headers_reads_matching_repo_config() {
        let dir = temp_path("http-extra-headers");
        let repo = Repository::init(&dir).expect("init repo");
        let mut config = repo.config().expect("open repo config");
        config
            .set_str(
                "http.https://trev.zip/.extraheader",
                "AUTHORIZATION: basic abc",
            )
            .expect("set matching header");
        config
            .set_str(
                "http.https://gitlab.com/.extraheader",
                "AUTHORIZATION: basic def",
            )
            .expect("set non-matching header");

        let headers = configured_http_extra_headers(&repo, Some("https://trev.zip/llc/bumper.git"));

        assert!(headers.contains(&"AUTHORIZATION: basic abc".to_string()));
        assert!(!headers.contains(&"AUTHORIZATION: basic def".to_string()));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn http_credentials_can_be_read_from_url() {
        assert_eq!(
            http_userpass_from_url("https://alice:secret@example.com/repo.git"),
            Some(("alice".to_string(), "secret".to_string()))
        );
        assert_eq!(
            http_username_from_url("https://alice@example.com/repo.git"),
            Some("alice".to_string())
        );
        assert_eq!(http_userpass_from_url("git@example.com:repo.git"), None);
    }
}

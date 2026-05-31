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
        .map_err(|e| format!("failed to read branch name: {e}"))?;
    Ok(shorthand.to_string())
}

pub fn latest_tag(repo: &Repository) -> AppResult<(String, Oid)> {
    let tags = repo
        .tag_names(None)
        .map_err(|e| format!("failed to list tags: {e}"))?;

    let mut latest: Option<(String, Oid, i64)> = None;

    for maybe_name in tags.iter() {
        let Ok(Some(name)) = maybe_name else {
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

        let Some(summary) = commit
            .summary()
            .map_err(|e| format!("failed to read commit {oid} summary: {e}"))?
        else {
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
        if dir_relative.as_os_str().is_empty() || relative.starts_with(dir_relative) {
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

pub fn git_push(repo: &Repository, branch: &str, tag: &str) -> AppResult<()> {
    let mut remote = repo
        .find_remote("origin")
        .map_err(|e| format!("failed to find remote 'origin': {e}"))?;
    let branch_ref = format!("refs/heads/{branch}:refs/heads/{branch}");
    let tag_ref = format!("refs/tags/{tag}:refs/tags/{tag}");

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
        .push(
            &[branch_ref.as_str(), tag_ref.as_str()],
            Some(&mut push_options),
        )
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

    #[test]
    fn http_extra_header_matches_actions_checkout_config() {
        assert!(http_extra_header_matches(
            "http.https://github.com/.extraheader",
            "https://github.com/spotdemo4/bumper"
        ));
        assert!(http_extra_header_matches(
            "http.extraheader",
            "https://github.com/spotdemo4/bumper"
        ));
        assert!(!http_extra_header_matches(
            "http.https://gitlab.com/.extraheader",
            "https://github.com/spotdemo4/bumper"
        ));
        assert!(!http_extra_header_matches(
            "http.https://github.com.extraheader",
            "https://github.com.evil/spotdemo4/bumper"
        ));
        assert!(!http_extra_header_matches(
            "http.https://github.com/.extraheader",
            "git@github.com:spotdemo4/bumper.git"
        ));
    }

    #[test]
    fn configured_http_extra_headers_reads_matching_repo_config() {
        let dir = temp_path("http-extra-headers");
        let repo = Repository::init(&dir).expect("init repo");
        let mut config = repo.config().expect("open repo config");
        config
            .set_str(
                "http.https://github.com/.extraheader",
                "AUTHORIZATION: basic abc",
            )
            .expect("set matching header");
        config
            .set_str(
                "http.https://gitlab.com/.extraheader",
                "AUTHORIZATION: basic def",
            )
            .expect("set non-matching header");

        let headers =
            configured_http_extra_headers(&repo, Some("https://github.com/spotdemo4/bumper.git"));

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

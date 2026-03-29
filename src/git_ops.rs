use git2::{Oid, Repository, StatusOptions};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

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
        if relative.starts_with(dir_relative) {
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

pub fn run_git_command(repo_root: &Path, args: &[&str]) -> AppResult<()> {
    let status = Command::new("git")
        .current_dir(repo_root)
        .args(args)
        .status()
        .map_err(|e| format!("failed to run git {}: {e}", args.join(" ")))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("git {} failed", args.join(" ")))
    }
}

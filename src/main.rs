mod config;
mod git_ops;
mod model;
mod versioning;

use bumper::bump::apply_typed_change;
use git2::Repository;
use std::fs;
use std::path::Path;
use std::process::ExitCode;

use config::load_config;
use git_ops::{
    current_branch, ensure_clean_repo, get_impact, git_commit, git_fetch, git_push, git_tag,
    latest_tag, list_tracked_files_under, repo_root, stage_path, staged_files,
};
use model::AppResult;
use versioning::next_version;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("{err}");
            ExitCode::from(1)
        }
    }
}

fn run() -> AppResult<()> {
    let mut config = load_config();
    let repo = Repository::discover(".").map_err(|e| format!("not a git repository: {e}"))?;
    let repo_root = repo_root(&repo)?;

    if config.paths.is_empty() || config.ci {
        config.paths = vec![repo_root.clone()];
    }

    if !config.allow_dirty {
        ensure_clean_repo(&repo)?;
    }

    git_fetch(&repo)?;

    let branch = current_branch(&repo)?;
    let (last_tag_name, last_tag_commit) = latest_tag(&repo)?;
    let last_version = last_tag_name.trim_start_matches('v').to_string();

    let impact = get_impact(
        &repo,
        last_tag_commit,
        &config.major_types,
        &config.minor_types,
        &config.patch_types,
        &config.skip_scopes,
        config.force,
    )?;

    let Some(impact) = impact else {
        println!("no new impactful commits since last tag (v{last_version})");
        return Ok(());
    };

    println!("impact: {}", impact.as_str());
    let next_version = next_version(&last_version, impact)?;
    println!("v{last_version} -> v{next_version}");

    for path in &config.paths {
        let absolute = if path.is_absolute() {
            path.clone()
        } else {
            repo_root.join(path)
        };

        if absolute.is_file() {
            bump_file(&repo, &repo_root, &absolute, &last_version, &next_version)?;
        } else if absolute.is_dir() {
            bump_dir(&repo, &repo_root, &absolute, &last_version, &next_version)?;
        } else {
            eprintln!(
                "warning: file or directory not found: {}",
                absolute.display()
            );
        }
    }

    if !config.commit {
        println!("skipping commit");
    } else if staged_files(&repo)?.is_empty() {
        println!("no changes to commit");
    } else {
        let message = format!("bump: v{last_version} -> v{next_version}");
        git_commit(&repo, &message)?;
    }

    if !config.tag {
        println!("skipping tag");
    } else {
        let message = format!("bump: v{last_version} -> v{next_version}");
        let tag = format!("v{next_version}");
        git_tag(&repo, &tag, &message)?;
    }

    if !config.push {
        println!("skipping push");
    } else {
        let tag = format!("v{next_version}");
        git_push(&repo, &branch, &tag)?;
    }

    Ok(())
}

fn bump_dir(
    repo: &Repository,
    repo_root: &Path,
    directory: &Path,
    old_version: &str,
    new_version: &str,
) -> AppResult<()> {
    let files = list_tracked_files_under(repo, repo_root, directory)?;
    for absolute in files {
        if !absolute.is_file() {
            continue;
        }
        let _ = bump_typed_file(repo, repo_root, &absolute, old_version, new_version)?;
    }
    Ok(())
}

fn bump_file(
    repo: &Repository,
    repo_root: &Path,
    file: &Path,
    old_version: &str,
    new_version: &str,
) -> AppResult<()> {
    if bump_typed_file(repo, repo_root, file, old_version, new_version)? {
        return Ok(());
    }
    let source = fs::read_to_string(file)
        .map_err(|e| format!("failed to read '{}': {e}", file.display()))?;
    if !source.contains(old_version) {
        return Err(format!("no occurrences found in {}", file.display()));
    }
    let replaced = source.replace(old_version, new_version);
    if replaced == source {
        return Err(format!("failed to replace version in {}", file.display()));
    }
    fs::write(file, replaced).map_err(|e| format!("failed to write '{}': {e}", file.display()))?;
    stage_path(repo, repo_root, file)
}

fn bump_typed_file(
    repo: &Repository,
    repo_root: &Path,
    file: &Path,
    old_version: &str,
    new_version: &str,
) -> AppResult<bool> {
    let changed = apply_typed_change(file, old_version, new_version)?;
    if changed {
        stage_path(repo, repo_root, file)?;
    }
    Ok(changed)
}

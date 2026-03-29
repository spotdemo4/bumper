mod bump;
mod config;
mod git_ops;
mod model;
mod versioning;

use git2::Repository;
use std::process::ExitCode;

use bump::{bump_dir, bump_file};
use config::load_config;
use git_ops::{
    current_branch, ensure_clean_repo, get_impact, latest_tag, repo_root, run_git_command,
    staged_files,
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

    run_git_command(&repo_root, &["fetch", "--all", "--tags", "--quiet"])?;

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
        run_git_command(&repo_root, &["commit", "-m", &message])?;
    }

    if !config.tag {
        println!("skipping tag");
    } else {
        let message = format!("bump: v{last_version} -> v{next_version}");
        let tag = format!("v{next_version}");
        run_git_command(&repo_root, &["tag", "-a", &tag, "-m", &message])?;
    }

    if !config.push {
        println!("skipping push");
    } else {
        let tag = format!("v{next_version}");
        run_git_command(&repo_root, &["push", "--atomic", "origin", &branch, &tag])?;
    }

    Ok(())
}

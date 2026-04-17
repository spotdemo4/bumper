use std::collections::HashSet;
use std::env;
use std::path::PathBuf;

use clap::Parser;

use crate::model::Config;

#[derive(Parser)]
#[command(
    name = "bumper",
    about = "Minimal CLI for version bumps based on conventional commits",
    version
)]
struct Cli {
    /// Paths to check for version files (defaults to repo root) [env: PATHS]
    #[arg(value_name = "PATH")]
    paths: Vec<PathBuf>,

    /// Commit types that trigger a major version bump [env: MAJOR_TYPES] [default: "BREAKING CHANGE"]
    #[arg(long, value_delimiter = ',', value_name = "TYPE")]
    major_types: Vec<String>,

    /// Commit types that trigger a minor version bump [env: MINOR_TYPES] [default: feat]
    #[arg(long, value_delimiter = ',', value_name = "TYPE")]
    minor_types: Vec<String>,

    /// Commit types that trigger a patch version bump [env: PATCH_TYPES] [default: fix]
    #[arg(long, value_delimiter = ',', value_name = "TYPE")]
    patch_types: Vec<String>,

    /// Commit scopes to skip when determining version bump [env: SKIP_SCOPES] [default: ci]
    #[arg(long, value_delimiter = ',', value_name = "SCOPE")]
    skip_scopes: Vec<String>,

    /// Create a commit for the version bump [env: COMMIT]
    #[arg(long, overrides_with = "no_commit")]
    commit: bool,

    /// Skip creating a commit
    #[arg(long, overrides_with = "commit")]
    no_commit: bool,

    /// Create a git tag for the version bump [env: TAG]
    #[arg(long, overrides_with = "no_tag")]
    tag: bool,

    /// Skip creating a git tag
    #[arg(long, overrides_with = "tag")]
    no_tag: bool,

    /// Push commits and tags to remote [env: PUSH]
    #[arg(long, overrides_with = "no_push")]
    push: bool,

    /// Skip pushing to remote
    #[arg(long, overrides_with = "push")]
    no_push: bool,

    /// Force a version bump even without relevant commits [env: FORCE]
    #[arg(long)]
    force: bool,

    /// Allow bumping in a dirty working tree [env: ALLOW_DIRTY]
    #[arg(long)]
    allow_dirty: bool,
}

pub fn load_config() -> Config {
    let cli = Cli::parse();

    let mut paths = parse_list_env("PATHS");
    paths.extend(cli.paths);

    let major_types = resolve_set(cli.major_types, "MAJOR_TYPES", &["BREAKING CHANGE"]);
    let minor_types = resolve_set(cli.minor_types, "MINOR_TYPES", &["feat"]);
    let patch_types = resolve_set(cli.patch_types, "PATCH_TYPES", &["fix"]);
    let skip_scopes = resolve_set(cli.skip_scopes, "SKIP_SCOPES", &["ci"]);

    let commit = resolve_bool(cli.commit, cli.no_commit, "COMMIT", true);
    let tag = resolve_bool(cli.tag, cli.no_tag, "TAG", true);
    let push = resolve_bool(cli.push, cli.no_push, "PUSH", true);

    Config {
        paths,
        major_types,
        minor_types,
        patch_types,
        skip_scopes,
        commit,
        tag,
        push,
        force: cli.force || parse_bool_env("FORCE", false),
        allow_dirty: cli.allow_dirty || parse_bool_env("ALLOW_DIRTY", false),
    }
}

/// Resolves a list-type config field: CLI args take precedence over env var, falling back to defaults.
fn resolve_set(cli_values: Vec<String>, env_name: &str, defaults: &[&str]) -> HashSet<String> {
    if !cli_values.is_empty() {
        return cli_values
            .into_iter()
            .map(|s| s.to_ascii_lowercase())
            .collect();
    }
    parse_lower_set_or_default(env_name, defaults)
}

/// Resolves a boolean config field: explicit CLI flag beats env var, which beats the default.
fn resolve_bool(flag: bool, no_flag: bool, env_name: &str, default: bool) -> bool {
    if no_flag {
        false
    } else if flag {
        true
    } else {
        parse_bool_env(env_name, default)
    }
}

fn parse_bool_env(name: &str, default: bool) -> bool {
    match env::var(name) {
        Ok(value) => matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        Err(_) => default,
    }
}

fn parse_list_env(name: &str) -> Vec<PathBuf> {
    match env::var(name) {
        Ok(raw) => split_list(&raw).into_iter().map(PathBuf::from).collect(),
        Err(_) => Vec::new(),
    }
}

fn parse_lower_set_or_default(name: &str, defaults: &[&str]) -> HashSet<String> {
    match env::var(name) {
        Ok(raw) => {
            let parsed: Vec<String> = split_list(&raw)
                .into_iter()
                .map(|item| item.to_ascii_lowercase())
                .collect();

            if parsed.is_empty() {
                defaults.iter().map(|s| s.to_ascii_lowercase()).collect()
            } else {
                parsed.into_iter().collect()
            }
        }
        Err(_) => defaults.iter().map(|s| s.to_ascii_lowercase()).collect(),
    }
}

fn split_list(value: &str) -> Vec<String> {
    if value.contains('\n') {
        value
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(ToOwned::to_owned)
            .collect()
    } else {
        value
            .split_whitespace()
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(ToOwned::to_owned)
            .collect()
    }
}

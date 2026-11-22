use std::collections::HashSet;
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};
use toml_edit::{DocumentMut, Item};

use crate::model::{AppResult, Config, GroupConfig, Impact};

// doc comments are the help text, and paired flags are bools
#[allow(clippy::doc_markdown, clippy::struct_excessive_bools)]
#[derive(Debug, Parser)]
#[command(
    name = "bumper",
    about = "Minimal CLI for version bumps based on conventional commits",
    version,
    args_conflicts_with_subcommands = true,
    disable_help_subcommand = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

    /// Additional files or directories to bump [env: PATHS]
    #[arg(value_name = "PATH")]
    paths: Vec<PathBuf>,

    /// Repository-relative directories to ignore [env: IGNORE_DIRECTORIES]
    #[arg(long, value_delimiter = ',', value_name = "PATH")]
    ignore_directories: Vec<PathBuf>,

    /// Commit types that trigger a major version bump [env: MAJOR_TYPES] [default: none]
    #[arg(long, value_delimiter = ',', value_name = "TYPE")]
    major_types: Vec<String>,

    /// Commit types that trigger a minor version bump [env: MINOR_TYPES] [default: feat]
    #[arg(long, value_delimiter = ',', value_name = "TYPE")]
    minor_types: Vec<String>,

    /// Commit types that trigger a patch version bump [env: PATCH_TYPES] [default: fix]
    #[arg(long, value_delimiter = ',', value_name = "TYPE")]
    patch_types: Vec<String>,

    /// Commit scopes to skip when determining version bump [env: SKIP_SCOPES] [default: ci,docs]
    #[arg(long, value_delimiter = ',', value_name = "SCOPE")]
    skip_scopes: Vec<String>,

    /// Bump the major version for a `!` after the commit type or a BREAKING CHANGE footer [env: BREAKING_CHANGES]
    #[arg(long, overrides_with = "no_breaking_changes")]
    breaking_changes: bool,

    /// Ignore `!` and BREAKING CHANGE footers when determining version bump
    #[arg(long, overrides_with = "breaking_changes")]
    no_breaking_changes: bool,

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

    /// Force every discovered package to bump without relevant commits [env: FORCE]
    #[arg(long)]
    force: bool,

    /// Force a specific package directory to bump without relevant commits [env: FORCE_PACKAGES]
    #[arg(long, value_delimiter = ',', value_name = "PATH")]
    force_package: Vec<PathBuf>,

    /// Bump type for packages with impactful commits, overriding the type the commits imply [env: BUMP_TYPE]
    #[arg(long, value_name = "TYPE")]
    bump_type: Option<Impact>,

    /// Version bump type used by --force [env: FORCE_BUMP_TYPE] [default: patch]
    #[arg(long, value_name = "TYPE")]
    force_bump_type: Option<Impact>,

    /// Give every package the same version, bumping all of them when any releases [env: FIXED_VERSION]
    #[arg(long, overrides_with = "no_fixed_version")]
    fixed_version: bool,

    /// Version packages independently without asking about a fixed version
    #[arg(long, overrides_with = "fixed_version")]
    no_fixed_version: bool,

    /// Allow bumping in a dirty working tree [env: ALLOW_DIRTY]
    #[arg(long)]
    allow_dirty: bool,

    /// Choose which packages to force from an interactive list
    #[arg(short, long)]
    interactive: bool,
}

/// The name of the repository configuration file, at the repository root.
pub const CONFIG_FILE: &str = "bumper.toml";

/// Parses the command line and validates the bump type environment variables, before the
/// repository is opened so `--help` and invalid values work anywhere.
pub fn parse_cli() -> AppResult<Cli> {
    let mut cli = Cli::parse();
    if cli.command.is_some() {
        return Ok(cli);
    }
    cli.bump_type = resolve_bump_type(cli.bump_type, "BUMP_TYPE")?;
    cli.force_bump_type = resolve_bump_type(cli.force_bump_type, "FORCE_BUMP_TYPE")?;
    Ok(cli)
}

/// Combines the command line, environment and the repository's `bumper.toml`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Subcommand)]
pub enum Command {
    /// Create a bumper.toml at the repository root
    Init,
}

pub fn load_config(cli: Cli, repo_root: &Path) -> AppResult<Config> {
    let path = repo_root.join(CONFIG_FILE);
    let file = match fs::read_to_string(&path) {
        Ok(source) => parse_file_config(&source).map_err(|e| format!("{CONFIG_FILE}: {e}"))?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => FileConfig::default(),
        Err(e) => return Err(format!("failed to read '{}': {e}", path.display())),
    };
    resolve_config(cli, file)
}

/// Settings from the command line beat the environment, which beats `bumper.toml`, which
/// beats the defaults. Paths and ignored directories from every source add up.
fn resolve_config(cli: Cli, file: FileConfig) -> AppResult<Config> {
    let mut paths = file.paths;
    paths.extend(parse_list_env("PATHS"));
    paths.extend(cli.paths);
    let mut ignored_directories = file.ignore_directories;
    ignored_directories.extend(parse_list_env("IGNORE_DIRECTORIES"));
    ignored_directories.extend(cli.ignore_directories);
    let mut force_packages = parse_list_env("FORCE_PACKAGES");
    force_packages.extend(cli.force_package);

    let major_types = resolve_set(cli.major_types, "MAJOR_TYPES", file.major_types, &[]);
    let minor_types = resolve_set(cli.minor_types, "MINOR_TYPES", file.minor_types, &["feat"]);
    let patch_types = resolve_set(cli.patch_types, "PATCH_TYPES", file.patch_types, &["fix"]);
    let skip_scopes = resolve_set(
        cli.skip_scopes,
        "SKIP_SCOPES",
        file.skip_scopes,
        &["ci", "docs"],
    );

    let commit = resolve_bool(cli.commit, cli.no_commit, "COMMIT", true);
    let tag = resolve_bool(cli.tag, cli.no_tag, "TAG", true);
    let push = resolve_bool(cli.push, cli.no_push, "PUSH", true);

    Ok(Config {
        paths,
        ignored_directories,
        major_types,
        minor_types,
        patch_types,
        skip_scopes,
        breaking_changes: resolve_optional_bool(
            cli.breaking_changes,
            cli.no_breaking_changes,
            "BREAKING_CHANGES",
        )
        .or(file.breaking_changes)
        .unwrap_or(true),
        commit,
        tag,
        push,
        force: cli.force || parse_bool_env("FORCE", false),
        force_packages,
        bump_type: resolve_bump_type(cli.bump_type, "BUMP_TYPE")?,
        force_bump_type: resolve_bump_type(cli.force_bump_type, "FORCE_BUMP_TYPE")?
            .unwrap_or(Impact::Patch),
        fixed_version: resolve_optional_bool(
            cli.fixed_version,
            cli.no_fixed_version,
            "FIXED_VERSION",
        )
        .or(file.fixed_version),
        groups: file.groups,
        allow_dirty: cli.allow_dirty || parse_bool_env("ALLOW_DIRTY", false),
        interactive: cli.interactive,
    })
}

/// The settings of a `bumper.toml`.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct FileConfig {
    pub paths: Vec<PathBuf>,
    pub ignore_directories: Vec<PathBuf>,
    pub major_types: Option<Vec<String>>,
    pub minor_types: Option<Vec<String>>,
    pub patch_types: Option<Vec<String>>,
    pub skip_scopes: Option<Vec<String>>,
    pub breaking_changes: Option<bool>,
    pub fixed_version: Option<bool>,
    pub groups: Vec<GroupConfig>,
}

pub fn parse_file_config(source: &str) -> AppResult<FileConfig> {
    let document = source
        .parse::<DocumentMut>()
        .map_err(|e| format!("invalid TOML: {e}"))?;
    let mut config = FileConfig::default();
    for (key, item) in document.as_table() {
        match key {
            "paths" => {
                config.paths = string_array(key, item)?
                    .into_iter()
                    .map(PathBuf::from)
                    .collect();
            }
            "ignore-directories" => {
                config.ignore_directories = string_array(key, item)?
                    .into_iter()
                    .map(PathBuf::from)
                    .collect();
            }
            "major-types" => config.major_types = Some(string_array(key, item)?),
            "minor-types" => config.minor_types = Some(string_array(key, item)?),
            "patch-types" => config.patch_types = Some(string_array(key, item)?),
            "skip-scopes" => config.skip_scopes = Some(string_array(key, item)?),
            "breaking-changes" => config.breaking_changes = Some(bool_value(key, item)?),
            "fixed-version" => config.fixed_version = Some(bool_value(key, item)?),
            "groups" => config.groups = parse_groups(item)?,
            _ => return Err(format!("unknown key '{key}'")),
        }
    }
    Ok(config)
}

fn parse_groups(item: &Item) -> AppResult<Vec<GroupConfig>> {
    let table = item
        .as_table_like()
        .ok_or_else(|| "'groups' must be a table of groups".to_string())?;
    let mut groups = Vec::new();
    for (name, group) in table.iter() {
        let group = group
            .as_table_like()
            .ok_or_else(|| format!("group '{name}' must be a table"))?;
        let mut packages = None;
        for (key, item) in group.iter() {
            match key {
                "packages" => packages = Some(string_array(&format!("groups.{name}.{key}"), item)?),
                _ => return Err(format!("unknown key '{key}' in group '{name}'")),
            }
        }
        let packages = packages
            .filter(|packages| !packages.is_empty())
            .ok_or_else(|| format!("group '{name}' must list its packages"))?;
        groups.push(GroupConfig {
            name: name.to_string(),
            packages,
        });
    }
    Ok(groups)
}

fn bool_value(key: &str, item: &Item) -> AppResult<bool> {
    item.as_bool()
        .ok_or_else(|| format!("'{key}' must be a boolean"))
}

fn string_array(key: &str, item: &Item) -> AppResult<Vec<String>> {
    let array = item
        .as_array()
        .ok_or_else(|| format!("'{key}' must be an array of strings"))?;
    array
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(ToOwned::to_owned)
                .ok_or_else(|| format!("'{key}' must be an array of strings"))
        })
        .collect()
}

/// Resolves a bump type config field: CLI arg beats env var, returning `None` when neither was set.
fn resolve_bump_type(cli_value: Option<Impact>, env_name: &str) -> AppResult<Option<Impact>> {
    if cli_value.is_some() {
        return Ok(cli_value);
    }

    match env::var(env_name) {
        Ok(value) if value.trim().is_empty() => Ok(None),
        Ok(value) => value
            .parse()
            .map(Some)
            .map_err(|error| format!("invalid {env_name}: {error}")),
        Err(_) => Ok(None),
    }
}

/// Resolves a list-type config field: CLI args beat the env var, which beats the config file,
/// falling back to defaults. Only the config file can set an empty list.
fn resolve_set(
    cli_values: Vec<String>,
    env_name: &str,
    file_values: Option<Vec<String>>,
    defaults: &[&str],
) -> HashSet<String> {
    let lower = |values: Vec<String>| {
        values
            .into_iter()
            .map(|s| s.trim().to_ascii_lowercase())
            .filter(|s| !s.is_empty())
            .collect()
    };
    if !cli_values.is_empty() {
        return lower(cli_values);
    }
    if let Some(values) = parse_lower_set_env(env_name) {
        return values;
    }
    match file_values {
        Some(values) => lower(values),
        None => defaults.iter().map(|s| s.to_ascii_lowercase()).collect(),
    }
}

/// Resolves a boolean config field: explicit CLI flag beats env var, which beats the default.
fn resolve_bool(flag: bool, no_flag: bool, env_name: &str, default: bool) -> bool {
    resolve_optional_bool(flag, no_flag, env_name).unwrap_or(default)
}

/// Resolves a boolean config field like `resolve_bool`, returning `None` when nothing was set.
fn resolve_optional_bool(flag: bool, no_flag: bool, env_name: &str) -> Option<bool> {
    if no_flag {
        Some(false)
    } else if flag {
        Some(true)
    } else {
        parse_optional_bool_env(env_name)
    }
}

fn parse_bool_env(name: &str, default: bool) -> bool {
    parse_optional_bool_env(name).unwrap_or(default)
}

fn parse_optional_bool_env(name: &str) -> Option<bool> {
    match env::var(name) {
        Ok(value) if value.trim().is_empty() => None,
        Ok(value) => Some(matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )),
        Err(_) => None,
    }
}

fn parse_list_env(name: &str) -> Vec<PathBuf> {
    match env::var(name) {
        Ok(raw) => split_path_list(&raw)
            .into_iter()
            .map(PathBuf::from)
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// The lowercase items of a list env var, or `None` when it is unset or empty.
fn parse_lower_set_env(name: &str) -> Option<HashSet<String>> {
    let raw = env::var(name).ok()?;
    let parsed = split_config_list(&raw)
        .into_iter()
        .map(|item| item.trim().to_ascii_lowercase())
        .filter(|item| !item.is_empty())
        .collect::<HashSet<_>>();
    (!parsed.is_empty()).then_some(parsed)
}

fn split_path_list(value: &str) -> Vec<String> {
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

fn split_config_list(value: &str) -> Vec<String> {
    if value.contains('\n') {
        return value
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(ToOwned::to_owned)
            .collect();
    }

    if value.contains(',') {
        return value
            .split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(ToOwned::to_owned)
            .collect();
    }

    let trimmed = value.trim();
    if trimmed.is_empty() {
        Vec::new()
    } else {
        vec![trimmed.to_string()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn config_lists_preserve_single_values_with_spaces() {
        assert_eq!(
            split_config_list("BREAKING CHANGE"),
            vec!["BREAKING CHANGE".to_string()]
        );
        assert_eq!(
            split_config_list("feat,fix"),
            vec!["feat".to_string(), "fix".to_string()]
        );
        assert_eq!(
            split_config_list("feat\nfix"),
            vec!["feat".to_string(), "fix".to_string()]
        );
    }

    #[test]
    fn parses_ignored_directories_option() {
        let cli = Cli::try_parse_from([
            "bumper",
            "--ignore-directories",
            "generated,packages/legacy",
        ])
        .expect("parse CLI");

        assert_eq!(
            cli.ignore_directories,
            vec![PathBuf::from("generated"), PathBuf::from("packages/legacy")]
        );
    }

    #[test]
    fn parses_repeated_and_delimited_force_package_options() {
        let cli = Cli::try_parse_from([
            "bumper",
            "--force-package",
            "packages/app,packages/lib",
            "--force-package",
            ".",
            "README.md",
        ])
        .expect("parse CLI");

        assert_eq!(
            cli.force_package,
            vec![
                PathBuf::from("packages/app"),
                PathBuf::from("packages/lib"),
                PathBuf::from(".")
            ]
        );
        assert_eq!(cli.paths, vec![PathBuf::from("README.md")]);
    }

    #[test]
    fn parses_interactive_short_option() {
        let cli = Cli::try_parse_from(["bumper", "-i"]).expect("parse CLI");

        assert!(cli.interactive);
    }

    #[test]
    fn parses_fixed_version_option() {
        let cli = Cli::try_parse_from(["bumper", "--fixed-version"]).expect("parse CLI");

        assert!(cli.fixed_version);
        assert_eq!(
            resolve_optional_bool(true, false, "BUMPER_TEST_UNSET"),
            Some(true)
        );
    }

    #[test]
    fn last_fixed_version_option_wins() {
        let cli = Cli::try_parse_from(["bumper", "--fixed-version", "--no-fixed-version"])
            .expect("parse CLI");

        assert!(!cli.fixed_version);
        assert!(cli.no_fixed_version);
        assert_eq!(
            resolve_optional_bool(cli.fixed_version, cli.no_fixed_version, "BUMPER_TEST_UNSET"),
            Some(false)
        );
        assert_eq!(
            resolve_optional_bool(false, false, "BUMPER_TEST_UNSET"),
            None
        );
    }

    #[test]
    fn parses_force_bump_type_option() {
        let cli = Cli::try_parse_from(["bumper", "--force-bump-type", "minor"]).expect("parse CLI");

        assert_eq!(cli.force_bump_type, Some(Impact::Minor));
    }

    #[test]
    fn parses_bump_type_option() {
        let cli = Cli::try_parse_from(["bumper", "--bump-type", "minor"]).expect("parse CLI");

        assert_eq!(cli.bump_type, Some(Impact::Minor));
        assert_eq!(
            resolve_bump_type(cli.bump_type, "BUMPER_TEST_UNSET"),
            Ok(Some(Impact::Minor))
        );
        assert_eq!(resolve_bump_type(None, "BUMPER_TEST_UNSET"), Ok(None));
    }

    /// A `bumper.toml` that sets every key.
    const FULL_CONFIG: &str = r#"#:schema https://trev.zip/llc/bumper/raw/branch/main/bumper.schema.json
paths = ["README.md"]
ignore-directories = ["generated"]
major-types = ["BREAKING CHANGE", "major"]
minor-types = ["feat"]
patch-types = ["fix", "perf"]
skip-scopes = []
breaking-changes = false
fixed-version = false

[groups.sdk]
packages = ["packages/sdk-*", "packages/cli"]

[groups.tools]
packages = ["."]
"#;

    #[test]
    fn parses_config_file_settings_and_groups() {
        let config = parse_file_config(FULL_CONFIG).expect("parse config file");

        assert_eq!(
            config,
            FileConfig {
                paths: vec![PathBuf::from("README.md")],
                ignore_directories: vec![PathBuf::from("generated")],
                major_types: Some(vec!["BREAKING CHANGE".to_string(), "major".to_string()]),
                minor_types: Some(vec!["feat".to_string()]),
                patch_types: Some(vec!["fix".to_string(), "perf".to_string()]),
                skip_scopes: Some(Vec::new()),
                breaking_changes: Some(false),
                fixed_version: Some(false),
                groups: vec![
                    GroupConfig {
                        name: "sdk".to_string(),
                        packages: vec!["packages/sdk-*".to_string(), "packages/cli".to_string()],
                    },
                    GroupConfig {
                        name: "tools".to_string(),
                        packages: vec![".".to_string()],
                    },
                ],
            }
        );
        assert_eq!(
            parse_file_config("groups = { sdk = { packages = [\"packages/sdk\"] } }")
                .expect("parse inline groups")
                .groups,
            vec![GroupConfig {
                name: "sdk".to_string(),
                packages: vec!["packages/sdk".to_string()],
            }]
        );
        assert_eq!(parse_file_config(""), Ok(FileConfig::default()));
    }

    fn schema_keys(schema: &serde_json::Value) -> BTreeSet<String> {
        schema["properties"]
            .as_object()
            .expect("schema properties")
            .keys()
            .cloned()
            .collect()
    }

    #[test]
    fn json_schema_describes_the_config_file_keys() {
        let schema: serde_json::Value =
            serde_json::from_str(include_str!("../bumper.schema.json")).expect("parse schema");
        let full = FULL_CONFIG
            .parse::<DocumentMut>()
            .expect("parse full config");

        let keys = schema_keys(&schema);
        let used = full
            .as_table()
            .iter()
            .map(|(key, _)| key.to_string())
            .collect::<BTreeSet<_>>();
        assert_eq!(keys, used, "FULL_CONFIG should set every schema key");

        let group_keys = schema_keys(&schema["properties"]["groups"]["additionalProperties"]);
        let used_group_keys = full["groups"]["sdk"]
            .as_table()
            .expect("sdk group")
            .iter()
            .map(|(key, _)| key.to_string())
            .collect::<BTreeSet<_>>();
        assert_eq!(group_keys, used_group_keys);
    }

    #[test]
    fn rejects_invalid_config_files() {
        for (source, message) in [
            ("paths = [", "invalid TOML"),
            ("fixed = true", "unknown key 'fixed'"),
            (
                "paths = \"README.md\"",
                "'paths' must be an array of strings",
            ),
            (
                "skip-scopes = [1]",
                "'skip-scopes' must be an array of strings",
            ),
            (
                "breaking-changes = 1",
                "'breaking-changes' must be a boolean",
            ),
            (
                "fixed-version = \"yes\"",
                "'fixed-version' must be a boolean",
            ),
            ("groups = []", "'groups' must be a table of groups"),
            ("[groups]\nsdk = 1", "group 'sdk' must be a table"),
            ("[groups.sdk]", "group 'sdk' must list its packages"),
            (
                "[groups.sdk]\npackages = []",
                "group 'sdk' must list its packages",
            ),
            (
                "[groups.sdk]\npackages = [\"a\"]\nversion = \"1.0.0\"",
                "unknown key 'version' in group 'sdk'",
            ),
        ] {
            let error = parse_file_config(source).expect_err(source);
            assert!(error.contains(message), "{source}: {error}");
        }
    }

    #[test]
    fn config_file_lists_fall_between_the_environment_and_defaults() {
        let set = |values: &[&str]| {
            values
                .iter()
                .map(ToString::to_string)
                .collect::<HashSet<_>>()
        };

        assert_eq!(
            resolve_set(
                Vec::new(),
                "BUMPER_TEST_UNSET",
                Some(vec!["Feat".to_string(), "perf".to_string()]),
                &["feat"],
            ),
            set(&["feat", "perf"])
        );
        assert_eq!(
            resolve_set(Vec::new(), "BUMPER_TEST_UNSET", Some(Vec::new()), &["ci"]),
            HashSet::new()
        );
        assert_eq!(
            resolve_set(Vec::new(), "BUMPER_TEST_UNSET", None, &["ci", "docs"]),
            set(&["ci", "docs"])
        );
        assert_eq!(
            resolve_set(
                vec!["fix".to_string()],
                "BUMPER_TEST_UNSET",
                Some(vec!["perf".to_string()]),
                &["fix"],
            ),
            set(&["fix"])
        );
    }

    #[test]
    fn config_file_paths_add_up_and_flags_override_its_fixed_version() {
        let file = || FileConfig {
            paths: vec![PathBuf::from("README.md")],
            ignore_directories: vec![PathBuf::from("generated")],
            fixed_version: Some(true),
            ..FileConfig::default()
        };
        let cli = Cli::try_parse_from(["bumper", "--ignore-directories", "legacy", "action.yaml"])
            .expect("parse CLI");
        let config = resolve_config(cli, file()).expect("resolve config");

        assert!(config.paths.contains(&PathBuf::from("README.md")));
        assert!(config.paths.contains(&PathBuf::from("action.yaml")));
        assert!(
            config
                .ignored_directories
                .contains(&PathBuf::from("generated"))
        );
        assert!(
            config
                .ignored_directories
                .contains(&PathBuf::from("legacy"))
        );
        if env::var_os("FIXED_VERSION").is_none() {
            assert_eq!(config.fixed_version, Some(true));
        }

        let cli = Cli::try_parse_from(["bumper", "--no-fixed-version"]).expect("parse CLI");
        let config = resolve_config(cli, file()).expect("resolve config");
        assert_eq!(config.fixed_version, Some(false));
    }

    #[test]
    fn breaking_changes_default_on_and_flags_override_the_config_file() {
        let resolve = |args: &[&str], breaking_changes| {
            let cli = Cli::try_parse_from(args).expect("parse CLI");
            let file = FileConfig {
                breaking_changes,
                ..FileConfig::default()
            };
            resolve_config(cli, file)
                .expect("resolve config")
                .breaking_changes
        };

        if env::var_os("BREAKING_CHANGES").is_none() {
            assert!(resolve(&["bumper"], None));
            assert!(!resolve(&["bumper"], Some(false)));
        }
        assert!(!resolve(&["bumper", "--no-breaking-changes"], Some(true)));
        assert!(resolve(&["bumper", "--breaking-changes"], Some(false)));
        assert!(resolve(
            &["bumper", "--no-breaking-changes", "--breaking-changes"],
            None
        ));
    }

    #[test]
    fn major_types_default_to_none() {
        assert_eq!(
            resolve_set(Vec::new(), "BUMPER_TEST_UNSET", None, &[]),
            HashSet::new()
        );
        if env::var_os("MAJOR_TYPES").is_none() {
            let cli = Cli::try_parse_from(["bumper"]).expect("parse CLI");
            let config = resolve_config(cli, FileConfig::default()).expect("resolve config");
            assert!(config.major_types.is_empty());
        }
    }

    #[test]
    fn rejects_invalid_force_bump_type_option() {
        let error = Cli::try_parse_from(["bumper", "--force-bump-type", "breaking"])
            .expect_err("reject invalid bump type");

        assert!(
            error
                .to_string()
                .contains("expected patch, minor, or major")
        );
    }
}

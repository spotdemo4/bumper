use std::collections::HashSet;
use std::env;
use std::path::PathBuf;

use crate::model::Config;

pub fn load_config() -> Config {
    let mut paths = parse_list_env("PATHS");
    paths.extend(env::args().skip(1).map(PathBuf::from));

    let major_types = parse_lower_set_or_default("MAJOR_TYPES", &["BREAKING CHANGE"]);
    let minor_types = parse_lower_set_or_default("MINOR_TYPES", &["feat"]);
    let patch_types = parse_lower_set_or_default("PATCH_TYPES", &["fix"]);
    let skip_scopes = parse_lower_set_or_default("SKIP_SCOPES", &["ci"]);

    Config {
        paths,
        major_types,
        minor_types,
        patch_types,
        skip_scopes,
        commit: parse_bool_env("COMMIT", true),
        tag: parse_bool_env("TAG", true),
        push: parse_bool_env("PUSH", true),
        force: parse_bool_env("FORCE", false),
        allow_dirty: parse_bool_env("ALLOW_DIRTY", false),
        ci: env::var("CI").is_ok(),
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

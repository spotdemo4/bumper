use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use bumper::package::Package;
use toml_edit::Array;

use crate::config::CONFIG_FILE;
use crate::model::AppResult;

/// The URL of the JSON schema that editors validate `bumper.toml` against.
pub const SCHEMA_URL: &str = "https://trev.zip/llc/bumper/raw/branch/main/bumper.schema.json";

/// Creates a `bumper.toml` at the repository root, refusing to replace an existing one.
pub fn init_config(
    repo_root: &Path,
    known_packages: &BTreeMap<PathBuf, Package>,
) -> AppResult<PathBuf> {
    let path = repo_root.join(CONFIG_FILE);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|e| match e.kind() {
            io::ErrorKind::AlreadyExists => format!("{} already exists", path.display()),
            _ => format!("failed to create '{}': {e}", path.display()),
        })?;
    file.write_all(render_config(known_packages).as_bytes())
        .map_err(|e| format!("failed to write '{}': {e}", path.display()))?;
    Ok(path)
}

/// A `bumper.toml` that documents every setting with its default, commented out, and suggests
/// a version group of the repository's nested packages.
fn render_config(known_packages: &BTreeMap<PathBuf, Package>) -> String {
    let nested = known_packages
        .keys()
        .filter(|path| !path.as_os_str().is_empty())
        .filter_map(|path| {
            path.iter()
                .map(|part| part.to_str())
                .collect::<Option<Vec<_>>>()
                .map(|parts| parts.join("/"))
        })
        .collect::<Vec<_>>();
    let group_packages = if nested.len() >= 2 {
        nested.into_iter().collect::<Array>()
    } else {
        ["packages/*"].into_iter().collect::<Array>()
    };

    format!(
        r#"#:schema {SCHEMA_URL}

# additional repository-relative files or directories to bump
# paths = ["README.md"]

# repository-relative directories to ignore
# ignore-directories = ["generated"]

# conventional commit types for major, minor and patch version bumps
# major-types = ["BREAKING CHANGE"]
# minor-types = ["feat"]
# patch-types = ["fix"]

# conventional commit scopes to skip over
# skip-scopes = ["ci", "docs"]

# give every package the same version, releasing all of them when any releases
# fixed-version = false

# packages that always release together at one version, while other packages keep their own
# [groups.example]
# packages = {group_packages}
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use crate::config::{FileConfig, parse_file_config};

    fn known(paths: &[&str]) -> BTreeMap<PathBuf, Package> {
        paths
            .iter()
            .map(|path| {
                (
                    PathBuf::from(path),
                    Package {
                        root: PathBuf::from("/repo").join(path),
                        path: PathBuf::from(path),
                    },
                )
            })
            .collect()
    }

    /// The config with every commented-out setting enabled.
    fn uncommented(config: &str) -> String {
        config
            .lines()
            .filter_map(|line| line.strip_prefix("# "))
            .filter(|line| line.contains('=') || line.starts_with('['))
            .flat_map(|line| [line, "\n"])
            .collect()
    }

    #[test]
    fn rendered_config_changes_nothing_until_settings_are_uncommented() {
        let config = render_config(&known(&["", "packages/app", "packages/lib"]));

        assert!(config.starts_with(&format!("#:schema {SCHEMA_URL}\n")));
        assert_eq!(parse_file_config(&config), Ok(FileConfig::default()));

        let enabled = parse_file_config(&uncommented(&config)).expect("parse uncommented config");
        assert_eq!(enabled.fixed_version, Some(false));
        assert_eq!(
            enabled.skip_scopes,
            Some(vec!["ci".to_string(), "docs".to_string()])
        );
        assert_eq!(enabled.groups.len(), 1);
        assert_eq!(
            enabled.groups.first().map(|group| group.packages.clone()),
            Some(vec!["packages/app".to_string(), "packages/lib".to_string()])
        );
    }

    #[test]
    fn rendered_config_suggests_a_pattern_without_several_nested_packages() {
        for packages in [&[""][..], &["", "packages/app"][..]] {
            let config = render_config(&known(packages));
            assert!(
                config.contains("# packages = [\"packages/*\"]\n"),
                "{config}"
            );
        }
    }

    #[test]
    fn init_refuses_to_replace_an_existing_config() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time is after epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("bumper-init-{nanos}"));
        fs::create_dir_all(&root).expect("create temp directory");

        let path = init_config(&root, &known(&[""])).expect("create config");
        assert_eq!(path, root.join(CONFIG_FILE));
        fs::write(&path, "paths = []\n").expect("edit config");

        let error = init_config(&root, &known(&[""])).expect_err("refuse existing config");
        assert!(error.contains("already exists"), "{error}");
        assert_eq!(
            fs::read_to_string(&path).expect("read config"),
            "paths = []\n"
        );
        let _ = fs::remove_dir_all(root);
    }
}

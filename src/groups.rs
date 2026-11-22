use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use bumper::package::Package;

use crate::model::{AppResult, GroupConfig};
use crate::release_plan::package_label;

/// Packages that always release together at one version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionGroup {
    /// The group's name in `bumper.toml`, or `None` for `--fixed-version`.
    pub name: Option<String>,
    pub members: BTreeSet<PathBuf>,
}

/// The version groups a release plan uses, with or without `--fixed-version`.
pub struct PlanGroups {
    configured: Vec<VersionGroup>,
    fixed: [VersionGroup; 1],
}

impl PlanGroups {
    pub fn new(
        groups: &[GroupConfig],
        known_packages: &BTreeMap<PathBuf, Package>,
    ) -> AppResult<Self> {
        Ok(Self {
            configured: resolve_groups(groups, known_packages)?,
            fixed: [VersionGroup {
                name: None,
                members: known_packages.keys().cloned().collect(),
            }],
        })
    }

    /// `--fixed-version` puts every package in one group, which covers any configured groups.
    pub fn get(&self, fixed_version: bool) -> &[VersionGroup] {
        if fixed_version {
            &self.fixed
        } else {
            &self.configured
        }
    }
}

/// Resolves the package patterns of each configured group to the packages they match.
///
/// Every pattern must match a package, and no package may belong to two groups.
fn resolve_groups(
    groups: &[GroupConfig],
    known_packages: &BTreeMap<PathBuf, Package>,
) -> AppResult<Vec<VersionGroup>> {
    let mut owners = BTreeMap::<&Path, &str>::new();
    let mut resolved = Vec::with_capacity(groups.len());
    for group in groups {
        let mut members = BTreeSet::new();
        for pattern in &group.packages {
            let segments = pattern_segments(pattern).ok_or_else(|| {
                format!(
                    "group '{}' package '{pattern}' must be a repository-relative path",
                    group.name
                )
            })?;
            let matched = known_packages
                .keys()
                .filter(|path| path_matches(&segments, &path_segments(path)))
                .collect::<Vec<_>>();
            if matched.is_empty() {
                return Err(format!(
                    "group '{}' package '{pattern}' matches no discovered package",
                    group.name
                ));
            }
            for path in matched {
                if let Some(owner) = owners.insert(path, &group.name)
                    && owner != group.name
                {
                    return Err(format!(
                        "package {} is in groups '{owner}' and '{}'",
                        package_label(path),
                        group.name
                    ));
                }
                members.insert(path.clone());
            }
        }
        resolved.push(VersionGroup {
            name: Some(group.name.clone()),
            members,
        });
    }
    Ok(resolved)
}

/// Splits a pattern into path segments. `.` is the repository root.
fn pattern_segments(pattern: &str) -> Option<Vec<&str>> {
    let path = Path::new(pattern);
    let mut segments = Vec::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => segments.push(part.to_str()?),
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(segments)
}

fn path_segments(path: &Path) -> Vec<String> {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect()
}

/// Matches path segments against pattern segments, where `**` matches any number of
/// segments and `*` matches any characters within one segment.
fn path_matches(pattern: &[&str], path: &[String]) -> bool {
    match pattern.split_first() {
        None => path.is_empty(),
        Some((&"**", rest)) => (0..=path.len()).any(|skip| {
            path.get(skip..)
                .is_some_and(|tail| path_matches(rest, tail))
        }),
        Some((segment, rest)) => path.split_first().is_some_and(|(first, tail)| {
            let segment = segment.chars().collect::<Vec<_>>();
            let first = first.chars().collect::<Vec<_>>();
            segment_matches(&segment, &first) && path_matches(rest, tail)
        }),
    }
}

fn segment_matches(pattern: &[char], text: &[char]) -> bool {
    match pattern.split_first() {
        None => text.is_empty(),
        Some(('*', rest)) => (0..=text.len()).any(|skip| {
            text.get(skip..)
                .is_some_and(|tail| segment_matches(rest, tail))
        }),
        Some((expected, rest)) => text
            .split_first()
            .is_some_and(|(actual, tail)| actual == expected && segment_matches(rest, tail)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn group(name: &str, packages: &[&str]) -> GroupConfig {
        GroupConfig {
            name: name.to_string(),
            packages: packages.iter().map(ToString::to_string).collect(),
        }
    }

    fn matches(pattern: &str, path: &str) -> bool {
        path_matches(
            &pattern_segments(pattern).expect("valid pattern"),
            &path_segments(Path::new(path)),
        )
    }

    #[test]
    fn patterns_match_literal_paths_and_wildcards() {
        assert!(matches("packages/app", "packages/app"));
        assert!(matches("./packages/app/", "packages/app"));
        assert!(!matches("packages/app", "packages/app/plugin"));
        assert!(matches(".", ""));
        assert!(!matches(".", "packages/app"));
        assert!(matches("packages/*", "packages/app"));
        assert!(!matches("packages/*", "packages/app/plugin"));
        assert!(!matches("*", ""));
        assert!(matches("packages/sdk-*", "packages/sdk-core"));
        assert!(!matches("packages/sdk-*", "packages/cli"));
        assert!(matches("packages/**", "packages/app/plugin"));
        assert!(matches("**/plugin", "packages/app/plugin"));
        assert!(matches("**", ""));
    }

    #[test]
    fn groups_resolve_to_matching_packages() {
        let known = known(&["", "packages/sdk-core", "packages/sdk-web", "packages/cli"]);
        let groups = resolve_groups(
            &[
                group("sdk", &["packages/sdk-*", "packages/sdk-core"]),
                group("tools", &[".", "packages/cli"]),
            ],
            &known,
        )
        .expect("resolve groups");

        assert_eq!(
            groups,
            vec![
                VersionGroup {
                    name: Some("sdk".to_string()),
                    members: BTreeSet::from([
                        PathBuf::from("packages/sdk-core"),
                        PathBuf::from("packages/sdk-web"),
                    ]),
                },
                VersionGroup {
                    name: Some("tools".to_string()),
                    members: BTreeSet::from([PathBuf::new(), PathBuf::from("packages/cli")]),
                },
            ]
        );
    }

    #[test]
    fn groups_reject_unmatched_overlapping_and_escaping_patterns() {
        let known = known(&["", "packages/app", "packages/lib"]);
        for (groups, message) in [
            (
                vec![group("sdk", &["packages/missing"])],
                "group 'sdk' package 'packages/missing' matches no discovered package",
            ),
            (
                vec![group("a", &["packages/*"]), group("b", &["packages/lib"])],
                "package packages/lib is in groups 'a' and 'b'",
            ),
            (
                vec![group("sdk", &["../outside"])],
                "must be a repository-relative path",
            ),
            (
                vec![group("sdk", &["/packages/app"])],
                "must be a repository-relative path",
            ),
        ] {
            let error = resolve_groups(&groups, &known).expect_err("invalid groups");
            assert!(error.contains(message), "{error}");
        }
    }
}

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use bumper::bump::{
    DependencyUpdate, VersionBump, package_name_for_file, preview_dependency_update,
};
use bumper::package::{Package, is_package_marker, package_version};
use git2::{Oid, Repository};

use crate::git_ops::{ImpactConfig, get_impact_for_package, latest_tag_or_none};
use crate::model::{AppResult, Config, Impact};
use crate::versioning::next_version;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReleaseReasons {
    pub direct: Option<Impact>,
    pub forced: bool,
    pub descendants: BTreeSet<PathBuf>,
    pub dependencies: BTreeMap<PathBuf, DependencyCause>,
}

impl ReleaseReasons {
    pub fn impact(&self) -> Option<Impact> {
        self.direct.or_else(|| {
            (self.forced || !self.descendants.is_empty() || !self.dependencies.is_empty())
                .then_some(Impact::Patch)
        })
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DependencyCause {
    pub names: BTreeSet<String>,
    pub manifests: BTreeSet<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct Release {
    pub package: Package,
    pub last_tag: String,
    pub old_version: String,
    pub new_version: String,
    pub impact: Impact,
    pub tag: String,
    pub reasons: ReleaseReasons,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamedPackageBump {
    pub package_path: PathBuf,
    pub version: VersionBump,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DependencyFilePlan {
    pub dependencies: BTreeMap<PathBuf, BTreeSet<String>>,
}

#[derive(Debug)]
pub struct ReleasePlan {
    pub releases: BTreeMap<PathBuf, Release>,
    pub skipped_packages: BTreeSet<PathBuf>,
    pub named_bumps: BTreeMap<String, NamedPackageBump>,
    pub dependency_files: BTreeMap<PathBuf, DependencyFilePlan>,
}

impl ReleasePlan {
    pub fn version_bumps(&self) -> BTreeMap<String, VersionBump> {
        version_bumps(&self.named_bumps)
    }
}

struct ReleaseBase {
    last_tag: String,
    version: String,
    commit: Option<Oid>,
}

struct PackageState {
    package: Package,
    names: BTreeSet<String>,
    base: ReleaseBase,
    reasons: ReleaseReasons,
}

pub struct PlanInput<'a> {
    pub repo: &'a Repository,
    pub repo_root: &'a Path,
    pub packages: BTreeMap<PathBuf, Package>,
    pub known_packages: &'a BTreeMap<PathBuf, Package>,
    pub selected_packages: &'a HashSet<PathBuf>,
    pub tracked_files: &'a [PathBuf],
    pub tracked_paths: &'a HashSet<PathBuf>,
    pub package_impact_paths: &'a BTreeMap<PathBuf, Vec<PathBuf>>,
    pub ignored_directories: &'a [PathBuf],
    pub config: &'a Config,
}

pub fn build_release_plan(input: PlanInput<'_>) -> AppResult<ReleasePlan> {
    let package_paths = input.packages.keys().cloned().collect::<Vec<_>>();
    let mut deepest_first = package_paths.clone();
    deepest_first.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    let parents = package_paths
        .iter()
        .filter_map(|path| {
            nearest_parent(path, &package_paths).map(|parent| (path.clone(), parent.clone()))
        })
        .collect::<BTreeMap<_, _>>();
    let mut states = BTreeMap::new();

    for (path, package) in input.packages {
        let base = resolve_release_base(input.repo, &package, input.known_packages)?;
        let child_paths = input
            .known_packages
            .keys()
            .filter(|child| *child != &path && is_package_ancestor(&path, child))
            .cloned()
            .collect::<Vec<_>>();
        let direct = get_impact_for_package(
            input.repo,
            base.commit,
            &path,
            &child_paths,
            &ImpactConfig {
                major_types: &input.config.major_types,
                minor_types: &input.config.minor_types,
                patch_types: &input.config.patch_types,
                skip_scopes: &input.config.skip_scopes,
                ignored_directories: input.ignored_directories,
                impact_paths: input
                    .package_impact_paths
                    .get(&path)
                    .map_or(&[], Vec::as_slice),
            },
        )?;
        let forced = input.config.force && input.selected_packages.contains(&path);
        let names = package_files(&package.root, input.tracked_paths)
            .into_iter()
            .filter_map(|file| package_name_for_file(&file))
            .collect();
        states.insert(
            path,
            PackageState {
                package,
                names,
                base,
                reasons: ReleaseReasons {
                    direct,
                    forced,
                    ..ReleaseReasons::default()
                },
            },
        );
    }

    propagate_descendants(&mut states, &deepest_first, &parents);
    let mut dependency_files = BTreeMap::new();

    loop {
        let named_bumps = named_package_bumps(&states)?;
        if named_bumps.is_empty() {
            break;
        }
        let version_bumps = version_bumps(&named_bumps);
        let active_before = active_package_count(&states);
        let mut round_files = BTreeMap::<PathBuf, DependencyFilePlan>::new();

        for file in input.tracked_files {
            let update = match preview_dependency_update(file, &version_bumps) {
                Ok(update) => update,
                Err(error) => {
                    eprintln!("warning: {error}");
                    continue;
                }
            };
            let DependencyUpdate::Changed(names) = update else {
                continue;
            };
            let owner = resolve_known_package_owner(input.repo_root, file, input.known_packages)?;
            let manifest = file
                .strip_prefix(input.repo_root)
                .map_err(|_| {
                    format!(
                        "file '{}' is outside repository root '{}'",
                        file.display(),
                        input.repo_root.display()
                    )
                })?
                .to_path_buf();
            let owner_state = states
                .get_mut(&owner.path)
                .expect("dependency owner is a planned package");
            for name in names {
                let Some(producer) = named_bumps.get(&name) else {
                    continue;
                };
                if producer.package_path == owner.path {
                    continue;
                }
                let cause = owner_state
                    .reasons
                    .dependencies
                    .entry(producer.package_path.clone())
                    .or_default();
                cause.names.insert(name.clone());
                cause.manifests.insert(manifest.clone());
                round_files
                    .entry(manifest.clone())
                    .or_default()
                    .dependencies
                    .entry(producer.package_path.clone())
                    .or_default()
                    .insert(name);
            }
        }

        dependency_files = round_files;
        propagate_descendants(&mut states, &deepest_first, &parents);
        if active_package_count(&states) == active_before {
            break;
        }
    }

    let releases = materialize_releases(&states)?;
    let skipped_packages = states
        .iter()
        .filter(|(path, state)| {
            input.selected_packages.contains(*path) && state.reasons.impact().is_none()
        })
        .map(|(path, _)| path.clone())
        .collect();
    let named_bumps = named_package_bumps(&states)?;

    Ok(ReleasePlan {
        releases,
        skipped_packages,
        named_bumps,
        dependency_files,
    })
}

fn active_package_count(states: &BTreeMap<PathBuf, PackageState>) -> usize {
    states
        .values()
        .filter(|state| state.reasons.impact().is_some())
        .count()
}

fn propagate_descendants(
    states: &mut BTreeMap<PathBuf, PackageState>,
    deepest_first: &[PathBuf],
    parents: &BTreeMap<PathBuf, PathBuf>,
) {
    for path in deepest_first {
        if states
            .get(path)
            .and_then(|state| state.reasons.impact())
            .is_none()
        {
            continue;
        }
        if let Some(parent) = parents.get(path)
            && let Some(parent_state) = states.get_mut(parent)
        {
            parent_state.reasons.descendants.insert(path.clone());
        }
    }
}

fn materialize_releases(
    states: &BTreeMap<PathBuf, PackageState>,
) -> AppResult<BTreeMap<PathBuf, Release>> {
    states
        .iter()
        .filter_map(|(path, state)| {
            state.reasons.impact().map(|impact| {
                let new_version = next_version(&state.base.version, impact)?;
                let tag = tag_name(path, &new_version)?;
                Ok((
                    path.clone(),
                    Release {
                        package: state.package.clone(),
                        last_tag: state.base.last_tag.clone(),
                        old_version: state.base.version.clone(),
                        new_version,
                        impact,
                        tag,
                        reasons: state.reasons.clone(),
                    },
                ))
            })
        })
        .collect()
}

fn named_package_bumps(
    states: &BTreeMap<PathBuf, PackageState>,
) -> AppResult<BTreeMap<String, NamedPackageBump>> {
    let mut bumped = BTreeMap::new();
    for (path, state) in states {
        let Some(impact) = state.reasons.impact() else {
            continue;
        };
        let new_version = next_version(&state.base.version, impact)?;
        for name in &state.names {
            bumped
                .entry(name.clone())
                .or_insert_with(|| NamedPackageBump {
                    package_path: path.clone(),
                    version: VersionBump {
                        old_version: state.base.version.clone(),
                        new_version: new_version.clone(),
                    },
                });
        }
    }
    Ok(bumped)
}

fn version_bumps(
    named_bumps: &BTreeMap<String, NamedPackageBump>,
) -> BTreeMap<String, VersionBump> {
    named_bumps
        .iter()
        .map(|(name, bump)| (name.clone(), bump.version.clone()))
        .collect()
}

pub fn package_files(package_root: &Path, tracked_paths: &HashSet<PathBuf>) -> Vec<PathBuf> {
    const MARKERS: &[&str] = &[
        "package.json",
        "Cargo.toml",
        "pyproject.toml",
        "gleam.toml",
        "build.zig.zon",
        "CMakeLists.txt",
        "build.gradle",
        "build.gradle.kts",
        "go.mod",
    ];
    let markers = MARKERS
        .iter()
        .map(|name| package_root.join(name))
        .filter(|path| tracked_paths.contains(path) && is_package_marker(path))
        .collect::<Vec<_>>();
    let marker_names = markers
        .iter()
        .filter_map(|path| path.file_name().and_then(|name| name.to_str()))
        .collect::<HashSet<_>>();
    let mut files = markers.clone();
    for (marker, companions) in [
        ("package.json", &["package-lock.json"][..]),
        ("Cargo.toml", &["Cargo.lock"][..]),
        ("pyproject.toml", &["uv.lock"][..]),
        (
            "build.gradle",
            &["gradle.properties", "build.gradle.kts"][..],
        ),
        (
            "build.gradle.kts",
            &["gradle.properties", "build.gradle"][..],
        ),
    ] {
        if marker_names.contains(marker) {
            files.extend(
                companions
                    .iter()
                    .map(|name| package_root.join(name))
                    .filter(|path| tracked_paths.contains(path)),
            );
        }
    }
    files.sort();
    files.dedup();
    files
}

pub fn resolve_known_package_hierarchy(
    repo_root: &Path,
    supplied_path: &Path,
    known_packages: &BTreeMap<PathBuf, Package>,
) -> AppResult<Vec<Package>> {
    let repo_root = std::fs::canonicalize(repo_root).map_err(|e| {
        format!(
            "failed to resolve repository root '{}': {e}",
            repo_root.display()
        )
    })?;
    let supplied_path = std::fs::canonicalize(supplied_path)
        .map_err(|e| format!("failed to resolve '{}': {e}", supplied_path.display()))?;
    let relative = supplied_path.strip_prefix(&repo_root).map_err(|_| {
        format!(
            "path '{}' is outside repository root '{}'",
            supplied_path.display(),
            repo_root.display()
        )
    })?;
    let mut hierarchy = known_packages
        .values()
        .filter(|package| {
            package.path.as_os_str().is_empty() || relative.starts_with(&package.path)
        })
        .cloned()
        .collect::<Vec<_>>();
    hierarchy.sort_by_key(|package| std::cmp::Reverse(package.path.components().count()));
    Ok(hierarchy)
}

pub fn resolve_known_package_owner<'a>(
    repo_root: &Path,
    supplied_path: &Path,
    known_packages: &'a BTreeMap<PathBuf, Package>,
) -> AppResult<&'a Package> {
    let relative = supplied_path.strip_prefix(repo_root).map_err(|_| {
        format!(
            "path '{}' is outside repository root '{}'",
            supplied_path.display(),
            repo_root.display()
        )
    })?;
    known_packages
        .values()
        .filter(|package| {
            package.path.as_os_str().is_empty() || relative.starts_with(&package.path)
        })
        .max_by_key(|package| package.path.components().count())
        .ok_or_else(|| format!("no package owns '{}'", supplied_path.display()))
}

pub fn is_package_ancestor(ancestor: &Path, descendant: &Path) -> bool {
    ancestor.as_os_str().is_empty() || descendant.starts_with(ancestor)
}

pub fn nearest_parent<'a>(path: &Path, package_paths: &'a [PathBuf]) -> Option<&'a PathBuf> {
    package_paths
        .iter()
        .filter(|candidate| *candidate != path && is_package_ancestor(candidate, path))
        .max_by_key(|candidate| candidate.components().count())
}

fn resolve_release_base(
    repo: &Repository,
    package: &Package,
    known_packages: &BTreeMap<PathBuf, Package>,
) -> AppResult<ReleaseBase> {
    if let Some((last_tag, commit)) = latest_tag_or_none(repo, &package.path)? {
        return Ok(ReleaseBase {
            version: version_from_tag(&last_tag)?.to_string(),
            last_tag,
            commit: Some(commit),
        });
    }

    let version = resolve_current_version(repo, package, known_packages)?;
    let last_tag = tag_name(&package.path, &version)?;
    let mut ancestors = known_packages
        .values()
        .filter(|ancestor| {
            ancestor.path != package.path && is_package_ancestor(&ancestor.path, &package.path)
        })
        .collect::<Vec<_>>();
    ancestors.sort_by_key(|ancestor| std::cmp::Reverse(ancestor.path.components().count()));
    let mut commit = None;
    for ancestor in ancestors {
        if let Some((_, ancestor_commit)) = latest_tag_or_none(repo, &ancestor.path)? {
            commit = Some(ancestor_commit);
            break;
        }
    }

    Ok(ReleaseBase {
        last_tag,
        version,
        commit,
    })
}

fn resolve_current_version(
    repo: &Repository,
    package: &Package,
    known_packages: &BTreeMap<PathBuf, Package>,
) -> AppResult<String> {
    if let Some((tag, _)) = latest_tag_or_none(repo, &package.path)? {
        return Ok(version_from_tag(&tag)?.to_string());
    }
    if let Some(version) = package_version(&package.root) {
        return Ok(version);
    }

    let package_paths = known_packages.keys().cloned().collect::<Vec<_>>();
    if let Some(parent_path) = nearest_parent(&package.path, &package_paths) {
        let parent = known_packages
            .get(parent_path)
            .expect("nearest parent came from known packages");
        return resolve_current_version(repo, parent, known_packages);
    }

    Err(format!(
        "no semantic version git tag, package-file version, or parent package version found for {}",
        package_label(&package.path)
    ))
}

fn version_from_tag(tag: &str) -> AppResult<&str> {
    let version = tag.rsplit('/').next().unwrap_or(tag);
    let version = version
        .strip_prefix('v')
        .or_else(|| version.strip_prefix('V'))
        .unwrap_or(version);
    if version.is_empty() {
        Err(format!("invalid semantic version tag '{tag}'"))
    } else {
        Ok(version)
    }
}

pub fn tag_name(package_path: &Path, version: &str) -> AppResult<String> {
    if package_path.as_os_str().is_empty() {
        return Ok(format!("v{version}"));
    }
    let scope = package_path
        .iter()
        .map(|part| part.to_str())
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| {
            format!(
                "package path is not valid UTF-8: {}",
                package_path.display()
            )
        })?
        .join("/");
    Ok(format!("{scope}/v{version}"))
}

pub fn package_label(path: &Path) -> String {
    if path.as_os_str().is_empty() {
        "root".to_string()
    } else {
        path.display().to_string()
    }
}

pub fn release_message<'a>(releases: impl Iterator<Item = &'a Release>) -> String {
    let transitions = releases
        .map(|release| format!("{} -> {}", release.last_tag, release.tag))
        .collect::<Vec<_>>()
        .join(", ");
    format!("bump: {transitions}")
}

pub fn commit_message<'a>(releases: impl Iterator<Item = &'a Release>) -> String {
    let releases = releases.collect::<Vec<_>>();
    if releases.len() == 1 {
        return release_message(releases.into_iter());
    }

    let root = releases
        .iter()
        .find(|release| release.package.path.as_os_str().is_empty())
        .expect("multiple releases include the root package");
    let trailers = releases
        .iter()
        .filter(|release| !release.package.path.as_os_str().is_empty())
        .map(|release| format!("Package: {}", release.tag))
        .collect::<Vec<_>>()
        .join("\n");
    format!("bump: {}\n\n{trailers}", root.tag)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(path: &str, reasons: ReleaseReasons) -> PackageState {
        PackageState {
            package: Package {
                root: PathBuf::from(path),
                path: PathBuf::from(path),
            },
            names: BTreeSet::new(),
            base: ReleaseBase {
                last_tag: String::new(),
                version: "1.0.0".to_string(),
                commit: None,
            },
            reasons,
        }
    }

    #[test]
    fn release_reasons_keep_direct_precedence_and_propagated_evidence() {
        let reasons = ReleaseReasons {
            direct: Some(Impact::Minor),
            forced: true,
            descendants: BTreeSet::from([PathBuf::from("packages/app")]),
            dependencies: BTreeMap::from([(
                PathBuf::from("packages/library"),
                DependencyCause {
                    names: BTreeSet::from(["library".to_string()]),
                    manifests: BTreeSet::from([PathBuf::from("packages/app/package.json")]),
                },
            )]),
        };

        assert_eq!(reasons.impact(), Some(Impact::Minor));
        assert!(reasons.forced);
        assert_eq!(reasons.descendants.len(), 1);
        assert_eq!(reasons.dependencies.len(), 1);
    }

    #[test]
    fn descendant_propagation_records_immediate_edges_and_all_siblings() {
        let paths = vec![
            PathBuf::new(),
            PathBuf::from("packages/parent"),
            PathBuf::from("packages/parent/one"),
            PathBuf::from("packages/parent/two"),
        ];
        let mut states = BTreeMap::from([
            (PathBuf::new(), state("", ReleaseReasons::default())),
            (
                PathBuf::from("packages/parent"),
                state("packages/parent", ReleaseReasons::default()),
            ),
            (
                PathBuf::from("packages/parent/one"),
                state(
                    "packages/parent/one",
                    ReleaseReasons {
                        direct: Some(Impact::Patch),
                        ..ReleaseReasons::default()
                    },
                ),
            ),
            (
                PathBuf::from("packages/parent/two"),
                state(
                    "packages/parent/two",
                    ReleaseReasons {
                        direct: Some(Impact::Minor),
                        ..ReleaseReasons::default()
                    },
                ),
            ),
        ]);

        let mut deepest_first = paths.clone();
        deepest_first.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
        let parents = paths
            .iter()
            .filter_map(|path| {
                nearest_parent(path, &paths).map(|parent| (path.clone(), parent.clone()))
            })
            .collect();
        propagate_descendants(&mut states, &deepest_first, &parents);

        assert_eq!(
            states[Path::new("packages/parent")].reasons.descendants,
            BTreeSet::from([
                PathBuf::from("packages/parent/one"),
                PathBuf::from("packages/parent/two"),
            ])
        );
        assert_eq!(
            states[Path::new("")].reasons.descendants,
            BTreeSet::from([PathBuf::from("packages/parent")])
        );
        assert_eq!(
            states[Path::new("packages/parent")].reasons.impact(),
            Some(Impact::Patch)
        );
    }
}

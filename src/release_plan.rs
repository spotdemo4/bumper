use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use bumper::bump::{
    DependencyUpdate, VersionBump, package_name_for_file, preview_dependency_update,
};
use bumper::go::{
    ModuleRename, PathMajor, local_replacements, read_module_path, required_module_path,
    rewrite_file, split_path_major, workspace_uses,
};
use bumper::package::{Package, is_package_marker, package_version};
use git2::{Oid, Repository};

use crate::git_ops::{ImpactConfig, MajorFilter, get_impact_for_package, latest_tag_or_none};
use crate::model::{AppResult, Config, Impact};
use crate::versioning::{next_version, parse_version};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReleaseReasons {
    pub direct: Option<Impact>,
    /// The impact implied by the commits when `--bump-type` replaced it with `direct`.
    pub overridden: Option<Impact>,
    pub forced: Option<Impact>,
    pub descendants: BTreeSet<PathBuf>,
    pub dependencies: BTreeMap<PathBuf, DependencyCause>,
    pub fixed: Option<FixedCause>,
}

impl ReleaseReasons {
    pub fn impact(&self) -> Option<Impact> {
        [
            self.direct,
            self.forced,
            self.fixed.as_ref().map(|cause| cause.impact),
            (!self.descendants.is_empty() || !self.dependencies.is_empty())
                .then_some(Impact::Patch),
        ]
        .into_iter()
        .flatten()
        .max()
    }
}

/// A release caused by `--fixed-version`: every package takes the highest impact in the repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixedCause {
    pub impact: Impact,
    pub sources: BTreeSet<PathBuf>,
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

/// A go.mod or .go file whose references to Go modules move to new major version paths.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GoFilePlan {
    pub renames: Vec<ModuleRename>,
}

/// Why a selected package does not release.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    NoChanges,
    /// An untagged package at version 0.0.0, which only releases when forced by name.
    Unversioned,
}

#[derive(Debug)]
pub struct ReleasePlan {
    pub releases: BTreeMap<PathBuf, Release>,
    pub skipped_packages: BTreeMap<PathBuf, SkipReason>,
    pub named_bumps: BTreeMap<String, NamedPackageBump>,
    pub dependency_files: BTreeMap<PathBuf, DependencyFilePlan>,
    pub go_files: BTreeMap<PathBuf, GoFilePlan>,
    /// The module path of every Go module in the repository.
    pub go_module_paths: Vec<String>,
    pub current_versions: BTreeMap<PathBuf, String>,
}

impl ReleasePlan {
    pub fn version_bumps(&self) -> BTreeMap<String, VersionBump> {
        version_bumps(&self.named_bumps)
    }

    /// The version every package currently has, when there are several packages and they all agree.
    pub fn shared_current_version(&self) -> Option<&str> {
        let first = self.current_versions.values().next()?;
        (self.current_versions.len() > 1
            && self
                .current_versions
                .values()
                .all(|version| version == first))
        .then_some(first.as_str())
    }
}

struct ReleaseBase {
    last_tag: String,
    version: String,
    commit: Option<Oid>,
    /// Whether the package has a tag of its own rather than a manifest or inherited version.
    tagged: bool,
    stream: TagStream,
}

/// The tags a package releases under.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TagStream {
    /// The tag prefix, which is the parent directory for a Go major version subdirectory.
    scope: PathBuf,
    majors: MajorFilter,
}

impl ReleaseBase {
    fn is_unversioned(&self, path: &Path) -> bool {
        !path.as_os_str().is_empty() && !self.tagged && self.version == UNVERSIONED
    }
}

/// The version of a package that has not started releasing.
const UNVERSIONED: &str = "0.0.0";

struct PackageState {
    package: Package,
    names: BTreeSet<String>,
    base: ReleaseBase,
    /// The version bumped to produce the release, when it differs from `base.version`.
    shared_version: Option<String>,
    reasons: ReleaseReasons,
    /// Never releases, though its reasons are still recorded for its dependency files.
    unversioned: bool,
}

impl PackageState {
    fn impact(&self) -> Option<Impact> {
        if self.unversioned {
            None
        } else {
            self.reasons.impact()
        }
    }
}

pub struct PlanInput<'a> {
    pub repo: &'a Repository,
    pub repo_root: &'a Path,
    pub packages: BTreeMap<PathBuf, Package>,
    pub known_packages: &'a BTreeMap<PathBuf, Package>,
    pub selected_packages: &'a HashSet<PathBuf>,
    pub forced_packages: &'a HashSet<PathBuf>,
    pub tracked_files: &'a [PathBuf],
    pub tracked_paths: &'a HashSet<PathBuf>,
    pub package_impact_paths: &'a BTreeMap<PathBuf, Vec<PathBuf>>,
    pub ignored_directories: &'a [PathBuf],
    pub config: &'a Config,
    pub fixed_version: bool,
}

pub fn build_release_plan(mut input: PlanInput<'_>) -> AppResult<ReleasePlan> {
    let package_paths = input.packages.keys().cloned().collect::<Vec<_>>();
    let mut deepest_first = package_paths.clone();
    deepest_first.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    let parents = package_paths
        .iter()
        .filter_map(|path| {
            nearest_parent(path, &package_paths).map(|parent| (path.clone(), parent.clone()))
        })
        .collect::<BTreeMap<_, _>>();
    let mut bases = BTreeMap::new();
    for (path, package) in input.known_packages {
        bases.insert(
            path.clone(),
            resolve_release_base(input.repo, package, input.known_packages)?,
        );
    }
    // forcing a package by name starts its releases, so it claims its own commits again
    let unversioned = bases
        .iter()
        .filter(|(path, base)| !input.forced_packages.contains(*path) && base.is_unversioned(path))
        .map(|(path, _)| path.clone())
        .collect::<HashSet<_>>();
    let mut states = BTreeMap::new();
    for (path, package) in std::mem::take(&mut input.packages) {
        let base = bases
            .remove(&path)
            .ok_or_else(|| format!("{} is not a known package", package_label(&path)))?;
        let state = package_state(&input, &path, package, base, &unversioned)?;
        states.insert(path, state);
    }

    if input.fixed_version {
        apply_fixed_version(&mut states)?;
    } else {
        propagate_descendants(&mut states, &deepest_first, &parents);
    }
    let go = go_repository(&input);
    let Propagation {
        dependency_files,
        go_files,
    } = propagate_dependencies(&input, &go, &mut states, &deepest_first, &parents)?;

    if input.fixed_version {
        // Every package already releases at the shared version, so propagation adds no information.
        for state in states.values_mut() {
            state.reasons.descendants.clear();
            state.reasons.dependencies.clear();
        }
    }

    let releases = materialize_releases(&states)?;
    let skipped_packages = states
        .iter()
        .filter(|(path, state)| input.selected_packages.contains(*path) && state.impact().is_none())
        .map(|(path, state)| {
            let reason = if state.unversioned {
                SkipReason::Unversioned
            } else {
                SkipReason::NoChanges
            };
            (path.clone(), reason)
        })
        .collect();
    let named_bumps = named_package_bumps(&states)?;
    let current_versions = states
        .iter()
        .filter(|(_, state)| !state.unversioned)
        .map(|(path, state)| (path.clone(), state.base.version.clone()))
        .collect();

    Ok(ReleasePlan {
        releases,
        skipped_packages,
        named_bumps,
        dependency_files,
        go_files,
        go_module_paths: go.module_paths(),
        current_versions,
    })
}

/// Builds a package's release state. Unversioned packages never release, so their commits
/// count for their nearest versioned ancestor instead.
fn package_state(
    input: &PlanInput<'_>,
    path: &Path,
    package: Package,
    base: ReleaseBase,
    unversioned: &HashSet<PathBuf>,
) -> AppResult<PackageState> {
    let is_unversioned = unversioned.contains(path);
    let child_paths = input
        .known_packages
        .keys()
        .filter(|child| {
            *child != path && is_package_ancestor(path, child) && !unversioned.contains(*child)
        })
        .cloned()
        .collect::<Vec<_>>();
    let impact_paths = owned_impact_paths(input, path, unversioned);
    let direct = if is_unversioned {
        None
    } else {
        get_impact_for_package(
            input.repo,
            base.commit,
            path,
            &child_paths,
            &ImpactConfig {
                major_types: &input.config.major_types,
                minor_types: &input.config.minor_types,
                patch_types: &input.config.patch_types,
                skip_scopes: &input.config.skip_scopes,
                ignored_directories: input.ignored_directories,
                impact_paths: &impact_paths,
            },
        )?
    };
    let (direct, overridden) = override_impact(direct, input.config.bump_type);
    let forced = input
        .forced_packages
        .contains(path)
        .then_some(input.config.force_bump_type);
    let names = package_files(&package.root, input.tracked_paths)
        .into_iter()
        .filter_map(|file| package_name_for_file(&file))
        .collect();
    Ok(PackageState {
        package,
        names,
        base,
        shared_version: None,
        reasons: ReleaseReasons {
            direct,
            overridden,
            forced,
            ..ReleaseReasons::default()
        },
        unversioned: is_unversioned,
    })
}

/// The impact paths of `path` and of the unversioned packages whose commits it owns.
fn owned_impact_paths(
    input: &PlanInput<'_>,
    path: &Path,
    unversioned: &HashSet<PathBuf>,
) -> Vec<PathBuf> {
    let versioned = input
        .known_packages
        .keys()
        .filter(|package| !unversioned.contains(*package))
        .cloned()
        .collect::<Vec<_>>();
    input
        .package_impact_paths
        .iter()
        .filter(|(owner, _)| {
            *owner == path
                || (unversioned.contains(*owner)
                    && nearest_parent(owner, &versioned).is_some_and(|parent| parent == path))
        })
        .flat_map(|(_, paths)| paths.iter().cloned())
        .collect()
}

/// Whether `package` is untagged at version 0.0.0, so it only releases when forced by name.
pub fn is_unversioned_package(
    repo: &Repository,
    package: &Package,
    known_packages: &BTreeMap<PathBuf, Package>,
) -> AppResult<bool> {
    Ok(resolve_release_base(repo, package, known_packages)?.is_unversioned(&package.path))
}

/// The files the final round of dependency propagation updates.
struct Propagation {
    dependency_files: BTreeMap<PathBuf, DependencyFilePlan>,
    go_files: BTreeMap<PathBuf, GoFilePlan>,
}

/// Releases packages whose dependencies on other bumped packages change, until no
/// more packages release.
fn propagate_dependencies(
    input: &PlanInput<'_>,
    go: &GoRepository,
    states: &mut BTreeMap<PathBuf, PackageState>,
    deepest_first: &[PathBuf],
    parents: &BTreeMap<PathBuf, PathBuf>,
) -> AppResult<Propagation> {
    let mut propagation = Propagation {
        dependency_files: BTreeMap::new(),
        go_files: BTreeMap::new(),
    };

    loop {
        let active_before = active_package_count(states);
        if active_before == 0 {
            break;
        }
        let named_bumps = named_package_bumps(states)?;
        let version_bumps = version_bumps(&named_bumps);
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
            let manifest = repository_relative(input.repo_root, file)?;
            let owner_state = states.get_mut(&owner.path).ok_or_else(|| {
                format!(
                    "dependency owner {} is not a planned package",
                    package_label(&owner.path)
                )
            })?;
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

        propagation.dependency_files = round_files;
        propagation.go_files = plan_go_rewrites(go, input.repo_root, states)?;
        propagate_descendants(states, deepest_first, parents);
        if active_package_count(states) == active_before {
            break;
        }
    }

    Ok(propagation)
}

/// A Go module in the repository.
struct GoModule {
    package_path: PathBuf,
    root: PathBuf,
    module_path: String,
    /// The tracked go.mod and .go files that build the module.
    files: Vec<PathBuf>,
    /// The module paths its go.mod replaces with the directory of that module.
    local_replacements: BTreeSet<String>,
}

#[derive(Default)]
struct GoRepository {
    modules: Vec<GoModule>,
    /// The module directories `use`d by the repository's go.work.
    workspace: HashSet<PathBuf>,
}

impl GoRepository {
    fn module_paths(&self) -> Vec<String> {
        self.modules
            .iter()
            .map(|module| module.module_path.clone())
            .collect()
    }

    /// Whether `consumer` builds against the repository's copy of `producer` rather than a
    /// published version.
    fn resolves_locally(&self, consumer: &GoModule, producer: &GoModule) -> bool {
        consumer.local_replacements.contains(&producer.module_path)
            || (self.workspace.contains(&consumer.root) && self.workspace.contains(&producer.root))
    }
}

fn go_repository(input: &PlanInput<'_>) -> GoRepository {
    let mut modules = input
        .known_packages
        .values()
        .filter_map(|package| {
            let go_mod = package.root.join("go.mod");
            if !input.tracked_paths.contains(&go_mod) {
                return None;
            }
            Some(GoModule {
                package_path: package.path.clone(),
                root: package.root.clone(),
                module_path: read_module_path(&go_mod)?,
                files: Vec::new(),
                local_replacements: BTreeSet::new(),
            })
        })
        .collect::<Vec<_>>();
    if modules.is_empty() {
        return GoRepository::default();
    }

    for file in input.tracked_files {
        if file.extension() != Some(OsStr::new("go"))
            && file.file_name() != Some(OsStr::new("go.mod"))
        {
            continue;
        }
        let Some(owner) = modules
            .iter_mut()
            .filter(|module| file.starts_with(&module.root))
            .max_by_key(|module| module.root.components().count())
        else {
            continue;
        };
        // the go command ignores these directories and files
        let built = file.strip_prefix(&owner.root).is_ok_and(|relative| {
            relative.components().all(|component| {
                let name = component.as_os_str().to_string_lossy();
                name != "testdata" && !name.starts_with('_') && !name.starts_with('.')
            })
        });
        if built {
            owner.files.push(file.clone());
        }
    }

    let roots = modules
        .iter()
        .map(|module| (module.root.clone(), module.module_path.clone()))
        .collect::<Vec<_>>();
    for module in &mut modules {
        let Ok(source) = fs::read_to_string(module.root.join("go.mod")) else {
            continue;
        };
        for (path, directory) in local_replacements(&source) {
            let Ok(target) = fs::canonicalize(module.root.join(directory)) else {
                continue;
            };
            if roots
                .iter()
                .any(|(root, module_path)| *root == target && *module_path == path)
            {
                module.local_replacements.insert(path);
            }
        }
    }

    let go_work = input.repo_root.join("go.work");
    let workspace = if input.tracked_paths.contains(&go_work) {
        fs::read_to_string(&go_work)
            .map(|source| workspace_uses(&source))
            .unwrap_or_default()
            .into_iter()
            .filter_map(|directory| fs::canonicalize(input.repo_root.join(directory)).ok())
            .collect()
    } else {
        HashSet::new()
    };

    GoRepository { modules, workspace }
}

/// Moves each releasing Go module whose new major version its module path doesn't allow to
/// the path of that major, with the modules that build against its local copy. Those modules
/// release as its dependents.
fn plan_go_rewrites(
    go: &GoRepository,
    repo_root: &Path,
    states: &mut BTreeMap<PathBuf, PackageState>,
) -> AppResult<BTreeMap<PathBuf, GoFilePlan>> {
    let mut renames = BTreeMap::new();
    for module in &go.modules {
        let Some(state) = states.get(&module.package_path) else {
            continue;
        };
        let Some(impact) = state.impact() else {
            continue;
        };
        let version = planned_version(state, impact)?;
        let major_subdirectory = matches!(state.base.stream.majors, MajorFilter::Only(_));
        let new_path = required_module_path(&module.module_path, &version, major_subdirectory)
            .map_err(|e| format!("{}: {e}", package_label(&module.package_path)))?;
        if let Some(new_path) = new_path {
            renames.insert(
                module.package_path.clone(),
                ModuleRename {
                    old_path: module.module_path.clone(),
                    new_path,
                    version,
                },
            );
        }
    }
    if renames.is_empty() {
        return Ok(BTreeMap::new());
    }

    let module_paths = go.module_paths();
    let mut files = BTreeMap::new();
    for consumer in &go.modules {
        let applicable = renames
            .iter()
            .filter(|(producer, _)| {
                **producer == consumer.package_path
                    || go.modules.iter().any(|module| {
                        module.package_path == **producer && go.resolves_locally(consumer, module)
                    })
            })
            .collect::<Vec<_>>();
        if applicable.is_empty() {
            continue;
        }
        let consumer_renames = applicable
            .iter()
            .map(|(_, rename)| (*rename).clone())
            .collect::<Vec<_>>();

        let mut changed = Vec::new();
        for file in &consumer.files {
            let rewrite = match rewrite_file(file, &consumer_renames, &module_paths) {
                Ok(Some(rewrite)) => rewrite,
                Ok(None) => continue,
                Err(error) => {
                    eprintln!("warning: {error}");
                    continue;
                }
            };
            let relative = repository_relative(repo_root, file)?;
            let renames = consumer_renames
                .iter()
                .filter(|rename| rewrite.changed.contains(&rename.old_path))
                .cloned()
                .collect();
            files.insert(relative.clone(), GoFilePlan { renames });
            changed.push((relative, rewrite.changed));
        }

        for (producer, rename) in applicable {
            if *producer == consumer.package_path {
                continue;
            }
            let touched = changed
                .iter()
                .filter(|(_, paths)| paths.contains(&rename.old_path))
                .map(|(file, _)| file)
                .collect::<Vec<_>>();
            if touched.is_empty() {
                continue;
            }
            // the go.mod explains the release best, so imports are only listed without one
            let go_mods = touched
                .iter()
                .copied()
                .filter(|file| file.file_name() == Some(OsStr::new("go.mod")))
                .collect::<Vec<_>>();
            let manifests = if go_mods.is_empty() { touched } else { go_mods };
            let state = states.get_mut(&consumer.package_path).ok_or_else(|| {
                format!(
                    "Go module {} is not a planned package",
                    package_label(&consumer.package_path)
                )
            })?;
            let cause = state
                .reasons
                .dependencies
                .entry(producer.clone())
                .or_default();
            cause.names.insert(rename.old_path.clone());
            cause.manifests.extend(manifests.into_iter().cloned());
        }
    }

    Ok(files)
}

/// Applies `--bump-type` to a commit-derived impact, returning it with the impact it replaced.
fn override_impact(
    direct: Option<Impact>,
    bump_type: Option<Impact>,
) -> (Option<Impact>, Option<Impact>) {
    match (direct, bump_type) {
        (Some(impact), Some(bump_type)) if impact != bump_type => (Some(bump_type), Some(impact)),
        _ => (direct, None),
    }
}

fn active_package_count(states: &BTreeMap<PathBuf, PackageState>) -> usize {
    states
        .values()
        .filter(|state| state.impact().is_some())
        .count()
}

/// Releases every package at one version when any package has a direct or forced impact.
///
/// The shared version bumps the highest current version so no package ever moves backwards.
fn apply_fixed_version(states: &mut BTreeMap<PathBuf, PackageState>) -> AppResult<()> {
    let own_impact = |state: &PackageState| {
        (!state.unversioned)
            .then_some(state.reasons.direct.max(state.reasons.forced))
            .flatten()
    };
    let Some(impact) = states.values().filter_map(own_impact).max() else {
        return Ok(());
    };
    let sources = states
        .iter()
        .filter(|(_, state)| own_impact(state) == Some(impact))
        .map(|(path, _)| path.clone())
        .collect::<BTreeSet<_>>();
    let Some(shared_version) = states
        .values()
        .filter(|state| !state.unversioned)
        .map(|state| parse_version(&state.base.version).map(|version| (version, state)))
        .collect::<AppResult<Vec<_>>>()?
        .into_iter()
        .max_by_key(|(version, _)| *version)
        .map(|(_, state)| state.base.version.clone())
    else {
        return Ok(());
    };

    for state in states.values_mut().filter(|state| !state.unversioned) {
        state.shared_version = Some(shared_version.clone());
        if own_impact(state) != Some(impact) {
            state.reasons.fixed = Some(FixedCause {
                impact,
                sources: sources.clone(),
            });
        }
    }
    Ok(())
}

fn planned_version(state: &PackageState, impact: Impact) -> AppResult<String> {
    next_version(
        state
            .shared_version
            .as_deref()
            .unwrap_or(&state.base.version),
        impact,
    )
}

fn propagate_descendants(
    states: &mut BTreeMap<PathBuf, PackageState>,
    deepest_first: &[PathBuf],
    parents: &BTreeMap<PathBuf, PathBuf>,
) {
    for path in deepest_first {
        if states.get(path).and_then(PackageState::impact).is_none() {
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
            state.impact().map(|impact| {
                let new_version = planned_version(state, impact)?;
                let major = parse_version(&new_version)?.major();
                match &state.base.stream.majors {
                    MajorFilter::Only(only) if *only != major => {
                        return Err(format!(
                            "{} is a Go major version subdirectory for v{only}, so it can't release {new_version}",
                            package_label(path)
                        ));
                    }
                    MajorFilter::Except(excluded) if excluded.contains(&major) => {
                        return Err(format!(
                            "{} can't release {new_version}: its v{major} tags belong to the Go module in its v{major} directory",
                            package_label(path)
                        ));
                    }
                    _ => {}
                }
                let tag = tag_name(&state.base.stream.scope, &new_version)?;
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
        let Some(impact) = state.impact() else {
            continue;
        };
        let new_version = planned_version(state, impact)?;
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

/// The tag stream of `package`. Go tags a module in a major version subdirectory such as `v2`
/// with its parent directory's prefix, so the two share that prefix by major version.
fn tag_stream(package: &Package, known_packages: &BTreeMap<PathBuf, Package>) -> TagStream {
    if let Some(major) = go_major_subdirectory(package) {
        return TagStream {
            scope: package
                .path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_default(),
            majors: MajorFilter::Only(major),
        };
    }

    let claimed = known_packages
        .values()
        .filter(|other| other.path.parent() == Some(package.path.as_path()))
        .filter_map(go_major_subdirectory)
        .collect::<BTreeSet<_>>();
    TagStream {
        scope: package.path.clone(),
        majors: if claimed.is_empty() {
            MajorFilter::Any
        } else {
            MajorFilter::Except(claimed)
        },
    }
}

/// The major version of a Go module in a major version subdirectory, whose directory name
/// matches the major version suffix of its module path.
fn go_major_subdirectory(package: &Package) -> Option<u64> {
    let directory = package.path.file_name()?.to_str()?;
    if !directory.starts_with('v') {
        return None;
    }
    let module_path = read_module_path(&package.root.join("go.mod"))?;
    match split_path_major(&module_path) {
        Ok((_, PathMajor::Suffixed(major))) if directory == format!("v{major}") => Some(major),
        _ => None,
    }
}

fn resolve_release_base(
    repo: &Repository,
    package: &Package,
    known_packages: &BTreeMap<PathBuf, Package>,
) -> AppResult<ReleaseBase> {
    let stream = tag_stream(package, known_packages);
    if let Some((last_tag, commit)) = latest_tag_or_none(repo, &stream.scope, &stream.majors)? {
        return Ok(ReleaseBase {
            version: version_from_tag(&last_tag)?.to_string(),
            last_tag,
            commit: Some(commit),
            tagged: true,
            stream,
        });
    }

    let version = resolve_current_version(repo, package, known_packages)?;
    let last_tag = tag_name(&stream.scope, &version)?;
    let mut ancestors = known_packages
        .values()
        .filter(|ancestor| {
            ancestor.path != package.path && is_package_ancestor(&ancestor.path, &package.path)
        })
        .collect::<Vec<_>>();
    ancestors.sort_by_key(|ancestor| std::cmp::Reverse(ancestor.path.components().count()));
    let mut commit = None;
    for ancestor in ancestors {
        let ancestor_stream = tag_stream(ancestor, known_packages);
        if let Some((_, ancestor_commit)) =
            latest_tag_or_none(repo, &ancestor_stream.scope, &ancestor_stream.majors)?
        {
            commit = Some(ancestor_commit);
            break;
        }
    }

    Ok(ReleaseBase {
        last_tag,
        version,
        commit,
        tagged: false,
        stream,
    })
}

fn resolve_current_version(
    repo: &Repository,
    package: &Package,
    known_packages: &BTreeMap<PathBuf, Package>,
) -> AppResult<String> {
    let stream = tag_stream(package, known_packages);
    if let Some((tag, _)) = latest_tag_or_none(repo, &stream.scope, &stream.majors)? {
        return Ok(version_from_tag(&tag)?.to_string());
    }
    if let Some(version) = package_version(&package.root) {
        return Ok(version);
    }

    let package_paths = known_packages.keys().cloned().collect::<Vec<_>>();
    if let Some(parent_path) = nearest_parent(&package.path, &package_paths)
        && let Some(parent) = known_packages.get(parent_path)
    {
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

pub fn repository_relative(repo_root: &Path, file: &Path) -> AppResult<PathBuf> {
    file.strip_prefix(repo_root)
        .map(Path::to_path_buf)
        .map_err(|_| {
            format!(
                "file '{}' is outside repository root '{}'",
                file.display(),
                repo_root.display()
            )
        })
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

    // a nested release always releases the root package too
    let Some(root) = releases
        .iter()
        .find(|release| release.package.path.as_os_str().is_empty())
    else {
        return release_message(releases.into_iter());
    };
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
                tagged: true,
                stream: TagStream {
                    scope: PathBuf::from(path),
                    majors: MajorFilter::Any,
                },
            },
            shared_version: None,
            reasons,
            unversioned: false,
        }
    }

    fn plan_with_versions(versions: &[(&str, &str)]) -> ReleasePlan {
        ReleasePlan {
            releases: BTreeMap::new(),
            skipped_packages: BTreeMap::new(),
            named_bumps: BTreeMap::new(),
            dependency_files: BTreeMap::new(),
            go_files: BTreeMap::new(),
            go_module_paths: Vec::new(),
            current_versions: versions
                .iter()
                .map(|(path, version)| (PathBuf::from(path), version.to_string()))
                .collect(),
        }
    }

    #[test]
    fn shared_current_version_requires_several_matching_packages() {
        assert_eq!(
            plan_with_versions(&[("", "1.2.3"), ("packages/app", "1.2.3")])
                .shared_current_version(),
            Some("1.2.3")
        );
        assert_eq!(
            plan_with_versions(&[("", "1.2.3"), ("packages/app", "1.2.4")])
                .shared_current_version(),
            None
        );
        assert_eq!(
            plan_with_versions(&[("", "1.2.3")]).shared_current_version(),
            None
        );
        assert_eq!(plan_with_versions(&[]).shared_current_version(), None);
    }

    #[test]
    fn release_reasons_use_highest_impact_and_keep_propagated_evidence() {
        let reasons = ReleaseReasons {
            direct: Some(Impact::Patch),
            forced: Some(Impact::Minor),
            descendants: BTreeSet::from([PathBuf::from("packages/app")]),
            dependencies: BTreeMap::from([(
                PathBuf::from("packages/library"),
                DependencyCause {
                    names: BTreeSet::from(["library".to_string()]),
                    manifests: BTreeSet::from([PathBuf::from("packages/app/package.json")]),
                },
            )]),
            fixed: None,
            overridden: None,
        };

        assert_eq!(reasons.impact(), Some(Impact::Minor));
        assert_eq!(reasons.forced, Some(Impact::Minor));
        assert_eq!(reasons.descendants.len(), 1);
        assert_eq!(reasons.dependencies.len(), 1);
    }

    #[test]
    fn bump_type_replaces_commit_impact_only_when_commits_have_one() {
        assert_eq!(
            override_impact(Some(Impact::Major), Some(Impact::Minor)),
            (Some(Impact::Minor), Some(Impact::Major))
        );
        assert_eq!(
            override_impact(Some(Impact::Patch), Some(Impact::Minor)),
            (Some(Impact::Minor), Some(Impact::Patch))
        );
        assert_eq!(
            override_impact(Some(Impact::Minor), Some(Impact::Minor)),
            (Some(Impact::Minor), None)
        );
        assert_eq!(override_impact(None, Some(Impact::Minor)), (None, None));
        assert_eq!(
            override_impact(Some(Impact::Major), None),
            (Some(Impact::Major), None)
        );
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

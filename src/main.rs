mod config;
mod git_ops;
mod model;
mod versioning;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use bumper::bump::{
    TypedChange, apply_typed_change, bump_cargo_lock_dependencies, bump_cargo_toml_dependencies,
    bump_package_json_dependencies, bump_package_lock_dependencies, dependency_update_needed,
};
use bumper::package::{Package, is_package_marker};
use git2::Repository;

use config::load_config;
use git_ops::{
    ImpactConfig, current_branch, ensure_clean_repo, get_impact_for_package, git_commit, git_fetch,
    git_push, git_tag, latest_tag, list_tracked_files_under, repo_root, stage_path, staged_files,
};
use model::{AppResult, Impact};
use versioning::next_version;

#[derive(Debug)]
struct Target {
    path: PathBuf,
    package_path: PathBuf,
}

#[derive(Debug)]
struct Release {
    package: Package,
    last_tag: String,
    old_version: String,
    new_version: String,
    impact: Option<Impact>,
    tag: String,
}

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
    let repo = Repository::discover(".")
        .or_else(|e| {
            // In Docker with bind-mounted repos the directory may be owned by a different
            // user. Retry with owner validation disabled only when that is the actual cause.
            if e.class() == git2::ErrorClass::Config && e.code() == git2::ErrorCode::Owner {
                unsafe { git2::opts::set_verify_owner_validation(false) }.map_err(|_| e)?;
                Repository::discover(".")
            } else {
                Err(e)
            }
        })
        .map_err(|e| format!("not a git repository: {e}"))?;
    let repo_root = fs::canonicalize(repo_root(&repo)?)
        .map_err(|e| format!("failed to resolve repository root: {e}"))?;

    if config.paths.is_empty() {
        config.paths = vec![repo_root.clone()];
    }

    if !config.allow_dirty {
        ensure_clean_repo(&repo)?;
    }

    println!("fetching latest tags from remote...");
    git_fetch(&repo)?;

    let tracked_files = list_tracked_files_under(&repo, &repo_root, &repo_root)?;
    let known_packages = discover_packages(&repo_root, &tracked_files)?;
    let tracked_paths = tracked_files.iter().cloned().collect::<HashSet<_>>();
    let mut packages = BTreeMap::<PathBuf, Package>::new();
    let mut selected_packages = HashSet::<PathBuf>::new();
    let mut targets = Vec::new();
    for path in &config.paths {
        let absolute = if path.is_absolute() {
            path.clone()
        } else {
            repo_root.join(path)
        };

        if !absolute.exists() {
            eprintln!(
                "warning: file or directory not found: {}",
                absolute.display()
            );
            continue;
        }

        let hierarchy = resolve_known_package_hierarchy(&repo_root, &absolute, &known_packages)?;
        let selected = hierarchy
            .first()
            .expect("package hierarchy always contains the repository root")
            .path
            .clone();
        selected_packages.insert(selected.clone());
        targets.push(Target {
            path: absolute,
            package_path: selected,
        });
        for package in hierarchy {
            packages.entry(package.path.clone()).or_insert(package);
        }
    }

    if targets.is_empty() {
        return Err("no valid files or directories were selected".to_string());
    }

    println!("determining package releases...");
    let package_paths = packages.keys().cloned().collect::<Vec<_>>();
    let mut releases = BTreeMap::<PathBuf, Release>::new();
    for (path, package) in packages {
        let (last_tag, last_tag_commit) = latest_tag(&repo, &path)?;
        let old_version = version_from_tag(&last_tag)?.to_string();
        let child_paths = known_packages
            .keys()
            .filter(|child| *child != &path && is_package_ancestor(&path, child))
            .cloned()
            .collect::<Vec<_>>();
        let impact = get_impact_for_package(
            &repo,
            last_tag_commit,
            &path,
            &child_paths,
            &ImpactConfig {
                major_types: &config.major_types,
                minor_types: &config.minor_types,
                patch_types: &config.patch_types,
                skip_scopes: &config.skip_scopes,
                force: config.force && selected_packages.contains(&path),
            },
        )?;
        releases.insert(
            path,
            Release {
                package,
                last_tag,
                old_version,
                new_version: String::new(),
                impact,
                tag: String::new(),
            },
        );
    }

    let mut deepest_first = package_paths.clone();
    deepest_first.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    for path in &deepest_first {
        if releases
            .get(path)
            .and_then(|release| release.impact)
            .is_none()
        {
            continue;
        }
        if let Some(parent) = nearest_parent(path, &package_paths)
            && let Some(parent_release) = releases.get_mut(parent)
            && parent_release.impact.is_none()
        {
            parent_release.impact = Some(Impact::Patch);
        }
    }

    releases.retain(|_, release| release.impact.is_some());
    if releases.is_empty() {
        println!("no new impactful commits for the selected packages");
        return Ok(());
    }

    for release in releases.values_mut() {
        let impact = release.impact.expect("retained releases have an impact");
        release.new_version = next_version(&release.old_version, impact)?;
        release.tag = tag_name(&release.package.path, &release.new_version)?;
        println!(
            "{}: {} -> {} ({})",
            package_label(&release.package.path),
            release.last_tag,
            release.tag,
            impact.as_str()
        );
    }

    let mut bumped: HashMap<String, (String, String)> = HashMap::new();
    for release in releases.values() {
        add_bumped_package_names(release, &tracked_paths, &mut bumped);
    }

    if !bumped.is_empty() {
        loop {
            let mut changed_packages = BTreeMap::<PathBuf, Package>::new();
            for abs in &tracked_files {
                match dependency_update_needed(abs, &bumped) {
                    Ok(true) => {
                        let owner =
                            resolve_known_package_hierarchy(&repo_root, abs, &known_packages)?
                                .into_iter()
                                .next()
                                .expect("known package hierarchy contains root");
                        if !releases.contains_key(&owner.path) {
                            changed_packages.insert(owner.path.clone(), owner);
                        }
                    }
                    Ok(false) => {}
                    Err(e) => eprintln!("warning: {e}"),
                }
            }

            if changed_packages.is_empty() {
                break;
            }

            let mut added_paths = Vec::new();
            for package in changed_packages.into_values() {
                let hierarchy =
                    resolve_known_package_hierarchy(&repo_root, &package.root, &known_packages)?;
                for package in hierarchy {
                    if releases.contains_key(&package.path) {
                        continue;
                    }
                    let (last_tag, last_tag_commit) = latest_tag(&repo, &package.path)?;
                    let old_version = version_from_tag(&last_tag)?.to_string();
                    let child_paths = known_packages
                        .keys()
                        .filter(|child| {
                            *child != &package.path && is_package_ancestor(&package.path, child)
                        })
                        .cloned()
                        .collect::<Vec<_>>();
                    let impact = get_impact_for_package(
                        &repo,
                        last_tag_commit,
                        &package.path,
                        &child_paths,
                        &ImpactConfig {
                            major_types: &config.major_types,
                            minor_types: &config.minor_types,
                            patch_types: &config.patch_types,
                            skip_scopes: &config.skip_scopes,
                            force: true,
                        },
                    )?
                    .expect("forced dependency release has an impact");
                    let new_version = next_version(&old_version, impact)?;
                    let tag = tag_name(&package.path, &new_version)?;
                    println!(
                        "{}: {} -> {} (dependency {})",
                        package_label(&package.path),
                        last_tag,
                        tag,
                        impact.as_str()
                    );
                    added_paths.push(package.path.clone());
                    releases.insert(
                        package.path.clone(),
                        Release {
                            package,
                            last_tag,
                            old_version,
                            new_version,
                            impact: Some(impact),
                            tag,
                        },
                    );
                }
            }

            for path in added_paths {
                let release = releases
                    .get(&path)
                    .expect("new dependency release was inserted");
                add_bumped_package_names(release, &tracked_paths, &mut bumped);
            }
        }
    }

    for release in releases.values() {
        bump_release_targets(&repo, &repo_root, release, &targets, &known_packages)?;
        bump_package_files(&repo, &repo_root, release, &tracked_paths)?;
    }

    if !bumped.is_empty() {
        println!(
            "propagating version bumps for {} package(s) to package.json, package-lock.json, Cargo.toml and Cargo.lock dependencies...",
            bumped.len()
        );
        for abs in &tracked_files {
            let file_name = abs.file_name().and_then(|n| n.to_str()).unwrap_or("");
            let result = match file_name {
                "package.json" => bump_package_json_dependencies(abs, &bumped),
                "package-lock.json" => bump_package_lock_dependencies(abs, &bumped),
                "Cargo.toml" => bump_cargo_toml_dependencies(abs, &bumped),
                "Cargo.lock" => bump_cargo_lock_dependencies(abs, &bumped),
                _ => continue,
            };
            match result {
                Ok(true) => {
                    println!("updated dependencies in {}", abs.display());
                    stage_path(&repo, &repo_root, abs)?;
                }
                Ok(false) => {}
                Err(e) => eprintln!("warning: {e}"),
            }
        }
    }

    if !config.commit {
        println!("skipping commit");
    } else if staged_files(&repo)?.is_empty() {
        println!("no changes to commit");
    } else {
        let message = release_message(releases.values());
        git_commit(&repo, &message)?;
    }

    let tags = releases
        .values()
        .map(|release| release.tag.clone())
        .collect::<Vec<_>>();
    if !config.tag {
        println!("skipping tag");
    } else {
        let message = release_message(releases.values());
        for tag in &tags {
            git_tag(&repo, tag, &message)?;
        }
    }

    if !config.push {
        println!("skipping push");
    } else {
        let branch = current_branch(&repo)?;
        git_push(&repo, &branch, if config.tag { &tags } else { &[] })?;
    }

    Ok(())
}

fn add_bumped_package_names(
    release: &Release,
    tracked_paths: &HashSet<PathBuf>,
    bumped: &mut HashMap<String, (String, String)>,
) {
    for file in package_files(&release.package.root, tracked_paths) {
        if let Some(name) = bumper::bump::package_name_for_file(&file) {
            bumped
                .entry(name)
                .or_insert_with(|| (release.old_version.clone(), release.new_version.clone()));
        }
    }
}

fn bump_release_targets(
    repo: &Repository,
    repo_root: &Path,
    release: &Release,
    targets: &[Target],
    known_packages: &BTreeMap<PathBuf, Package>,
) -> AppResult<()> {
    for target in targets
        .iter()
        .filter(|target| target.package_path == release.package.path)
    {
        if target.path.is_file() {
            bump_file(
                repo,
                repo_root,
                &target.path,
                &release.old_version,
                &release.new_version,
            )?;
        } else {
            bump_dir(
                repo,
                repo_root,
                &target.path,
                &release.package.path,
                known_packages,
                &release.old_version,
                &release.new_version,
            )?;
        }
    }
    Ok(())
}

fn bump_dir(
    repo: &Repository,
    repo_root: &Path,
    directory: &Path,
    package_path: &Path,
    known_packages: &BTreeMap<PathBuf, Package>,
    old_version: &str,
    new_version: &str,
) -> AppResult<()> {
    let files = list_tracked_files_under(repo, repo_root, directory)?;
    for absolute in files {
        if !absolute.is_file() {
            continue;
        }
        let hierarchy = resolve_known_package_hierarchy(repo_root, &absolute, known_packages)?;
        if hierarchy
            .first()
            .is_some_and(|package| package.path != package_path)
        {
            continue;
        }
        let _ = bump_typed_file(repo, repo_root, &absolute, old_version, new_version)?;
    }
    Ok(())
}

fn bump_package_files(
    repo: &Repository,
    repo_root: &Path,
    release: &Release,
    tracked_paths: &HashSet<PathBuf>,
) -> AppResult<()> {
    for file in package_files(&release.package.root, tracked_paths) {
        let _ = bump_typed_file(
            repo,
            repo_root,
            &file,
            &release.old_version,
            &release.new_version,
        )?;
    }
    Ok(())
}

fn package_files(package_root: &Path, tracked_paths: &HashSet<PathBuf>) -> Vec<PathBuf> {
    const MARKERS: &[&str] = &[
        "package.json",
        "Cargo.toml",
        "pyproject.toml",
        "gleam.toml",
        "build.zig.zon",
        "CMakeLists.txt",
        "build.gradle",
        "build.gradle.kts",
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

fn discover_packages(
    repo_root: &Path,
    tracked_files: &[PathBuf],
) -> AppResult<BTreeMap<PathBuf, Package>> {
    let root = fs::canonicalize(repo_root).map_err(|e| {
        format!(
            "failed to resolve repository root '{}': {e}",
            repo_root.display()
        )
    })?;
    let mut packages = BTreeMap::from([(
        PathBuf::new(),
        Package {
            root: root.clone(),
            path: PathBuf::new(),
        },
    )]);
    for file in tracked_files {
        if !is_package_marker(file) {
            continue;
        }
        let Some(directory) = file.parent() else {
            continue;
        };
        let directory = fs::canonicalize(directory).map_err(|e| {
            format!(
                "failed to resolve package directory '{}': {e}",
                directory.display()
            )
        })?;
        let path = directory
            .strip_prefix(&root)
            .map_err(|_| {
                format!(
                    "package directory '{}' is outside repository root '{}'",
                    directory.display(),
                    root.display()
                )
            })?
            .to_path_buf();
        packages.entry(path.clone()).or_insert(Package {
            root: directory,
            path,
        });
    }
    Ok(packages)
}

fn resolve_known_package_hierarchy(
    repo_root: &Path,
    supplied_path: &Path,
    known_packages: &BTreeMap<PathBuf, Package>,
) -> AppResult<Vec<Package>> {
    let repo_root = fs::canonicalize(repo_root).map_err(|e| {
        format!(
            "failed to resolve repository root '{}': {e}",
            repo_root.display()
        )
    })?;
    let supplied_path = fs::canonicalize(supplied_path)
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

fn is_package_ancestor(ancestor: &Path, descendant: &Path) -> bool {
    ancestor.as_os_str().is_empty() || descendant.starts_with(ancestor)
}

fn nearest_parent<'a>(path: &Path, package_paths: &'a [PathBuf]) -> Option<&'a PathBuf> {
    package_paths
        .iter()
        .filter(|candidate| *candidate != path && is_package_ancestor(candidate, path))
        .max_by_key(|candidate| candidate.components().count())
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

fn tag_name(package_path: &Path, version: &str) -> AppResult<String> {
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

fn package_label(path: &Path) -> String {
    if path.as_os_str().is_empty() {
        "root".to_string()
    } else {
        path.display().to_string()
    }
}

fn release_message<'a>(releases: impl Iterator<Item = &'a Release>) -> String {
    let transitions = releases
        .map(|release| format!("{} -> {}", release.last_tag, release.tag))
        .collect::<Vec<_>>()
        .join(", ");
    format!("bump: {transitions}")
}

fn bump_file(
    repo: &Repository,
    repo_root: &Path,
    file: &Path,
    old_version: &str,
    new_version: &str,
) -> AppResult<()> {
    match bump_typed_file(repo, repo_root, file, old_version, new_version)? {
        TypedChange::Changed | TypedChange::Unchanged => return Ok(()),
        TypedChange::Unhandled => {}
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
) -> AppResult<TypedChange> {
    let changed = apply_typed_change(file, old_version, new_version)?;
    if changed == TypedChange::Changed {
        stage_path(repo, repo_root, file)?;
    }
    Ok(changed)
}

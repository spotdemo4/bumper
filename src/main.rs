mod config;
mod git_ops;
mod model;
mod preview;
mod release_plan;
mod versioning;

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::env;
use std::ffi::OsStr;
use std::fs;
use std::io::{self, BufRead, IsTerminal, Write};
use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;

use bumper::bump::{DependencyUpdate, TypedChange, apply_dependency_update, apply_typed_change};
use bumper::package::{Package, is_package_marker};
use git2::Repository;

use config::load_config;
use git_ops::{
    current_branch, ensure_clean_repo, git_commit, git_fetch, git_push, git_tag,
    list_tracked_files_under, repo_root, stage_path, staged_files,
};
use model::AppResult;
use preview::{PreviewInput, collect_bump_preview, preview_colors_enabled, render_bump_preview};
use release_plan::{
    PlanInput, Release, build_release_plan, commit_message, package_files, package_label,
    release_message, resolve_known_package_hierarchy, resolve_known_package_owner,
};

#[derive(Debug)]
struct Target {
    path: PathBuf,
    package_path: PathBuf,
}

#[derive(Debug)]
struct DiscoveredPackages {
    packages: BTreeMap<PathBuf, Package>,
    impact_paths: BTreeMap<PathBuf, Vec<PathBuf>>,
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

fn normalize_repository_relative_path(path: &Path) -> Option<PathBuf> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => normalized.push(part),
            Component::ParentDir if normalized.pop() => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(normalized)
}

fn normalize_ignored_directories(
    repo_root: &Path,
    directories: &[PathBuf],
) -> AppResult<Vec<PathBuf>> {
    let mut normalized = Vec::new();
    for directory in directories {
        let relative = if directory.is_absolute() {
            directory.strip_prefix(repo_root).map_err(|_| {
                format!(
                    "ignored directory '{}' is outside repository root '{}'",
                    directory.display(),
                    repo_root.display()
                )
            })?
        } else {
            directory.as_path()
        };
        let clean = normalize_repository_relative_path(relative).ok_or_else(|| {
            format!(
                "ignored directory '{}' is outside repository root '{}'",
                directory.display(),
                repo_root.display()
            )
        })?;
        let normalized_absolute = repo_root.join(&clean);
        if normalized_absolute.exists() && !normalized_absolute.is_dir() {
            return Err(format!(
                "ignored path '{}' is not a directory",
                directory.display()
            ));
        }
        normalized.push(clean);
    }
    normalized.sort();
    normalized.dedup();
    Ok(normalized)
}

fn run() -> AppResult<()> {
    let mut config = load_config()?;
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
    let ignored_directories =
        normalize_ignored_directories(&repo_root, &config.ignored_directories)?;

    if !config.allow_dirty {
        ensure_clean_repo(&repo)?;
    }

    println!("fetching latest tags from remote...");
    git_fetch(&repo)?;

    let tracked_files =
        list_tracked_files_under(&repo, &repo_root, &repo_root, &ignored_directories)?;
    let discovered_packages = discover_packages(&repo_root, &tracked_files)?;
    let known_packages = discovered_packages.packages;
    let package_impact_paths = discovered_packages.impact_paths;
    config
        .paths
        .extend(known_packages.values().map(|package| package.root.clone()));
    let tracked_paths = tracked_files.iter().cloned().collect::<HashSet<_>>();
    let mut packages = BTreeMap::<PathBuf, Package>::new();
    let mut selected_packages = HashSet::<PathBuf>::new();
    let mut target_paths = HashSet::<PathBuf>::new();
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

        let absolute = fs::canonicalize(&absolute)
            .map_err(|e| format!("failed to resolve '{}': {e}", absolute.display()))?;
        let relative = absolute.strip_prefix(&repo_root).map_err(|_| {
            format!(
                "path '{}' is outside repository root '{}'",
                absolute.display(),
                repo_root.display()
            )
        })?;
        if git_ops::is_ignored_path(relative, &ignored_directories) {
            eprintln!("warning: ignoring selected path: {}", absolute.display());
            continue;
        }
        if !target_paths.insert(absolute.clone()) {
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
    let plan = build_release_plan(PlanInput {
        repo: &repo,
        repo_root: &repo_root,
        packages,
        known_packages: &known_packages,
        selected_packages: &selected_packages,
        tracked_files: &tracked_files,
        tracked_paths: &tracked_paths,
        package_impact_paths: &package_impact_paths,
        ignored_directories: &ignored_directories,
        config: &config,
    })?;
    if plan.releases.is_empty() {
        print_skipped_packages(&plan.skipped_packages);
        println!("no new impactful commits for the selected packages");
        return Ok(());
    }

    print_skipped_packages(&plan.skipped_packages);

    let preview = collect_bump_preview(PreviewInput {
        repo_root: &repo_root,
        plan: &plan,
        targets: &targets,
        known_packages: &known_packages,
        tracked_files: &tracked_files,
        tracked_paths: &tracked_paths,
    })?;
    let no_color = env::var_os("NO_COLOR");
    let term = env::var_os("TERM");
    let color = preview_colors_enabled(
        io::stdout().is_terminal(),
        no_color.as_deref(),
        term.as_deref(),
    );
    print!("{}", render_bump_preview(&plan, &preview, color));
    io::stdout()
        .flush()
        .map_err(|e| format!("failed to write preview: {e}"))?;

    let stdin = io::stdin();
    let interactive = stdin.is_terminal();
    let mut input = stdin.lock();
    let mut output = io::stderr();
    if !confirm_bump(interactive, &mut input, &mut output)? {
        println!("aborted");
        return Ok(());
    }

    for release in plan.releases.values() {
        bump_release_targets(
            &repo,
            &repo_root,
            release,
            &targets,
            &known_packages,
            &ignored_directories,
        )?;
        bump_package_files(&repo, &repo_root, release, &tracked_paths)?;
    }

    let version_bumps = plan.version_bumps();
    if !version_bumps.is_empty() {
        println!(
            "propagating version bumps for {} package(s) to package.json, package-lock.json, Cargo.toml and Cargo.lock dependencies...",
            version_bumps.len()
        );
        for relative in plan.dependency_files.keys() {
            let file = repo_root.join(relative);
            match apply_dependency_update(&file, &version_bumps) {
                Ok(DependencyUpdate::Changed(_)) => {
                    println!("updated dependencies in {}", file.display());
                    stage_path(&repo, &repo_root, &file)?;
                }
                Ok(DependencyUpdate::Unchanged | DependencyUpdate::Unhandled) => {}
                Err(e) => eprintln!("warning: {e}"),
            }
        }
    }

    if !config.commit {
        println!("skipping commit");
    } else if staged_files(&repo)?.is_empty() {
        println!("no changes to commit");
    } else {
        let message = commit_message(plan.releases.values());
        git_commit(&repo, &message)?;
    }

    let tags = plan
        .releases
        .values()
        .map(|release| release.tag.clone())
        .collect::<Vec<_>>();
    if !config.tag {
        println!("skipping tag");
    } else {
        let message = release_message(plan.releases.values());
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

fn print_skipped_packages(skipped_packages: &BTreeSet<PathBuf>) {
    for path in skipped_packages {
        println!("{}: (skipped)", package_label(path));
    }
}

fn confirm_bump<R: BufRead, W: Write>(
    interactive: bool,
    input: &mut R,
    output: &mut W,
) -> AppResult<bool> {
    if !interactive {
        return Ok(true);
    }

    loop {
        write!(output, "Would you like to proceed? [y/N] ")
            .map_err(|e| format!("failed to write confirmation prompt: {e}"))?;
        output
            .flush()
            .map_err(|e| format!("failed to write confirmation prompt: {e}"))?;

        let mut answer = String::new();
        let bytes = input
            .read_line(&mut answer)
            .map_err(|e| format!("failed to read confirmation: {e}"))?;
        if bytes == 0 {
            return Ok(false);
        }
        match answer.trim().to_ascii_lowercase().as_str() {
            "y" | "yes" => return Ok(true),
            "" | "n" | "no" => return Ok(false),
            _ => writeln!(output, "Please answer y or n.")
                .map_err(|e| format!("failed to write confirmation prompt: {e}"))?,
        }
    }
}

fn bump_release_targets(
    repo: &Repository,
    repo_root: &Path,
    release: &Release,
    targets: &[Target],
    known_packages: &BTreeMap<PathBuf, Package>,
    ignored_directories: &[PathBuf],
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
                release,
                known_packages,
                ignored_directories,
            )?;
        }
    }
    Ok(())
}

fn bump_dir(
    repo: &Repository,
    repo_root: &Path,
    directory: &Path,
    release: &Release,
    known_packages: &BTreeMap<PathBuf, Package>,
    ignored_directories: &[PathBuf],
) -> AppResult<()> {
    let files = list_tracked_files_under(repo, repo_root, directory, ignored_directories)?;
    for absolute in files {
        if !absolute.is_file() {
            continue;
        }
        let owner = resolve_known_package_owner(repo_root, &absolute, known_packages)?;
        if owner.path != release.package.path {
            continue;
        }
        let _ = bump_typed_file(
            repo,
            repo_root,
            &absolute,
            &release.old_version,
            &release.new_version,
        )?;
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

fn package_json_impact_paths(manifest: &Path, package_path: &Path) -> AppResult<Vec<PathBuf>> {
    let source = fs::read_to_string(manifest).map_err(|e| {
        format!(
            "failed to read package manifest '{}': {e}",
            manifest.display()
        )
    })?;
    let value = serde_json::from_str::<serde_json::Value>(&source).map_err(|e| {
        format!(
            "failed to parse package manifest '{}': {e}",
            manifest.display()
        )
    })?;
    let Some(bumper) = value.get("bumper") else {
        return Ok(Vec::new());
    };
    let bumper = bumper.as_object().ok_or_else(|| {
        format!(
            "package manifest '{}' field 'bumper' must be an object",
            manifest.display()
        )
    })?;
    let Some(impact_paths) = bumper.get("impactPaths") else {
        return Ok(Vec::new());
    };
    let impact_paths = impact_paths.as_array().ok_or_else(|| {
        format!(
            "package manifest '{}' field 'bumper.impactPaths' must be an array",
            manifest.display()
        )
    })?;

    let mut normalized = Vec::with_capacity(impact_paths.len());
    for (index, value) in impact_paths.iter().enumerate() {
        let field = format!("bumper.impactPaths[{index}]");
        let path = value.as_str().ok_or_else(|| {
            format!(
                "package manifest '{}' field '{field}' must be a string",
                manifest.display()
            )
        })?;
        if path.is_empty() {
            return Err(format!(
                "package manifest '{}' field '{field}' must not be empty",
                manifest.display()
            ));
        }
        let path = Path::new(path);
        if path.is_absolute() {
            return Err(format!(
                "package manifest '{}' field '{field}' must be relative to the package directory",
                manifest.display()
            ));
        }
        let path =
            normalize_repository_relative_path(&package_path.join(path)).ok_or_else(|| {
                format!(
                    "package manifest '{}' field '{field}' escapes the repository root",
                    manifest.display()
                )
            })?;
        if path == package_path {
            return Err(format!(
                "package manifest '{}' field '{field}' resolves to the package directory",
                manifest.display()
            ));
        }
        normalized.push(path);
    }
    normalized.sort();
    normalized.dedup();
    Ok(normalized)
}

fn discover_packages(repo_root: &Path, tracked_files: &[PathBuf]) -> AppResult<DiscoveredPackages> {
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
    let mut impact_paths = BTreeMap::new();
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
        if file.file_name() == Some(OsStr::new("package.json")) {
            impact_paths.insert(path.clone(), package_json_impact_paths(file, &path)?);
        }
        packages.entry(path.clone()).or_insert(Package {
            root: directory,
            path,
        });
    }
    Ok(DiscoveredPackages {
        packages,
        impact_paths,
    })
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_directory(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("bumper-{name}-{nanos}"));
        fs::create_dir_all(&path).expect("create temp directory");
        path
    }

    fn write_package_json(directory: &Path, source: &str) -> PathBuf {
        fs::create_dir_all(directory).expect("create package directory");
        let manifest = directory.join("package.json");
        fs::write(&manifest, source).expect("write package manifest");
        manifest
    }

    fn release(path: &str, last_tag: &str, tag: &str) -> Release {
        Release {
            package: Package {
                root: PathBuf::from(path),
                path: PathBuf::from(path),
            },
            last_tag: last_tag.to_string(),
            old_version: String::new(),
            new_version: String::new(),
            impact: crate::model::Impact::Patch,
            tag: tag.to_string(),
            reasons: crate::release_plan::ReleaseReasons::default(),
        }
    }

    #[test]
    fn single_release_commit_message_includes_version_transition() {
        let releases = [release("", "v1.2.3", "v1.3.0")];

        assert_eq!(commit_message(releases.iter()), "bump: v1.2.3 -> v1.3.0");
    }

    #[test]
    fn multiple_release_commit_message_uses_package_trailers() {
        let releases = [
            release("", "v1.0.0", "v1.0.1"),
            release("packages/app", "packages/app/v3.0.0", "packages/app/v3.0.1"),
            release(
                "packages/library",
                "packages/library/v2.0.0",
                "packages/library/v2.0.1",
            ),
        ];

        assert_eq!(
            commit_message(releases.iter()),
            "bump: v1.0.1\n\nPackage: packages/app/v3.0.1\nPackage: packages/library/v2.0.1"
        );
    }

    #[test]
    fn ignored_directories_are_normalized_relative_to_repository() {
        let root = Path::new("/repo");

        let normalized = normalize_ignored_directories(
            root,
            &[
                PathBuf::from("./generated"),
                PathBuf::from("packages/old/../legacy"),
                PathBuf::from("generated"),
            ],
        )
        .expect("normalize ignored directories");

        assert_eq!(
            normalized,
            vec![PathBuf::from("generated"), PathBuf::from("packages/legacy")]
        );
    }

    #[test]
    fn ignored_directories_cannot_escape_repository() {
        let root = Path::new("/repo");

        assert!(normalize_ignored_directories(root, &[PathBuf::from("../outside")]).is_err());
        assert!(normalize_ignored_directories(root, &[PathBuf::from("/outside")]).is_err());
    }

    #[test]
    fn package_json_impact_paths_accept_missing_empty_and_unknown_fields() {
        let root = temp_directory("impact-path-config");
        let package = root.join("packages/app");
        let manifest = write_package_json(
            &package,
            r#"{"name":"app","version":"1.0.0","custom":true}"#,
        );
        assert_eq!(
            package_json_impact_paths(&manifest, Path::new("packages/app"))
                .expect("parse missing bumper config"),
            Vec::<PathBuf>::new()
        );

        fs::write(
            &manifest,
            r#"{"name":"app","version":"1.0.0","bumper":{"impactPaths":[],"future":true}}"#,
        )
        .expect("write empty impact paths");
        assert_eq!(
            package_json_impact_paths(&manifest, Path::new("packages/app"))
                .expect("parse empty impact paths"),
            Vec::<PathBuf>::new()
        );

        fs::write(
            &manifest,
            r#"{"name":"app","version":"1.0.0","bumper":{"impactPaths":["../../shared/native","future/missing","src/../generated","../../shared/native"],"future":true}}"#,
        )
        .expect("write valid impact paths");
        assert_eq!(
            package_json_impact_paths(&manifest, Path::new("packages/app"))
                .expect("parse valid impact paths"),
            vec![
                PathBuf::from("packages/app/future/missing"),
                PathBuf::from("packages/app/generated"),
                PathBuf::from("shared/native"),
            ]
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn package_json_impact_paths_report_manifest_fields() {
        let root = temp_directory("invalid-impact-path-config");
        let manifest = write_package_json(&root, r#"{"name":"root","version":"1.0.0"}"#);
        for (source, field) in [
            (
                r#"{"name":"root","version":"1.0.0","bumper":true}"#,
                "field 'bumper'",
            ),
            (
                r#"{"name":"root","version":"1.0.0","bumper":{"impactPaths":true}}"#,
                "field 'bumper.impactPaths'",
            ),
            (
                r#"{"name":"root","version":"1.0.0","bumper":{"impactPaths":["src",1]}}"#,
                "field 'bumper.impactPaths[1]'",
            ),
        ] {
            fs::write(&manifest, source).expect("write invalid impact config");
            let error = package_json_impact_paths(&manifest, Path::new(""))
                .expect_err("invalid impact config should fail");
            assert!(error.contains(&manifest.display().to_string()), "{error}");
            assert!(error.contains(field), "{error}");
        }
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn package_json_impact_paths_reject_invalid_normalized_paths() {
        let root = temp_directory("invalid-normalized-impact-paths");
        let manifest = write_package_json(
            &root.join("packages/app"),
            r#"{"name":"app","version":"1.0.0"}"#,
        );
        for (path, message) in [
            ("", "must not be empty"),
            (".", "resolves to the package directory"),
            ("src/..", "resolves to the package directory"),
            ("../../../outside", "escapes the repository root"),
            ("/outside", "must be relative"),
        ] {
            let source = serde_json::json!({
                "name": "app",
                "version": "1.0.0",
                "bumper": {"impactPaths": [path]},
            });
            fs::write(&manifest, source.to_string()).expect("write invalid impact path");
            let error = package_json_impact_paths(&manifest, Path::new("packages/app"))
                .expect_err("invalid normalized path should fail");
            assert!(error.contains("bumper.impactPaths[0]"), "{error}");
            assert!(error.contains(message), "{error}");
        }
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn discovery_keeps_package_json_impact_paths_with_other_markers() {
        let root = temp_directory("impact-path-discovery");
        let package = root.join("packages/app");
        let package_json = write_package_json(
            &package,
            r#"{"name":"app","version":"1.0.0","bumper":{"impactPaths":["../../native"]}}"#,
        );
        let cargo_toml = package.join("Cargo.toml");
        fs::write(
            &cargo_toml,
            "[package]\nname = \"app-rust\"\nversion = \"1.0.0\"\n",
        )
        .expect("write Cargo manifest");

        let discovered = discover_packages(&root, &[cargo_toml, package_json])
            .expect("discover package metadata");

        assert!(discovered.packages.contains_key(Path::new("packages/app")));
        assert_eq!(
            discovered.impact_paths.get(Path::new("packages/app")),
            Some(&vec![PathBuf::from("native")])
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn malformed_root_impact_config_is_not_masked_by_root_package() {
        let root = temp_directory("invalid-root-impact-path");
        let manifest = write_package_json(
            &root,
            r#"{"name":"root","version":"1.0.0","bumper":{"impactPaths":"src"}}"#,
        );

        let error = discover_packages(&root, std::slice::from_ref(&manifest))
            .expect_err("malformed root package config should fail");

        assert!(error.contains(&manifest.display().to_string()), "{error}");
        assert!(error.contains("bumper.impactPaths"), "{error}");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn preview_colors_require_a_capable_terminal() {
        assert_eq!(preview_colors_enabled(true, None, None), cfg!(not(windows)));
        assert!(!preview_colors_enabled(false, None, None));
        assert!(!preview_colors_enabled(true, Some(OsStr::new("1")), None));
        assert!(!preview_colors_enabled(true, Some(OsStr::new("")), None));
        assert!(!preview_colors_enabled(
            true,
            None,
            Some(OsStr::new("dumb"))
        ));
        assert!(!preview_colors_enabled(
            true,
            None,
            Some(OsStr::new("DUMB"))
        ));
    }

    #[test]
    fn interactive_confirmation_accepts_yes_after_invalid_answer() {
        let mut input = b"maybe\ny\n".as_slice();
        let mut output = Vec::new();

        assert!(confirm_bump(true, &mut input, &mut output).expect("confirm bump"));
        assert_eq!(
            String::from_utf8(output).expect("UTF-8 output"),
            concat!(
                "Would you like to proceed? [y/N] ",
                "Please answer y or n.\n",
                "Would you like to proceed? [y/N] ",
            )
        );
    }

    #[test]
    fn interactive_confirmation_defaults_to_no() {
        let mut input = b"\n".as_slice();
        let mut output = Vec::new();

        assert!(!confirm_bump(true, &mut input, &mut output).expect("confirm bump"));
    }

    #[test]
    fn non_interactive_confirmation_does_not_read_or_prompt() {
        let mut input = b"n\n".as_slice();
        let mut output = Vec::new();

        assert!(confirm_bump(false, &mut input, &mut output).expect("confirm bump"));
        assert!(output.is_empty());
        assert_eq!(input, b"n\n");
    }
}

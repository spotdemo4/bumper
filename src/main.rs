mod config;
mod git_ops;
mod model;
mod versioning;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::env;
use std::ffi::{OsStr, OsString};
use std::fmt::Write as _;
use std::fs;
use std::io::{self, BufRead, IsTerminal, Write};
use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;

use bumper::bump::{
    TypedChange, apply_typed_change, bump_cargo_lock_dependencies, bump_cargo_toml_dependencies,
    bump_package_json_dependencies, bump_package_lock_dependencies, dependency_update_needed,
    preview_typed_change,
};
use bumper::package::{Package, is_package_marker, package_version};
use git2::{Oid, Repository};

use config::load_config;
use git_ops::{
    ImpactConfig, current_branch, ensure_clean_repo, get_impact_for_package, git_commit, git_fetch,
    git_push, git_tag, latest_tag_or_none, list_tracked_files_under, repo_root, stage_path,
    staged_files,
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

struct ReleaseBase {
    last_tag: String,
    version: String,
    commit: Option<Oid>,
}

#[derive(Debug)]
struct DiscoveredPackages {
    packages: BTreeMap<PathBuf, Package>,
    impact_paths: BTreeMap<PathBuf, Vec<PathBuf>>,
}

#[derive(Default)]
struct PreviewNode {
    children: BTreeMap<OsString, PreviewNode>,
    entry: Option<PreviewEntry>,
}

#[derive(Clone)]
struct PreviewEntry {
    old_version: String,
    new_version: String,
    package_path: PathBuf,
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
    let package_paths = packages.keys().cloned().collect::<Vec<_>>();
    let mut releases = BTreeMap::<PathBuf, Release>::new();
    for (path, package) in packages {
        let base = resolve_release_base(&repo, &package, &known_packages)?;
        let child_paths = known_packages
            .keys()
            .filter(|child| *child != &path && is_package_ancestor(&path, child))
            .cloned()
            .collect::<Vec<_>>();
        let impact = get_impact_for_package(
            &repo,
            base.commit,
            &path,
            &child_paths,
            &ImpactConfig {
                major_types: &config.major_types,
                minor_types: &config.minor_types,
                patch_types: &config.patch_types,
                skip_scopes: &config.skip_scopes,
                ignored_directories: &ignored_directories,
                impact_paths: package_impact_paths.get(&path).map_or(&[], Vec::as_slice),
                force: config.force && selected_packages.contains(&path),
            },
        )?;
        releases.insert(
            path,
            Release {
                package,
                last_tag: base.last_tag,
                old_version: base.version,
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

    let skipped_packages = releases
        .iter()
        .filter(|(path, release)| selected_packages.contains(*path) && release.impact.is_none())
        .map(|(path, _)| path.clone())
        .collect::<BTreeSet<_>>();
    releases.retain(|_, release| release.impact.is_some());
    if releases.is_empty() {
        print_skipped_packages(&skipped_packages, &releases);
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
                    let base = resolve_release_base(&repo, &package, &known_packages)?;
                    let child_paths = known_packages
                        .keys()
                        .filter(|child| {
                            *child != &package.path && is_package_ancestor(&package.path, child)
                        })
                        .cloned()
                        .collect::<Vec<_>>();
                    let impact = get_impact_for_package(
                        &repo,
                        base.commit,
                        &package.path,
                        &child_paths,
                        &ImpactConfig {
                            major_types: &config.major_types,
                            minor_types: &config.minor_types,
                            patch_types: &config.patch_types,
                            skip_scopes: &config.skip_scopes,
                            ignored_directories: &ignored_directories,
                            impact_paths: package_impact_paths
                                .get(&package.path)
                                .map_or(&[], Vec::as_slice),
                            force: true,
                        },
                    )?
                    .expect("forced dependency release has an impact");
                    let new_version = next_version(&base.version, impact)?;
                    let tag = tag_name(&package.path, &new_version)?;
                    println!(
                        "{}: {} -> {} (dependency {})",
                        package_label(&package.path),
                        base.last_tag,
                        tag,
                        impact.as_str()
                    );
                    added_paths.push(package.path.clone());
                    releases.insert(
                        package.path.clone(),
                        Release {
                            package,
                            last_tag: base.last_tag,
                            old_version: base.version,
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

    print_skipped_packages(&skipped_packages, &releases);

    let files = collect_bump_preview(
        &repo,
        &repo_root,
        &releases,
        &targets,
        &known_packages,
        &ignored_directories,
        &tracked_files,
        &tracked_paths,
        &bumped,
    )?;
    let no_color = env::var_os("NO_COLOR");
    let term = env::var_os("TERM");
    let color = preview_colors_enabled(
        io::stdout().is_terminal(),
        no_color.as_deref(),
        term.as_deref(),
    );
    print!("{}", render_bump_preview(&files, color));
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

    for release in releases.values() {
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
        let message = commit_message(releases.values());
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

fn print_skipped_packages(
    skipped_packages: &BTreeSet<PathBuf>,
    releases: &BTreeMap<PathBuf, Release>,
) {
    for path in skipped_packages {
        if !releases.contains_key(path) {
            println!("{}: (skipped)", package_label(path));
        }
    }
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

#[allow(clippy::too_many_arguments)]
fn collect_bump_preview(
    repo: &Repository,
    repo_root: &Path,
    releases: &BTreeMap<PathBuf, Release>,
    targets: &[Target],
    known_packages: &BTreeMap<PathBuf, Package>,
    ignored_directories: &[PathBuf],
    tracked_files: &[PathBuf],
    tracked_paths: &HashSet<PathBuf>,
    bumped: &HashMap<String, (String, String)>,
) -> AppResult<BTreeMap<PathBuf, PreviewEntry>> {
    let mut files = BTreeMap::new();

    for release in releases.values() {
        for target in targets
            .iter()
            .filter(|target| target.package_path == release.package.path)
        {
            if target.path.is_file() {
                if preview_file_change(&target.path, release, true)? {
                    add_preview_file(&mut files, repo_root, &target.path, release)?;
                }
                continue;
            }

            for file in
                list_tracked_files_under(repo, repo_root, &target.path, ignored_directories)?
            {
                if !file.is_file() {
                    continue;
                }
                let hierarchy = resolve_known_package_hierarchy(repo_root, &file, known_packages)?;
                if hierarchy
                    .first()
                    .is_some_and(|package| package.path != release.package.path)
                {
                    continue;
                }
                if preview_file_change(&file, release, false)? {
                    add_preview_file(&mut files, repo_root, &file, release)?;
                }
            }
        }

        for file in package_files(&release.package.root, tracked_paths) {
            if preview_file_change(&file, release, false)? {
                add_preview_file(&mut files, repo_root, &file, release)?;
            }
        }
    }

    if !bumped.is_empty() {
        for file in tracked_files {
            match dependency_update_needed(file, bumped) {
                Ok(true) => {
                    let owner = resolve_known_package_hierarchy(repo_root, file, known_packages)?
                        .into_iter()
                        .next()
                        .expect("known package hierarchy contains root");
                    if let Some(release) = releases.get(&owner.path) {
                        add_preview_file(&mut files, repo_root, file, release)?;
                    }
                }
                Ok(false) => {}
                Err(e) => eprintln!("warning: {e}"),
            }
        }
    }

    Ok(files)
}

fn preview_file_change(file: &Path, release: &Release, replace_unhandled: bool) -> AppResult<bool> {
    match preview_typed_change(file, &release.old_version, &release.new_version)? {
        TypedChange::Changed => Ok(true),
        TypedChange::Unchanged => Ok(false),
        TypedChange::Unhandled if !replace_unhandled => Ok(false),
        TypedChange::Unhandled => {
            let source = fs::read_to_string(file)
                .map_err(|e| format!("failed to read '{}': {e}", file.display()))?;
            if source.contains(&release.old_version) {
                Ok(true)
            } else {
                Err(format!("no occurrences found in {}", file.display()))
            }
        }
    }
}

fn add_preview_file(
    files: &mut BTreeMap<PathBuf, PreviewEntry>,
    repo_root: &Path,
    file: &Path,
    release: &Release,
) -> AppResult<()> {
    let relative = file.strip_prefix(repo_root).map_err(|_| {
        format!(
            "file '{}' is outside repository root '{}'",
            file.display(),
            repo_root.display()
        )
    })?;
    files.insert(
        relative.to_path_buf(),
        PreviewEntry {
            old_version: release.old_version.clone(),
            new_version: release.new_version.clone(),
            package_path: release.package.path.clone(),
        },
    );
    Ok(())
}

fn preview_colors_enabled(
    stdout_is_terminal: bool,
    no_color: Option<&OsStr>,
    term: Option<&OsStr>,
) -> bool {
    cfg!(not(windows))
        && stdout_is_terminal
        && no_color.is_none()
        && term
            .and_then(OsStr::to_str)
            .is_none_or(|term| !term.eq_ignore_ascii_case("dumb"))
}

fn render_bump_preview(files: &BTreeMap<PathBuf, PreviewEntry>, color: bool) -> String {
    const PACKAGE_COLORS: &[&str] = &[
        "\x1b[36m", "\x1b[33m", "\x1b[32m", "\x1b[35m", "\x1b[34m", "\x1b[31m", "\x1b[96m",
        "\x1b[93m", "\x1b[92m", "\x1b[95m", "\x1b[94m", "\x1b[91m",
    ];

    let package_paths = files
        .values()
        .map(|entry| entry.package_path.clone())
        .collect::<BTreeSet<_>>();
    let package_colors = package_paths
        .into_iter()
        .enumerate()
        .map(|(index, path)| (path, PACKAGE_COLORS[index % PACKAGE_COLORS.len()]))
        .collect::<BTreeMap<_, _>>();
    let mut root = PreviewNode::default();
    for (path, entry) in files {
        let mut node = &mut root;
        for component in path.components() {
            node = node
                .children
                .entry(component.as_os_str().to_os_string())
                .or_default();
        }
        node.entry = Some(entry.clone());
    }

    let mut output = String::from("Files to bump:\n");
    write_preview_node_name(
        &mut output,
        ".",
        package_colors.get(Path::new("")).copied(),
        color,
    );
    output.push('\n');
    render_preview_children(
        &root,
        "",
        Path::new(""),
        &package_colors,
        color,
        &mut output,
    );
    output
}

fn render_preview_children(
    node: &PreviewNode,
    prefix: &str,
    path: &Path,
    package_colors: &BTreeMap<PathBuf, &str>,
    color: bool,
    output: &mut String,
) {
    let child_count = node.children.len();
    for (index, (name, child)) in node.children.iter().enumerate() {
        let last = index + 1 == child_count;
        let connector = if last { "`-- " } else { "|-- " };
        let child_path = path.join(name);
        let package_color = package_colors
            .iter()
            .filter(|(package_path, _)| {
                package_path.as_os_str().is_empty() || child_path.starts_with(package_path)
            })
            .max_by_key(|(package_path, _)| package_path.components().count())
            .map(|(_, color)| *color);
        let _ = write!(output, "{prefix}{connector}");
        let mut label = name.to_string_lossy().into_owned();
        if let Some(entry) = &child.entry {
            let _ = write!(label, " ({} -> {})", entry.old_version, entry.new_version);
        }
        write_preview_node_name(output, &label, package_color, color);
        output.push('\n');

        let child_prefix = format!("{prefix}{}", if last { "    " } else { "|   " });
        render_preview_children(
            child,
            &child_prefix,
            &child_path,
            package_colors,
            color,
            output,
        );
    }
}

fn write_preview_node_name(output: &mut String, label: &str, ansi: Option<&str>, color: bool) {
    if color && let Some(ansi) = ansi {
        let _ = write!(output, "{ansi}{label}\x1b[0m");
    } else {
        output.push_str(label);
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
        let hierarchy = resolve_known_package_hierarchy(repo_root, &absolute, known_packages)?;
        if hierarchy
            .first()
            .is_some_and(|package| package.path != release.package.path)
        {
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

fn commit_message<'a>(releases: impl Iterator<Item = &'a Release>) -> String {
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
            impact: None,
            tag: tag.to_string(),
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

        let error = discover_packages(&root, &[manifest.clone()])
            .expect_err("malformed root package config should fail");

        assert!(error.contains(&manifest.display().to_string()), "{error}");
        assert!(error.contains("bumper.impactPaths"), "{error}");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn bump_preview_renders_file_hierarchy_and_versions() {
        let files = BTreeMap::from([
            (
                PathBuf::from("Cargo.toml"),
                PreviewEntry {
                    old_version: "1.2.3".to_string(),
                    new_version: "1.3.0".to_string(),
                    package_path: PathBuf::new(),
                },
            ),
            (
                PathBuf::from("packages/app/Cargo.lock"),
                PreviewEntry {
                    old_version: "2.0.0".to_string(),
                    new_version: "2.0.1".to_string(),
                    package_path: PathBuf::from("packages/app"),
                },
            ),
            (
                PathBuf::from("packages/app/Cargo.toml"),
                PreviewEntry {
                    old_version: "2.0.0".to_string(),
                    new_version: "2.0.1".to_string(),
                    package_path: PathBuf::from("packages/app"),
                },
            ),
        ]);

        assert_eq!(
            render_bump_preview(&files, false),
            concat!(
                "Files to bump:\n",
                ".\n",
                "|-- Cargo.toml (1.2.3 -> 1.3.0)\n",
                "`-- packages\n",
                "    `-- app\n",
                "        |-- Cargo.lock (2.0.0 -> 2.0.1)\n",
                "        `-- Cargo.toml (2.0.0 -> 2.0.1)\n",
            )
        );
    }

    #[test]
    fn bump_preview_colors_paths_by_nearest_package() {
        let files = BTreeMap::from([
            (
                PathBuf::from("Cargo.toml"),
                PreviewEntry {
                    old_version: "1.2.3".to_string(),
                    new_version: "1.3.0".to_string(),
                    package_path: PathBuf::new(),
                },
            ),
            (
                PathBuf::from("packages/app/Cargo.toml"),
                PreviewEntry {
                    old_version: "2.0.0".to_string(),
                    new_version: "2.0.1".to_string(),
                    package_path: PathBuf::from("packages/app"),
                },
            ),
            (
                PathBuf::from("packages/library/README.md"),
                PreviewEntry {
                    old_version: "3.0.0".to_string(),
                    new_version: "3.1.0".to_string(),
                    package_path: PathBuf::from("packages/library"),
                },
            ),
            (
                PathBuf::from("packages/library/plugin/Cargo.toml"),
                PreviewEntry {
                    old_version: "4.0.0".to_string(),
                    new_version: "4.0.1".to_string(),
                    package_path: PathBuf::from("packages/library/plugin"),
                },
            ),
        ]);

        assert_eq!(
            render_bump_preview(&files, true),
            concat!(
                "Files to bump:\n",
                "\x1b[36m.\x1b[0m\n",
                "|-- \x1b[36mCargo.toml (1.2.3 -> 1.3.0)\x1b[0m\n",
                "`-- \x1b[36mpackages\x1b[0m\n",
                "    |-- \x1b[33mapp\x1b[0m\n",
                "    |   `-- \x1b[33mCargo.toml (2.0.0 -> 2.0.1)\x1b[0m\n",
                "    `-- \x1b[32mlibrary\x1b[0m\n",
                "        |-- \x1b[32mREADME.md (3.0.0 -> 3.1.0)\x1b[0m\n",
                "        `-- \x1b[35mplugin\x1b[0m\n",
                "            `-- \x1b[35mCargo.toml (4.0.0 -> 4.0.1)\x1b[0m\n",
            )
        );
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

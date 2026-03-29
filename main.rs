use git2::{Oid, Repository, StatusOptions};
use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

type AppResult<T> = Result<T, String>;

#[derive(Debug, Clone)]
struct Config {
    paths: Vec<PathBuf>,
    major_types: HashSet<String>,
    minor_types: HashSet<String>,
    patch_types: HashSet<String>,
    skip_scopes: HashSet<String>,
    commit: bool,
    tag: bool,
    push: bool,
    force: bool,
    allow_dirty: bool,
    ci: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Impact {
    Patch,
    Minor,
    Major,
}

impl Impact {
    fn as_str(self) -> &'static str {
        match self {
            Impact::Patch => "patch",
            Impact::Minor => "minor",
            Impact::Major => "major",
        }
    }
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
    let repo = Repository::discover(".").map_err(|e| format!("not a git repository: {e}"))?;
    let repo_root = repo_root(&repo)?;

    if config.paths.is_empty() || config.ci {
        config.paths = vec![repo_root.clone()];
    }

    if !config.allow_dirty {
        ensure_clean_repo(&repo)?;
    }

    run_git_command(&repo_root, &["fetch", "--all", "--tags", "--quiet"])?;

    let branch = current_branch(&repo)?;
    let (last_tag_name, last_tag_commit) = latest_tag(&repo)?;
    let last_version = last_tag_name.trim_start_matches('v').to_string();

    let impact = get_impact(
        &repo,
        last_tag_commit,
        &config.major_types,
        &config.minor_types,
        &config.patch_types,
        &config.skip_scopes,
        config.force,
    )?;

    let Some(impact) = impact else {
        println!("no new impactful commits since last tag (v{last_version})");
        return Ok(());
    };

    println!("impact: {}", impact.as_str());
    let next_version = next_version(&last_version, impact)?;
    println!("v{last_version} -> v{next_version}");

    for path in &config.paths {
        let absolute = if path.is_absolute() {
            path.clone()
        } else {
            repo_root.join(path)
        };

        if absolute.is_file() {
            bump_file(&repo, &repo_root, &absolute, &last_version, &next_version)?;
        } else if absolute.is_dir() {
            bump_dir(&repo, &repo_root, &absolute, &last_version, &next_version)?;
        } else {
            eprintln!(
                "warning: file or directory not found: {}",
                absolute.display()
            );
        }
    }

    if !config.commit {
        println!("skipping commit");
    } else if staged_files(&repo)?.is_empty() {
        println!("no changes to commit");
    } else {
        let message = format!("bump: v{last_version} -> v{next_version}");
        run_git_command(&repo_root, &["commit", "-m", &message])?;
    }

    if !config.tag {
        println!("skipping tag");
    } else {
        let message = format!("bump: v{last_version} -> v{next_version}");
        let tag = format!("v{next_version}");
        run_git_command(&repo_root, &["tag", "-a", &tag, "-m", &message])?;
    }

    if !config.push {
        println!("skipping push");
    } else {
        let tag = format!("v{next_version}");
        run_git_command(&repo_root, &["push", "--atomic", "origin", &branch, &tag])?;
    }

    Ok(())
}

fn load_config() -> Config {
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

fn repo_root(repo: &Repository) -> AppResult<PathBuf> {
    let path = repo
        .workdir()
        .ok_or_else(|| "bare repositories are not supported".to_string())?;
    Ok(path.to_path_buf())
}

fn ensure_clean_repo(repo: &Repository) -> AppResult<()> {
    let mut opts = StatusOptions::new();
    opts.include_untracked(false)
        .include_ignored(false)
        .renames_head_to_index(true)
        .renames_index_to_workdir(true);

    let statuses = repo
        .statuses(Some(&mut opts))
        .map_err(|e| format!("failed to read git status: {e}"))?;

    if statuses.is_empty() {
        Ok(())
    } else {
        Err("please commit or stash changes before running bumper".to_string())
    }
}

fn current_branch(repo: &Repository) -> AppResult<String> {
    let head = repo.head().map_err(|e| format!("not on a branch: {e}"))?;
    let shorthand = head
        .shorthand()
        .ok_or_else(|| "not on a branch".to_string())?;
    Ok(shorthand.to_string())
}

fn latest_tag(repo: &Repository) -> AppResult<(String, Oid)> {
    let tags = repo
        .tag_names(None)
        .map_err(|e| format!("failed to list tags: {e}"))?;

    let mut latest: Option<(String, Oid, i64)> = None;

    for maybe_name in tags.iter() {
        let Some(name) = maybe_name else {
            continue;
        };

        let object = repo
            .revparse_single(&format!("refs/tags/{name}"))
            .or_else(|_| repo.revparse_single(name))
            .map_err(|e| format!("failed to read tag '{name}': {e}"))?;

        let commit = object
            .peel_to_commit()
            .map_err(|e| format!("tag '{name}' does not reference a commit: {e}"))?;
        let time = commit.time().seconds();

        match &latest {
            Some((_, _, latest_time)) if *latest_time >= time => {}
            _ => latest = Some((name.to_string(), commit.id(), time)),
        }
    }

    match latest {
        Some((name, oid, _)) => Ok((name, oid)),
        None => Err("no git tags found, please create a tag first".to_string()),
    }
}

fn get_impact(
    repo: &Repository,
    last_tag_commit: Oid,
    major_types: &HashSet<String>,
    minor_types: &HashSet<String>,
    patch_types: &HashSet<String>,
    skip_scopes: &HashSet<String>,
    force: bool,
) -> AppResult<Option<Impact>> {
    let mut impact = if force { Some(Impact::Patch) } else { None };

    let mut walk = repo
        .revwalk()
        .map_err(|e| format!("failed to create revwalk: {e}"))?;
    walk.push_head()
        .map_err(|e| format!("failed to walk from HEAD: {e}"))?;
    walk.hide(last_tag_commit)
        .map_err(|e| format!("failed to hide last tag commit: {e}"))?;

    for oid in walk {
        let oid = oid.map_err(|e| format!("failed to walk commit history: {e}"))?;
        let commit = repo
            .find_commit(oid)
            .map_err(|e| format!("failed to load commit {oid}: {e}"))?;

        let Some(summary) = commit.summary() else {
            continue;
        };

        let Some((prefix, _)) = summary.split_once(':') else {
            continue;
        };

        let typ = prefix.split('(').next().unwrap_or(prefix).trim();
        let mut scope = "none";
        if let Some(start) = prefix.find('(')
            && let Some(end) = prefix[start + 1..].find(')')
        {
            scope = &prefix[start + 1..start + 1 + end];
        }

        if skip_scopes.contains(&scope.trim().to_ascii_lowercase()) {
            continue;
        }

        if prefix.trim_end().ends_with('!') || major_types.contains(&typ.to_ascii_lowercase()) {
            impact = Some(Impact::Major);
            break;
        }

        if minor_types.contains(&typ.to_ascii_lowercase()) {
            if impact.unwrap_or(Impact::Patch) < Impact::Minor {
                impact = Some(Impact::Minor);
            }
            continue;
        }

        if patch_types.contains(&typ.to_ascii_lowercase()) && impact.is_none() {
            impact = Some(Impact::Patch);
        }
    }

    Ok(impact)
}

fn next_version(current: &str, impact: Impact) -> AppResult<String> {
    let mut parts = current.split('.');
    let mut major = parts
        .next()
        .ok_or_else(|| format!("invalid version '{current}'"))?
        .parse::<u64>()
        .map_err(|_| format!("invalid version '{current}'"))?;
    let mut minor = parts
        .next()
        .ok_or_else(|| format!("invalid version '{current}'"))?
        .parse::<u64>()
        .map_err(|_| format!("invalid version '{current}'"))?;
    let mut patch = parts
        .next()
        .ok_or_else(|| format!("invalid version '{current}'"))?
        .parse::<u64>()
        .map_err(|_| format!("invalid version '{current}'"))?;

    match impact {
        Impact::Major => {
            major += 1;
            minor = 0;
            patch = 0;
        }
        Impact::Minor => {
            minor += 1;
            patch = 0;
        }
        Impact::Patch => patch += 1,
    }

    Ok(format!("{major}.{minor}.{patch}"))
}

fn bump_dir(
    repo: &Repository,
    repo_root: &Path,
    directory: &Path,
    old_version: &str,
    new_version: &str,
) -> AppResult<()> {
    let files = list_tracked_files_under(repo, repo_root, directory)?;

    for absolute in files {
        if !absolute.is_file() {
            continue;
        }
        let _ = bump_typed_file(repo, repo_root, &absolute, old_version, new_version)?;
    }

    Ok(())
}

fn list_tracked_files_under(
    repo: &Repository,
    repo_root: &Path,
    directory: &Path,
) -> AppResult<Vec<PathBuf>> {
    let dir_relative = directory.strip_prefix(repo_root).unwrap_or(directory);

    let index = repo
        .index()
        .map_err(|e| format!("failed to read git index: {e}"))?;

    let mut files = Vec::new();
    for entry in index.iter() {
        let Ok(path) = std::str::from_utf8(&entry.path) else {
            continue;
        };
        let relative = Path::new(path);
        if relative.starts_with(dir_relative) {
            files.push(repo_root.join(relative));
        }
    }

    files.sort();
    files.dedup();
    Ok(files)
}

fn bump_file(
    repo: &Repository,
    repo_root: &Path,
    file: &Path,
    old_version: &str,
    new_version: &str,
) -> AppResult<()> {
    if bump_typed_file(repo, repo_root, file, old_version, new_version)? {
        return Ok(());
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
) -> AppResult<bool> {
    let name = file
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| format!("invalid file path '{}'", file.display()))?;

    let changed = match name {
        "flake.nix" | "flake.lock" => replace_literal(file, old_version, new_version)?,
        "package.json" => bump_package_json(file, new_version)?,
        "package-lock.json" => bump_package_lock_json(file, new_version)?,
        "Cargo.toml" => bump_toml_path(file, &["package", "version"], new_version)?,
        "pyproject.toml" => bump_toml_path(file, &["project", "version"], new_version)?,
        "uv.lock" => replace_literal(file, old_version, new_version)?,
        "Cargo.lock" => replace_literal(file, old_version, new_version)?,
        "build.zig.zon" => replace_line_value(file, ".version", new_version)?,
        _ => return Ok(false),
    };

    if changed {
        stage_path(repo, repo_root, file)?;
    }

    Ok(changed)
}

fn replace_literal(file: &Path, old_version: &str, new_version: &str) -> AppResult<bool> {
    let source = fs::read_to_string(file)
        .map_err(|e| format!("failed to read '{}': {e}", file.display()))?;
    let replaced = source.replace(old_version, new_version);
    if source == replaced {
        return Ok(false);
    }

    fs::write(file, replaced).map_err(|e| format!("failed to write '{}': {e}", file.display()))?;
    Ok(true)
}

fn replace_line_value(file: &Path, key: &str, new_version: &str) -> AppResult<bool> {
    let source = fs::read_to_string(file)
        .map_err(|e| format!("failed to read '{}': {e}", file.display()))?;
    let mut changed = false;
    let mut output = Vec::new();

    for line in source.lines() {
        if line.trim_start().starts_with(&format!("{key} = \"")) {
            let indent = line
                .chars()
                .take_while(|c| c.is_whitespace())
                .collect::<String>();
            output.push(format!("{indent}{key} = \"{new_version}\","));
            changed = true;
        } else {
            output.push(line.to_string());
        }
    }

    if !changed {
        return Ok(false);
    }

    let mut written = output.join("\n");
    if source.ends_with('\n') {
        written.push('\n');
    }

    fs::write(file, written).map_err(|e| format!("failed to write '{}': {e}", file.display()))?;
    Ok(true)
}

fn bump_package_json(file: &Path, new_version: &str) -> AppResult<bool> {
    let source = fs::read_to_string(file)
        .map_err(|e| format!("failed to read '{}': {e}", file.display()))?;
    let mut parsed = json::parse(&source)
        .map_err(|e| format!("failed to parse JSON '{}': {e}", file.display()))?;

    if parsed["version"].as_str() == Some(new_version) {
        return Ok(false);
    }

    parsed["version"] = json::JsonValue::String(new_version.to_string());
    fs::write(file, parsed.pretty(2))
        .map_err(|e| format!("failed to write '{}': {e}", file.display()))?;
    Ok(true)
}

fn bump_package_lock_json(file: &Path, new_version: &str) -> AppResult<bool> {
    let source = fs::read_to_string(file)
        .map_err(|e| format!("failed to read '{}': {e}", file.display()))?;
    let mut parsed = json::parse(&source)
        .map_err(|e| format!("failed to parse JSON '{}': {e}", file.display()))?;

    let mut changed = false;
    if parsed["version"].as_str() != Some(new_version) {
        parsed["version"] = json::JsonValue::String(new_version.to_string());
        changed = true;
    }

    if parsed["packages"][""].is_object()
        && parsed["packages"][""]["version"].as_str() != Some(new_version)
    {
        parsed["packages"][""]["version"] = json::JsonValue::String(new_version.to_string());
        changed = true;
    }

    if !changed {
        return Ok(false);
    }

    fs::write(file, parsed.pretty(2))
        .map_err(|e| format!("failed to write '{}': {e}", file.display()))?;
    Ok(true)
}

fn bump_toml_path(file: &Path, path: &[&str], new_version: &str) -> AppResult<bool> {
    let source = fs::read_to_string(file)
        .map_err(|e| format!("failed to read '{}': {e}", file.display()))?;
    let mut parsed = match source.parse::<toml::Value>() {
        Ok(parsed) => parsed,
        Err(_) => {
            if path.len() == 2 {
                return replace_toml_section_key_line(file, &source, path[0], path[1], new_version);
            }
            return Ok(false);
        }
    };

    let mut target = &mut parsed;
    for key in path.iter().take(path.len() - 1) {
        let Some(next) = target.get_mut(*key) else {
            return Ok(false);
        };
        target = next;
    }

    let leaf = path[path.len() - 1];
    let Some(value) = target.get_mut(leaf) else {
        return Ok(false);
    };

    if value.as_str() == Some(new_version) {
        return Ok(false);
    }

    *value = toml::Value::String(new_version.to_string());
    fs::write(file, parsed.to_string())
        .map_err(|e| format!("failed to write '{}': {e}", file.display()))?;
    Ok(true)
}

fn replace_toml_section_key_line(
    file: &Path,
    source: &str,
    section: &str,
    key: &str,
    new_version: &str,
) -> AppResult<bool> {
    let mut in_section = false;
    let mut changed = false;
    let mut output = Vec::new();

    for line in source.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            in_section = &trimmed[1..trimmed.len() - 1] == section;
            output.push(line.to_string());
            continue;
        }

        if in_section && trimmed.starts_with(&format!("{key} = \"")) {
            let indent = line
                .chars()
                .take_while(|c| c.is_whitespace())
                .collect::<String>();
            output.push(format!("{indent}{key} = \"{new_version}\""));
            changed = true;
        } else {
            output.push(line.to_string());
        }
    }

    if !changed {
        return Ok(false);
    }

    let mut written = output.join("\n");
    if source.ends_with('\n') {
        written.push('\n');
    }

    fs::write(file, written).map_err(|e| format!("failed to write '{}': {e}", file.display()))?;
    Ok(true)
}

fn stage_path(repo: &Repository, repo_root: &Path, absolute_path: &Path) -> AppResult<()> {
    let relative = absolute_path
        .strip_prefix(repo_root)
        .map_err(|_| format!("file is outside repository: {}", absolute_path.display()))?;

    let mut index = repo
        .index()
        .map_err(|e| format!("failed to open git index: {e}"))?;
    index
        .add_path(relative)
        .map_err(|e| format!("failed to stage '{}': {e}", relative.display()))?;
    index
        .write()
        .map_err(|e| format!("failed to write git index: {e}"))
}

fn staged_files(repo: &Repository) -> AppResult<Vec<PathBuf>> {
    let mut opts = StatusOptions::new();
    opts.include_untracked(false)
        .include_ignored(false)
        .renames_head_to_index(true)
        .renames_index_to_workdir(true);

    let statuses = repo
        .statuses(Some(&mut opts))
        .map_err(|e| format!("failed to read git status: {e}"))?;

    let staged = statuses
        .iter()
        .filter_map(|entry| {
            let status = entry.status();
            let indexed = status.is_index_new()
                || status.is_index_modified()
                || status.is_index_deleted()
                || status.is_index_renamed()
                || status.is_index_typechange();
            if indexed {
                entry.path().map(PathBuf::from)
            } else {
                None
            }
        })
        .collect();

    Ok(staged)
}

fn run_git_command(repo_root: &Path, args: &[&str]) -> AppResult<()> {
    let status = Command::new("git")
        .current_dir(repo_root)
        .args(args)
        .status()
        .map_err(|e| format!("failed to run git {}: {e}", args.join(" ")))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("git {} failed", args.join(" ")))
    }
}

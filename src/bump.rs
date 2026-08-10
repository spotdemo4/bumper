use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use regex::Regex;
use serde_json::Value;

type AppResult<T> = Result<T, String>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypedChange {
    Changed,
    Unchanged,
    Unhandled,
}

impl TypedChange {
    fn from_changed(changed: bool) -> Self {
        if changed {
            Self::Changed
        } else {
            Self::Unchanged
        }
    }
}

pub fn apply_typed_change(
    file: &Path,
    old_version: &str,
    new_version: &str,
) -> AppResult<TypedChange> {
    evaluate_typed_change(file, old_version, new_version, true)
}

/// Returns the change a typed writer would make without modifying the filesystem.
pub fn preview_typed_change(
    file: &Path,
    old_version: &str,
    new_version: &str,
) -> AppResult<TypedChange> {
    evaluate_typed_change(file, old_version, new_version, false)
}

fn evaluate_typed_change(
    file: &Path,
    old_version: &str,
    new_version: &str,
    write: bool,
) -> AppResult<TypedChange> {
    let name = file
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| format!("invalid file path '{}'", file.display()))?;

    let changed = match name {
        "README.md" => replace_literal(file, old_version, new_version, write),
        "action.yaml" => replace_literal(file, old_version, new_version, write),
        "action.yml" => replace_literal(file, old_version, new_version, write),
        "package.json" => bump_package_json(file, new_version, write),
        "package-lock.json" => bump_package_lock_json(file, new_version, write),
        "build.gradle" => bump_gradle_build(file, old_version, new_version, write),
        "build.gradle.kts" => bump_gradle_build(file, old_version, new_version, write),
        "gradle.properties" => bump_gradle_properties(file, old_version, new_version, write),
        "CMakeLists.txt" => bump_cmake_lists(file, new_version, write),
        "Cargo.toml" => bump_toml_path(file, &["package", "version"], new_version, write),
        "pyproject.toml" => bump_toml_path(file, &["project", "version"], new_version, write),
        "uv.lock" => {
            let name = read_toml_name(&file.with_file_name("pyproject.toml"), "project")?;
            bump_package_in_lock(file, old_version, new_version, &name, write)
        }
        "Cargo.lock" => {
            let name = read_toml_name(&file.with_file_name("Cargo.toml"), "package")?;
            bump_package_in_lock(file, old_version, new_version, &name, write)
        }
        "build.zig.zon" => replace_line_value(file, ".version", new_version, write),
        "gleam.toml" => bump_toml_path(file, &["version"], new_version, write),
        "go.mod" => Ok(false),
        _ if name.ends_with(".nix") => bump_nix_version(file, old_version, new_version, write),
        _ => return Ok(TypedChange::Unhandled),
    }?;

    Ok(TypedChange::from_changed(changed))
}

pub fn package_name_for_file(file: &Path) -> Option<String> {
    let name = file.file_name().and_then(|n| n.to_str())?;
    match name {
        "package.json" | "package-lock.json" => read_package_json_name(file),
        "Cargo.toml" => read_toml_name(file, "package").ok(),
        "pyproject.toml" => read_toml_name(file, "project").ok(),
        "Cargo.lock" => {
            let cargo_toml = file.with_file_name("Cargo.toml");
            read_toml_name(&cargo_toml, "package").ok()
        }
        "uv.lock" => {
            let pyproject = file.with_file_name("pyproject.toml");
            read_toml_name(&pyproject, "project").ok()
        }
        _ => None,
    }
}

fn read_package_json_name(path: &Path) -> Option<String> {
    let source = fs::read_to_string(path).ok()?;
    let value: Value = serde_json::from_str(&source).ok()?;
    value.get("name")?.as_str().map(|s| s.to_string())
}

/// Returns whether a dependency propagation writer would update `file` without
/// modifying the filesystem. Unsupported filenames return `false`.
pub fn dependency_update_needed(
    file: &Path,
    bumped: &HashMap<String, (String, String)>,
) -> AppResult<bool> {
    Ok(dependency_update_content(file, bumped)?.is_some())
}

fn dependency_update_content(
    file: &Path,
    bumped: &HashMap<String, (String, String)>,
) -> AppResult<Option<String>> {
    if bumped.is_empty() {
        return Ok(None);
    }

    let Some(file_name) = file.file_name().and_then(|name| name.to_str()) else {
        return Ok(None);
    };
    if !matches!(
        file_name,
        "package.json" | "package-lock.json" | "Cargo.toml" | "Cargo.lock"
    ) {
        return Ok(None);
    }

    let source = fs::read_to_string(file)
        .map_err(|e| format!("failed to read '{}': {e}", file.display()))?;
    match file_name {
        "package.json" => Ok(transform_package_json_dependencies(&source, bumped)),
        "package-lock.json" => transform_package_lock_dependencies(&source, bumped)
            .map_err(|e| format!("failed to serialize '{}': {e}", file.display())),
        "Cargo.toml" => Ok(transform_cargo_toml_dependencies(&source, bumped)),
        "Cargo.lock" => Ok(transform_cargo_lock_dependencies(&source, bumped)),
        _ => unreachable!(),
    }
}

fn write_dependency_update(
    file: &Path,
    bumped: &HashMap<String, (String, String)>,
    expected_file_name: &str,
) -> AppResult<bool> {
    let file_name = file
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    if file_name != expected_file_name {
        return Ok(false);
    }

    let Some(content) = dependency_update_content(file, bumped)? else {
        return Ok(false);
    };
    fs::write(file, content).map_err(|e| format!("failed to write '{}': {e}", file.display()))?;
    Ok(true)
}

/// Second-pass bump: for a `package.json` file, update any `dependencies`,
/// `devDependencies`, `peerDependencies`, `optionalDependencies`,
/// `bundledDependencies`, `bundleDependencies`, `overrides`, or
/// `resolutions` entries whose version string contains `old_version`
/// for a package name that was bumped in the first pass.
///
/// Preserves original formatting by operating on raw text with regexes
/// rather than re-serializing JSON.
pub fn bump_package_json_dependencies(
    file: &Path,
    bumped: &HashMap<String, (String, String)>,
) -> AppResult<bool> {
    write_dependency_update(file, bumped, "package.json")
}

fn transform_package_json_dependencies(
    source: &str,
    bumped: &HashMap<String, (String, String)>,
) -> Option<String> {
    // Parse to determine which dependency names actually appear in
    // dependency sections and whose version contains the old string.
    let value: Value = match serde_json::from_str(source) {
        Ok(v) => v,
        Err(_) => return None,
    };
    let Value::Object(map) = value else {
        return None;
    };

    const DEP_SECTIONS: &[&str] = &[
        "dependencies",
        "devDependencies",
        "peerDependencies",
        "optionalDependencies",
        "bundledDependencies",
        "bundleDependencies",
        "overrides",
        "resolutions",
    ];

    let mut to_update: HashSet<String> = HashSet::new();
    for (pkg_name, (old_version, _new_version)) in bumped {
        for section in DEP_SECTIONS {
            if let Some(Value::Object(deps)) = map.get(*section)
                && let Some(Value::String(ver)) = deps.get(pkg_name)
                && ver.contains(old_version)
            {
                to_update.insert(pkg_name.clone());
                break;
            }
        }
        // Also handle `overrides`/`resolutions` that may be nested one level deeper?
        // For simplicity, if not found at top-level, scan recursively for any object value
        // that contains the package name as key with a string version containing old_version.
        if !to_update.contains(pkg_name)
            && json_contains_dep(&Value::Object(map.clone()), pkg_name, old_version)
        {
            to_update.insert(pkg_name.clone());
        }
    }

    if to_update.is_empty() {
        return None;
    }

    let mut content = source.to_string();
    let mut changed_any = false;

    for pkg_name in to_update {
        let Some((old_version, new_version)) = bumped.get(&pkg_name) else {
            continue;
        };
        if old_version == new_version {
            continue;
        }
        let escaped = regex::escape(&pkg_name);
        // Captures: 1 = `"pkg" \s*:\s*"` (including opening quote of value), 2 = version content, 3 = closing quote
        let pattern = format!(r#"("{escaped}"\s*:\s*")([^"]*)(")"#);
        let re = Regex::new(&pattern).unwrap();

        let new_content = re
            .replace_all(&content, |caps: &regex::Captures| {
                let prefix = &caps[1];
                let version = &caps[2];
                let suffix = &caps[3];
                if version.contains(old_version) {
                    let new_ver = version.replace(old_version, new_version);
                    format!("{prefix}{new_ver}{suffix}")
                } else {
                    caps[0].to_string()
                }
            })
            .into_owned();

        if new_content != content {
            content = new_content;
            changed_any = true;
        }
    }

    if !changed_any || content == source {
        return None;
    }

    Some(content)
}

fn json_contains_dep(value: &Value, pkg_name: &str, old_version: &str) -> bool {
    match value {
        Value::Object(map) => {
            for (k, v) in map {
                if k == pkg_name
                    && let Value::String(s) = v
                    && s.contains(old_version)
                {
                    return true;
                }
                if json_contains_dep(v, pkg_name, old_version) {
                    return true;
                }
            }
            false
        }
        Value::Array(arr) => arr
            .iter()
            .any(|v| json_contains_dep(v, pkg_name, old_version)),
        _ => false,
    }
}

/// Second-pass bump for `package-lock.json`: update `packages["node_modules/<name>"].version`
/// and legacy `dependencies["<name>"].version` entries whose version exactly matches
/// `old_version` for a bumped package.
pub fn bump_package_lock_dependencies(
    file: &Path,
    bumped: &HashMap<String, (String, String)>,
) -> AppResult<bool> {
    write_dependency_update(file, bumped, "package-lock.json")
}

fn transform_package_lock_dependencies(
    source: &str,
    bumped: &HashMap<String, (String, String)>,
) -> Result<Option<String>, serde_json::Error> {
    let mut value: Value = match serde_json::from_str(source) {
        Ok(v) => v,
        Err(_) => return Ok(None),
    };
    let Value::Object(root) = &mut value else {
        return Ok(None);
    };

    let mut changed = false;

    // Update `packages` map (lockfileVersion 2/3)
    if let Some(Value::Object(packages)) = root.get_mut("packages") {
        for (key, pkg_val) in packages.iter_mut() {
            if key.is_empty() {
                continue;
            }
            let Some(obj) = pkg_val.as_object_mut() else {
                continue;
            };
            let Some(Value::String(ver)) = obj.get("version") else {
                continue;
            };
            for (pkg_name, (old_version, new_version)) in bumped {
                if ver != old_version {
                    continue;
                }
                let key_matches = key == &format!("node_modules/{pkg_name}")
                    || key.ends_with(&format!("/{pkg_name}"));
                let name_matches = obj
                    .get("name")
                    .and_then(|v| v.as_str())
                    .is_some_and(|n| n == pkg_name);
                if key_matches || name_matches {
                    obj.insert("version".to_string(), Value::String(new_version.clone()));
                    changed = true;
                    break;
                }
            }
        }
    }

    // Update legacy `dependencies` map (lockfileVersion 1) recursively
    if let Some(Value::Object(deps)) = root.get_mut("dependencies") {
        update_lock_dependencies_map(deps, bumped, &mut changed);
    }

    if !changed {
        return Ok(None);
    }

    let serialized = serde_json::to_string_pretty(&value)?;
    let mut written = serialized;
    if !written.ends_with('\n') {
        written.push('\n');
    }
    Ok(Some(written))
}

fn update_lock_dependencies_map(
    map: &mut serde_json::Map<String, Value>,
    bumped: &HashMap<String, (String, String)>,
    changed: &mut bool,
) {
    for (dep_name, dep_val) in map.iter_mut() {
        if let Some((old_version, new_version)) = bumped.get(dep_name)
            && let Some(obj) = dep_val.as_object_mut()
            && let Some(Value::String(ver)) = obj.get("version")
            && ver == old_version
        {
            obj.insert("version".to_string(), Value::String(new_version.clone()));
            *changed = true;
        }
        if let Some(obj) = dep_val.as_object_mut()
            && let Some(Value::Object(nested)) = obj.get_mut("dependencies")
        {
            update_lock_dependencies_map(nested, bumped, changed);
        }
    }
}

/// Second-pass bump for `Cargo.toml`: update `dependencies`/`dev-dependencies`/
/// `build-dependencies` (including `workspace.dependencies` and `target.<cfg>.dependencies`)
/// entries whose `version` matches `old_version` for a bumped crate.
pub fn bump_cargo_toml_dependencies(
    file: &Path,
    bumped: &HashMap<String, (String, String)>,
) -> AppResult<bool> {
    write_dependency_update(file, bumped, "Cargo.toml")
}

fn transform_cargo_toml_dependencies(
    source: &str,
    bumped: &HashMap<String, (String, String)>,
) -> Option<String> {
    let mut doc: toml_edit::DocumentMut = match source.parse() {
        Ok(d) => d,
        Err(_) => return None,
    };

    let mut changed = false;
    visit_cargo_toml_table(doc.as_table_mut(), bumped, &mut changed);

    if !changed {
        return None;
    }

    Some(doc.to_string())
}

fn visit_cargo_toml_table(
    table: &mut toml_edit::Table,
    bumped: &HashMap<String, (String, String)>,
    changed: &mut bool,
) {
    for (key, item) in table.iter_mut() {
        let key_str = key.to_string();
        if key_str == "dependencies"
            || key_str == "dev-dependencies"
            || key_str == "build-dependencies"
        {
            if let Some(dep_table) = item.as_table_mut() {
                for (dep_name, dep_item) in dep_table.iter_mut() {
                    let dep_name_str = dep_name.to_string();
                    if let Some((old_version, new_version)) = bumped.get(&dep_name_str) {
                        // `dep = "0.0.1"`
                        if let Some(ver) = dep_item.as_str()
                            && ver.contains(old_version)
                        {
                            let new_ver = ver.replace(old_version, new_version);
                            *dep_item = toml_edit::Item::Value(toml_edit::Value::String(
                                toml_edit::Formatted::new(new_ver),
                            ));
                            *changed = true;
                        } else if let Some(inline) = dep_item.as_inline_table_mut()
                            && let Some(ver_item) = inline.get_mut("version")
                            && let Some(ver) = ver_item.as_str()
                            && ver.contains(old_version)
                        {
                            let new_ver = ver.replace(old_version, new_version);
                            *ver_item =
                                toml_edit::Value::String(toml_edit::Formatted::new(new_ver));
                            *changed = true;
                        } else if let Some(tbl) = dep_item.as_table_mut()
                            && let Some(ver_item) = tbl.get_mut("version")
                            && let Some(ver) = ver_item.as_str()
                            && ver.contains(old_version)
                        {
                            let new_ver = ver.replace(old_version, new_version);
                            *ver_item = toml_edit::Item::Value(toml_edit::Value::String(
                                toml_edit::Formatted::new(new_ver),
                            ));
                            *changed = true;
                        }
                    }
                }
            }
        } else if let Some(sub_table) = item.as_table_mut() {
            visit_cargo_toml_table(sub_table, bumped, changed);
        }
    }
}

/// Second-pass bump for `Cargo.lock`: update `dependencies` entries like
/// `"test 0.0.1 (registry+...)"` inside any `[[package]]` block that references a bumped crate.
pub fn bump_cargo_lock_dependencies(
    file: &Path,
    bumped: &HashMap<String, (String, String)>,
) -> AppResult<bool> {
    write_dependency_update(file, bumped, "Cargo.lock")
}

fn transform_cargo_lock_dependencies(
    source: &str,
    bumped: &HashMap<String, (String, String)>,
) -> Option<String> {
    // Split into segments per `[[package]]` as in `bump_package_in_lock`.
    let mut segments: Vec<Vec<String>> = vec![Vec::new()];
    for line in source.lines() {
        if line.trim() == "[[package]]" {
            segments.push(vec![line.to_string()]);
        } else {
            segments.last_mut().unwrap().push(line.to_string());
        }
    }

    let mut changed = false;
    for segment in &mut segments {
        // Extract package name for this segment, if any, to handle `version` bump for the package itself.
        let pkg_name_in_segment = segment.iter().find_map(|l| {
            let t = l.trim();
            if t.starts_with("name = \"") && t.ends_with('"') {
                Some(t["name = \"".len()..t.len() - 1].to_string())
            } else {
                None
            }
        });

        let mut in_deps = false;
        for line in segment.iter_mut() {
            // Bump the package's own `version = "..."` if its name is in `bumped`.
            // Update unconditionally when the current version differs from `new_version` so
            // stale locks (where the on-disk version is behind the tag) are repaired.
            if let Some(ref name) = pkg_name_in_segment
                && let Some((_, new_version)) = bumped.get(name)
            {
                let trimmed = line.trim();
                if trimmed.starts_with("version = \"")
                    && trimmed.ends_with('"')
                    && let Some(start) = trimmed.find('"')
                    && let Some(end) = trimmed.rfind('"')
                    && start != end
                {
                    let current = &trimmed[start + 1..end];
                    if current != new_version {
                        let indent: String =
                            line.chars().take_while(|c| c.is_whitespace()).collect();
                        *line = format!("{indent}version = \"{new_version}\"");
                        changed = true;
                    }
                }
            }

            if line.trim() == "dependencies = [" {
                in_deps = true;
                continue;
            }
            if in_deps {
                if line.trim() == "]" {
                    in_deps = false;
                    continue;
                }
                // Update dependency strings like `"pkg 0.1.0"` or `"pkg 0.1.0 (registry+...)"`
                // by replacing the version token for any bumped package, regardless of
                // whether the on-disk version matches `old_version` (handles stale locks).
                if let (Some(f), Some(l)) = (line.find('"'), line.rfind('"'))
                    && f != l
                {
                    let inside = line[f + 1..l].to_string();
                    let mut tokens = inside.split_whitespace();
                    if let Some(pkg_token) = tokens.next()
                        && let Some((_, new_version)) = bumped.get(pkg_token)
                        && let Some(ver_token) = tokens.next()
                        && ver_token != new_version
                    {
                        let rest: Vec<&str> = tokens.collect();
                        let new_inside = if rest.is_empty() {
                            format!("{pkg_token} {new_version}")
                        } else {
                            format!("{pkg_token} {new_version} {}", rest.join(" "))
                        };
                        if new_inside != inside {
                            let new_line =
                                format!("{}{}{}", &line[..f + 1], new_inside, &line[l..]);
                            *line = new_line;
                            changed = true;
                        }
                    }
                }
            }
        }
    }

    if !changed {
        return None;
    }

    let mut written = segments
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("\n");
    if source.ends_with('\n') {
        written.push('\n');
    }

    Some(written)
}

fn read_toml_name(path: &Path, section: &str) -> AppResult<String> {
    let source = fs::read_to_string(path)
        .map_err(|e| format!("failed to read '{}': {e}", path.display()))?;
    let re = Regex::new(&format!(
        "(?m)^\\[{}\\][^\\[]*?name = \"([^\"]*)\"",
        regex::escape(section)
    ))
    .unwrap();
    re.captures(&source)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_owned())
        .ok_or_else(|| format!("no [{section}].name in '{}'", path.display()))
}

/// Updates `version` only inside the `[[package]]` block whose `name` matches `package_name`.
/// Used for lock files (`Cargo.lock`, `uv.lock`) that contain one block per package.
fn bump_package_in_lock(
    file: &Path,
    _old_version: &str,
    new_version: &str,
    package_name: &str,
    write: bool,
) -> AppResult<bool> {
    let source = fs::read_to_string(file)
        .map_err(|e| format!("failed to read '{}': {e}", file.display()))?;

    // Split into segments: first segment is the preamble, remaining segments each start with
    // a `[[package]]` line.
    let mut segments: Vec<Vec<String>> = vec![Vec::new()];
    for line in source.lines() {
        if line.trim() == "[[package]]" {
            segments.push(vec![line.to_string()]);
        } else {
            segments.last_mut().unwrap().push(line.to_string());
        }
    }

    let mut changed = false;
    for segment in &mut segments {
        let is_target = segment.first().is_some_and(|l| l.trim() == "[[package]]")
            && segment
                .iter()
                .any(|l| l.trim() == format!("name = \"{package_name}\""));

        if is_target {
            for line in segment.iter_mut() {
                let trimmed = line.trim();
                if trimmed.starts_with("version = \"")
                    && trimmed.ends_with('"')
                    && let Some(start) = trimmed.find('"')
                    && let Some(end) = trimmed.rfind('"')
                    && start != end
                {
                    let current = &trimmed[start + 1..end];
                    // Update even when `current != _old_version` so stale locks are repaired.
                    if current != new_version {
                        let indent: String =
                            line.chars().take_while(|c| c.is_whitespace()).collect();
                        *line = format!("{indent}version = \"{new_version}\"");
                        changed = true;
                    }
                }
            }
        }
    }

    if !changed {
        return Ok(false);
    }

    let mut written = segments
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("\n");
    if source.ends_with('\n') {
        written.push('\n');
    }

    if write {
        fs::write(file, written)
            .map_err(|e| format!("failed to write '{}': {e}", file.display()))?;
    }
    Ok(true)
}

fn regex_replace_file(file: &Path, re: &Regex, replacement: &str, write: bool) -> AppResult<bool> {
    let source = fs::read_to_string(file)
        .map_err(|e| format!("failed to read '{}': {e}", file.display()))?;
    match re.replace(&source, replacement) {
        Cow::Borrowed(_) => Ok(false),
        Cow::Owned(replaced) if replaced == source => Ok(false),
        Cow::Owned(replaced) => {
            if write {
                fs::write(file, replaced)
                    .map_err(|e| format!("failed to write '{}': {e}", file.display()))?;
            }
            Ok(true)
        }
    }
}

fn replace_literal(
    file: &Path,
    old_version: &str,
    new_version: &str,
    write: bool,
) -> AppResult<bool> {
    let source = fs::read_to_string(file)
        .map_err(|e| format!("failed to read '{}': {e}", file.display()))?;
    let replaced = source.replace(old_version, new_version);
    if source == replaced {
        return Ok(false);
    }

    if write {
        fs::write(file, replaced)
            .map_err(|e| format!("failed to write '{}': {e}", file.display()))?;
    }
    Ok(true)
}

fn replace_line_value(file: &Path, key: &str, new_version: &str, write: bool) -> AppResult<bool> {
    let re = Regex::new(&format!(r#"(?m)^(\s*){} = "[^"]*".*$"#, regex::escape(key))).unwrap();
    regex_replace_file(
        file,
        &re,
        &format!(r#"${{1}}{key} = "{new_version}","#),
        write,
    )
}

fn bump_package_json(file: &Path, new_version: &str, write: bool) -> AppResult<bool> {
    let re = Regex::new(r#"(?m)^(\s*)"version":\s*"[^"]*"(,?)$"#).unwrap();
    regex_replace_file(
        file,
        &re,
        &format!(r#"${{1}}"version": "{new_version}"$2"#),
        write,
    )
}

fn bump_gradle_build(
    file: &Path,
    old_version: &str,
    new_version: &str,
    write: bool,
) -> AppResult<bool> {
    let re = Regex::new(&format!(
        r#"(?m)^([ \t]*version[ \t]*=[ \t]*)(["']){}(["'])([^\r\n]*)$"#,
        regex::escape(old_version)
    ))
    .unwrap();
    let source = fs::read_to_string(file)
        .map_err(|e| format!("failed to read '{}': {e}", file.display()))?;
    let replacement = format!(r#"${{1}}${{2}}{new_version}${{3}}${{4}}"#);
    match re.replace_all(&source, replacement.as_str()) {
        Cow::Borrowed(_) => Ok(false),
        Cow::Owned(replaced) if replaced == source => Ok(false),
        Cow::Owned(replaced) => {
            if write {
                fs::write(file, replaced)
                    .map_err(|e| format!("failed to write '{}': {e}", file.display()))?;
            }
            Ok(true)
        }
    }
}

fn bump_gradle_properties(
    file: &Path,
    old_version: &str,
    new_version: &str,
    write: bool,
) -> AppResult<bool> {
    let re = Regex::new(&format!(
        r#"(?m)^([ \t]*version[ \t]*=[ \t]*){}([ \t]*)$"#,
        regex::escape(old_version)
    ))
    .unwrap();
    regex_replace_file(file, &re, &format!(r#"${{1}}{new_version}${{2}}"#), write)
}

fn bump_package_lock_json(file: &Path, new_version: &str, write: bool) -> AppResult<bool> {
    let source = fs::read_to_string(file)
        .map_err(|e| format!("failed to read '{}': {e}", file.display()))?;

    // package-lock.json has two version fields to update:
    // 1. The top-level "version" (appears before "packages")
    // 2. The "version" inside packages[""] (the root package entry)
    //
    // A state machine ensures only those two occurrences are touched,
    // leaving dependency versions under other package entries untouched.
    #[derive(PartialEq)]
    enum State {
        Root,
        InPackages,
        InRootPkg,
        Done,
    }

    let mut state = State::Root;
    let mut root_brace_depth: u32 = 0;
    let mut changed = false;
    let mut output = Vec::new();

    for line in source.lines() {
        let trimmed = line.trim();

        let replace_this_version = match state {
            State::Root if trimmed.starts_with("\"version\"") => true,
            State::Root => {
                if trimmed.starts_with("\"packages\"") {
                    state = State::InPackages;
                }
                false
            }
            State::InPackages => {
                if trimmed == "\"\":" || trimmed == "\"\": {" {
                    state = State::InRootPkg;
                    root_brace_depth = if trimmed.ends_with('{') { 1 } else { 0 };
                }
                false
            }
            State::InRootPkg => {
                root_brace_depth += trimmed.chars().filter(|&c| c == '{').count() as u32;
                root_brace_depth = root_brace_depth
                    .saturating_sub(trimmed.chars().filter(|&c| c == '}').count() as u32);
                if root_brace_depth == 0 {
                    state = State::Done;
                    false
                } else {
                    trimmed.starts_with("\"version\"")
                }
            }
            State::Done => false,
        };

        if replace_this_version {
            let indent: String = line.chars().take_while(|c| c.is_whitespace()).collect();
            let suffix = if trimmed.ends_with(',') { "," } else { "" };
            output.push(format!("{indent}\"version\": \"{new_version}\"{suffix}"));
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
    if written == source {
        return Ok(false);
    }
    if write {
        fs::write(file, written)
            .map_err(|e| format!("failed to write '{}': {e}", file.display()))?;
    }
    Ok(true)
}

fn bump_cmake_lists(file: &Path, new_version: &str, write: bool) -> AppResult<bool> {
    let re = Regex::new(
        r#"(?is)(\bproject\s*\([^)]*?\bVERSION[ \t\r\n]+)("?)([0-9]+(?:\.[0-9]+){0,3})("?)([^0-9.])"#,
    )
    .unwrap();
    regex_replace_file(
        file,
        &re,
        &format!("${{1}}${{2}}{new_version}${{4}}${{5}}"),
        write,
    )
}

fn bump_nix_version(
    file: &Path,
    old_version: &str,
    new_version: &str,
    write: bool,
) -> AppResult<bool> {
    // Matches `version = "0.1.0";` with flexible whitespace, preserving
    // the prefix (`version = "`) and suffix (`";`) formatting.
    let re = Regex::new(&format!(
        r#"(\bversion\s*=\s*"){}("\s*;?)"#,
        regex::escape(old_version)
    ))
    .unwrap();
    let source = fs::read_to_string(file)
        .map_err(|e| format!("failed to read '{}': {e}", file.display()))?;
    let replacement = format!(r"${{1}}{new_version}${{2}}");
    match re.replace_all(&source, replacement.as_str()) {
        Cow::Borrowed(_) => Ok(false),
        Cow::Owned(replaced) if replaced == source => Ok(false),
        Cow::Owned(replaced) => {
            if write {
                fs::write(file, replaced)
                    .map_err(|e| format!("failed to write '{}': {e}", file.display()))?;
            }
            Ok(true)
        }
    }
}

fn bump_toml_path(file: &Path, path: &[&str], new_version: &str, write: bool) -> AppResult<bool> {
    let source = fs::read_to_string(file)
        .map_err(|e| format!("failed to read '{}': {e}", file.display()))?;
    let mut doc: toml_edit::DocumentMut = match source.parse() {
        Ok(doc) => doc,
        Err(_) => {
            if path.len() == 2 {
                return replace_toml_section_key_line(
                    file,
                    &source,
                    path[0],
                    path[1],
                    new_version,
                    write,
                );
            }
            return Ok(false);
        }
    };

    let mut item = doc.as_item_mut();
    for key in path.iter().take(path.len() - 1) {
        let Some(next) = item.get_mut(*key) else {
            return Ok(false);
        };
        item = next;
    }

    let leaf = path[path.len() - 1];
    let Some(value) = item.get_mut(leaf) else {
        return Ok(false);
    };

    if value.as_str() == Some(new_version) {
        return Ok(false);
    }

    *value = toml_edit::value(new_version);
    if write {
        fs::write(file, doc.to_string())
            .map_err(|e| format!("failed to write '{}': {e}", file.display()))?;
    }
    Ok(true)
}

fn replace_toml_section_key_line(
    file: &Path,
    source: &str,
    section: &str,
    key: &str,
    new_version: &str,
    write: bool,
) -> AppResult<bool> {
    let re = Regex::new(&format!(
        r#"(?m)(^\[{}\][^\[]*?){} = "[^"]*""#,
        regex::escape(section),
        regex::escape(key)
    ))
    .unwrap();
    let replacement = format!(r#"${{1}}{key} = "{new_version}""#);
    match re.replace(source, replacement.as_str()) {
        Cow::Borrowed(_) => Ok(false),
        Cow::Owned(replaced) if replaced == source => Ok(false),
        Cow::Owned(replaced) => {
            if write {
                fs::write(file, replaced)
                    .map_err(|e| format!("failed to write '{}': {e}", file.display()))?;
            }
            Ok(true)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_path(name: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("bumper-bump-{name}-{nanos}"))
    }

    fn write_package_json(dir: &std::path::Path, content: &str) -> std::path::PathBuf {
        fs::create_dir_all(dir).expect("create dir");
        let path = dir.join("package.json");
        fs::write(&path, content).expect("write package.json");
        path
    }

    #[test]
    fn bump_package_json_deps_updates_all_dependency_sections() {
        let dir = temp_path("deps-all-sections");
        let path = write_package_json(
            &dir,
            r#"{
  "name": "app",
  "version": "0.13.0",
  "dependencies": {
    "@trevrpc/trevrpc-js": "^0.13.0"
  },
  "devDependencies": {
    "@trevrpc/trevrpc-js": "0.13.0"
  },
  "peerDependencies": {
    "@trevrpc/trevrpc-js": "~0.13.0"
  },
  "optionalDependencies": {
    "@trevrpc/trevrpc-js": ">=0.13.0"
  }
}"#,
        );
        let mut bumped = HashMap::new();
        bumped.insert(
            "@trevrpc/trevrpc-js".to_string(),
            ("0.13.0".to_string(), "0.14.0".to_string()),
        );
        let changed = bump_package_json_dependencies(&path, &bumped).expect("bump deps");
        assert!(changed);
        let out = fs::read_to_string(&path).expect("read");
        assert!(out.contains("\"@trevrpc/trevrpc-js\": \"^0.14.0\""));
        assert!(out.contains("\"@trevrpc/trevrpc-js\": \"0.14.0\""));
        assert!(out.contains("\"@trevrpc/trevrpc-js\": \"~0.14.0\""));
        assert!(out.contains("\"@trevrpc/trevrpc-js\": \">=0.14.0\""));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn bump_package_json_deps_preserves_formatting_and_indentation() {
        let dir = temp_path("deps-format");
        let original = "{\n  \"name\": \"app\",\n  \"dependencies\": {\n    \"@trevrpc/trevrpc-js\": \"^0.13.0\"\n  }\n}\n";
        let path = write_package_json(&dir, original);
        let mut bumped = HashMap::new();
        bumped.insert(
            "@trevrpc/trevrpc-js".to_string(),
            ("0.13.0".to_string(), "0.14.0".to_string()),
        );
        bump_package_json_dependencies(&path, &bumped).expect("bump");
        let out = fs::read_to_string(&path).expect("read");
        // Should preserve indentation (2 spaces before key)
        assert!(out.contains("    \"@trevrpc/trevrpc-js\": \"^0.14.0\""));
        assert!(!out.contains("0.13.0"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn bump_package_json_deps_no_change_when_version_not_matching() {
        let dir = temp_path("deps-no-change");
        let path = write_package_json(
            &dir,
            r#"{
  "name": "app",
  "dependencies": {
    "@trevrpc/trevrpc-js": "^0.12.0",
    "other": "0.13.0"
  }
}"#,
        );
        let mut bumped = HashMap::new();
        bumped.insert(
            "@trevrpc/trevrpc-js".to_string(),
            ("0.13.0".to_string(), "0.14.0".to_string()),
        );
        let changed = bump_package_json_dependencies(&path, &bumped).expect("bump");
        assert!(!changed);
        let out = fs::read_to_string(&path).expect("read");
        assert!(out.contains("^0.12.0"));
        assert!(!out.contains("0.14.0"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn bump_package_json_deps_handles_multiple_bumped_packages() {
        let dir = temp_path("deps-multi");
        let path = write_package_json(
            &dir,
            r#"{
  "dependencies": {
    "pkg-a": "0.13.0",
    "pkg-b": "^0.13.0"
  }
}"#,
        );
        let mut bumped = HashMap::new();
        bumped.insert(
            "pkg-a".to_string(),
            ("0.13.0".to_string(), "0.14.0".to_string()),
        );
        bumped.insert(
            "pkg-b".to_string(),
            ("0.13.0".to_string(), "0.14.0".to_string()),
        );
        let changed = bump_package_json_dependencies(&path, &bumped).expect("bump");
        assert!(changed);
        let out = fs::read_to_string(&path).expect("read");
        assert!(out.contains("\"pkg-a\": \"0.14.0\""));
        assert!(out.contains("\"pkg-b\": \"^0.14.0\""));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn bump_package_json_deps_overrides_and_resolutions() {
        let dir = temp_path("deps-overrides");
        let path = write_package_json(
            &dir,
            r#"{
  "overrides": {
    "@trevrpc/trevrpc-js": "0.13.0"
  },
  "resolutions": {
    "@trevrpc/trevrpc-js": "^0.13.0"
  }
}"#,
        );
        let mut bumped = HashMap::new();
        bumped.insert(
            "@trevrpc/trevrpc-js".to_string(),
            ("0.13.0".to_string(), "0.14.0".to_string()),
        );
        let changed = bump_package_json_dependencies(&path, &bumped).expect("bump");
        assert!(changed);
        let out = fs::read_to_string(&path).expect("read");
        assert!(out.contains("\"@trevrpc/trevrpc-js\": \"0.14.0\""));
        assert!(out.contains("\"@trevrpc/trevrpc-js\": \"^0.14.0\""));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn package_name_for_file_reads_package_json() {
        let dir = temp_path("pkg-name");
        let path = write_package_json(&dir, r#"{"name":"@trevrpc/trevrpc-js","version":"0.13.0"}"#);
        assert_eq!(
            package_name_for_file(&path),
            Some("@trevrpc/trevrpc-js".to_string())
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn bump_package_json_deps_ignores_non_package_json() {
        let dir = temp_path("deps-ignore");
        fs::create_dir_all(&dir).expect("create");
        let path = dir.join("other.json");
        fs::write(&path, r#"{"dependencies":{"pkg-a":"0.13.0"}}"#).expect("write");
        let mut bumped = HashMap::new();
        bumped.insert(
            "pkg-a".to_string(),
            ("0.13.0".to_string(), "0.14.0".to_string()),
        );
        let changed = bump_package_json_dependencies(&path, &bumped).expect("bump");
        assert!(!changed);
        let _ = fs::remove_dir_all(dir);
    }

    fn write_package_lock(dir: &std::path::Path, content: &str) -> std::path::PathBuf {
        fs::create_dir_all(dir).expect("create dir");
        let path = dir.join("package-lock.json");
        fs::write(&path, content).expect("write lock");
        path
    }

    #[test]
    fn bump_package_lock_deps_updates_packages() {
        let dir = temp_path("lock-packages");
        let path = write_package_lock(
            &dir,
            r#"{
  "name": "test",
  "version": "0.13.0",
  "lockfileVersion": 3,
  "packages": {
    "": {
      "name": "test",
      "version": "0.13.0"
    },
    "node_modules/pkg-a": {
      "version": "0.13.0"
    },
    "node_modules/pkg-b": {
      "version": "0.13.0"
    },
    "node_modules/other": {
      "version": "0.13.0"
    }
  }
}"#,
        );
        let mut bumped = HashMap::new();
        bumped.insert(
            "pkg-a".to_string(),
            ("0.13.0".to_string(), "0.14.0".to_string()),
        );
        let changed = bump_package_lock_dependencies(&path, &bumped).expect("bump");
        assert!(changed);
        let out = fs::read_to_string(&path).expect("read");
        let v: Value = serde_json::from_str(&out).expect("parse");
        assert_eq!(
            v["packages"]["node_modules/pkg-a"]["version"]
                .as_str()
                .unwrap(),
            "0.14.0"
        );
        assert_eq!(
            v["packages"]["node_modules/pkg-b"]["version"]
                .as_str()
                .unwrap(),
            "0.13.0"
        );
        assert_eq!(
            v["packages"]["node_modules/other"]["version"]
                .as_str()
                .unwrap(),
            "0.13.0"
        );
        assert_eq!(v["packages"][""]["version"].as_str().unwrap(), "0.13.0");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn bump_package_lock_deps_handles_scoped_packages() {
        let dir = temp_path("lock-scoped");
        let path = write_package_lock(
            &dir,
            r#"{
  "packages": {
    "": {"version": "0.13.0"},
    "node_modules/@trevrpc/trevrpc-js": {
      "version": "0.13.0"
    },
    "node_modules/@trevrpc/trevrpc-js/node_modules/other": {
      "version": "1.0.0"
    }
  }
}"#,
        );
        let mut bumped = HashMap::new();
        bumped.insert(
            "@trevrpc/trevrpc-js".to_string(),
            ("0.13.0".to_string(), "0.14.0".to_string()),
        );
        let changed = bump_package_lock_dependencies(&path, &bumped).expect("bump");
        assert!(changed);
        let out = fs::read_to_string(&path).expect("read");
        let v: Value = serde_json::from_str(&out).expect("parse");
        assert_eq!(
            v["packages"]["node_modules/@trevrpc/trevrpc-js"]["version"]
                .as_str()
                .unwrap(),
            "0.14.0"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn bump_package_lock_deps_updates_legacy_dependencies() {
        let dir = temp_path("lock-legacy");
        let path = write_package_lock(
            &dir,
            r#"{
  "dependencies": {
    "pkg-a": {
      "version": "0.13.0",
      "dependencies": {
        "pkg-a": {
          "version": "0.13.0"
        }
      }
    },
    "pkg-b": {
      "version": "0.13.0"
    }
  }
}"#,
        );
        let mut bumped = HashMap::new();
        bumped.insert(
            "pkg-a".to_string(),
            ("0.13.0".to_string(), "0.14.0".to_string()),
        );
        let changed = bump_package_lock_dependencies(&path, &bumped).expect("bump");
        assert!(changed);
        let out = fs::read_to_string(&path).expect("read");
        let v: Value = serde_json::from_str(&out).expect("parse");
        assert_eq!(
            v["dependencies"]["pkg-a"]["version"].as_str().unwrap(),
            "0.14.0"
        );
        assert_eq!(
            v["dependencies"]["pkg-a"]["dependencies"]["pkg-a"]["version"]
                .as_str()
                .unwrap(),
            "0.14.0"
        );
        assert_eq!(
            v["dependencies"]["pkg-b"]["version"].as_str().unwrap(),
            "0.13.0"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn bump_package_lock_deps_no_change_when_version_not_matching() {
        let dir = temp_path("lock-no-change");
        let path = write_package_lock(
            &dir,
            r#"{
  "packages": {
    "node_modules/pkg-a": {"version": "0.12.0"}
  }
}"#,
        );
        let mut bumped = HashMap::new();
        bumped.insert(
            "pkg-a".to_string(),
            ("0.13.0".to_string(), "0.14.0".to_string()),
        );
        let changed = bump_package_lock_dependencies(&path, &bumped).expect("bump");
        assert!(!changed);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn bump_package_lock_deps_ignores_non_lock() {
        let dir = temp_path("lock-ignore");
        fs::create_dir_all(&dir).expect("create");
        let path = dir.join("other.json");
        fs::write(
            &path,
            r#"{"packages":{"node_modules/pkg-a":{"version":"0.13.0"}}}"#,
        )
        .expect("write");
        let mut bumped = HashMap::new();
        bumped.insert(
            "pkg-a".to_string(),
            ("0.13.0".to_string(), "0.14.0".to_string()),
        );
        let changed = bump_package_lock_dependencies(&path, &bumped).expect("bump");
        assert!(!changed);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn dependency_update_preview_reports_updates_without_writing() {
        let dir = temp_path("dependency-preview-needed");
        fs::create_dir_all(&dir).expect("create dir");
        let bumped = HashMap::from([(
            "pkg-a".to_string(),
            ("0.13.0".to_string(), "0.14.0".to_string()),
        )]);
        let cases = [
            (
                "package.json",
                "{\n  \"dependencies\": {\n    \"pkg-a\": \"^0.13.0\"\n  }\n}\n",
            ),
            (
                "package-lock.json",
                "{\n  \"packages\": {\n    \"node_modules/pkg-a\": {\n      \"version\": \"0.13.0\"\n    }\n  }\n}\n",
            ),
            (
                "Cargo.toml",
                "[dependencies]\npkg-a = { version = \"0.13.0\", path = \"../pkg-a\" }\n",
            ),
            (
                "Cargo.lock",
                "version = 4\n\n[[package]]\nname = \"consumer\"\nversion = \"1.0.0\"\ndependencies = [\n \"pkg-a 0.13.0\",\n]\n\n[[package]]\nname = \"pkg-a\"\nversion = \"0.13.0\"\n",
            ),
        ];

        for (file_name, original) in cases {
            let path = dir.join(file_name);
            fs::write(&path, original).expect("write source file");

            assert!(
                dependency_update_needed(&path, &bumped).expect("preview dependency update"),
                "{file_name} should need an update"
            );
            assert_eq!(
                fs::read_to_string(&path).expect("read source file"),
                original,
                "preview must not modify {file_name}"
            );
        }

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn dependency_update_preview_reports_no_update_without_writing() {
        let dir = temp_path("dependency-preview-not-needed");
        fs::create_dir_all(&dir).expect("create dir");
        let bumped = HashMap::from([(
            "pkg-a".to_string(),
            ("0.13.0".to_string(), "0.14.0".to_string()),
        )]);
        let cases = [
            (
                "package.json",
                "{\n  \"dependencies\": {\n    \"pkg-a\": \"^0.14.0\"\n  }\n}\n",
            ),
            (
                "package-lock.json",
                "{\n  \"packages\": {\n    \"node_modules/pkg-a\": {\n      \"version\": \"0.14.0\"\n    }\n  }\n}\n",
            ),
            ("Cargo.toml", "[dependencies]\npkg-a = \"0.14.0\"\n"),
            (
                "Cargo.lock",
                "version = 4\n\n[[package]]\nname = \"consumer\"\nversion = \"1.0.0\"\ndependencies = [\n \"pkg-a 0.14.0\",\n]\n\n[[package]]\nname = \"pkg-a\"\nversion = \"0.14.0\"\n",
            ),
        ];

        for (file_name, original) in cases {
            let path = dir.join(file_name);
            fs::write(&path, original).expect("write source file");

            assert!(
                !dependency_update_needed(&path, &bumped).expect("preview dependency update"),
                "{file_name} should not need an update"
            );
            assert_eq!(
                fs::read_to_string(&path).expect("read source file"),
                original,
                "preview must not modify {file_name}"
            );
        }

        let unsupported = dir.join("dependencies.txt");
        assert!(
            !dependency_update_needed(&unsupported, &bumped).expect("preview unsupported file")
        );
        assert!(!unsupported.exists());

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn typed_change_preview_reports_change_without_writing() {
        let dir = temp_path("typed-preview");
        fs::create_dir_all(&dir).expect("create dir");
        let path = dir.join("README.md");
        let original = "version 1.2.3\n";
        fs::write(&path, original).expect("write README");

        assert_eq!(
            preview_typed_change(&path, "1.2.3", "1.2.4").expect("preview typed change"),
            TypedChange::Changed
        );
        assert_eq!(fs::read_to_string(&path).expect("read README"), original);

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn typed_change_preview_ignores_an_already_current_version() {
        let dir = temp_path("typed-preview-current");
        fs::create_dir_all(&dir).expect("create dir");
        let path = dir.join("package.json");
        let original = "{\n  \"name\": \"app\",\n  \"version\": \"1.2.4\"\n}\n";
        fs::write(&path, original).expect("write package.json");

        assert_eq!(
            preview_typed_change(&path, "1.2.3", "1.2.4").expect("preview typed change"),
            TypedChange::Unchanged
        );
        assert_eq!(
            fs::read_to_string(&path).expect("read package.json"),
            original
        );

        let _ = fs::remove_dir_all(dir);
    }
}

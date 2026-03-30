use std::fs;
use std::path::Path;

type AppResult<T> = Result<T, String>;

pub fn apply_typed_change(file: &Path, old_version: &str, new_version: &str) -> AppResult<bool> {
    let name = file
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| format!("invalid file path '{}'", file.display()))?;

    match name {
        "flake.nix" => replace_literal(file, old_version, new_version),
        "package.json" => bump_package_json(file, new_version),
        "package-lock.json" => bump_package_lock_json(file, new_version),
        "Cargo.toml" => bump_toml_path(file, &["package", "version"], new_version),
        "pyproject.toml" => bump_toml_path(file, &["project", "version"], new_version),
        "uv.lock" => {
            let name = read_toml_name(&file.with_file_name("pyproject.toml"), "project")?;
            bump_package_in_lock(file, old_version, new_version, &name)
        }
        "Cargo.lock" => {
            let name = read_toml_name(&file.with_file_name("Cargo.toml"), "package")?;
            bump_package_in_lock(file, old_version, new_version, &name)
        }
        "build.zig.zon" => replace_line_value(file, ".version", new_version),
        _ => Ok(false),
    }
}

/// Reads `[section].name` from a TOML manifest file using a simple line scan,
/// avoiding any TOML parse dependency (consistent with how `replace_toml_section_key_line` works).
fn read_toml_name(path: &Path, section: &str) -> AppResult<String> {
    let source = fs::read_to_string(path)
        .map_err(|e| format!("failed to read '{}': {e}", path.display()))?;
    let section_header = format!("[{section}]");
    let mut in_section = false;
    for line in source.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            in_section = trimmed == section_header;
            continue;
        }
        if in_section
            && let Some(rest) = trimmed.strip_prefix("name = \"")
            && let Some(name) = rest.strip_suffix('"')
        {
            return Ok(name.to_owned());
        }
    }
    Err(format!("no [{section}].name in '{}'", path.display()))
}

/// Updates `version` only inside the `[[package]]` block whose `name` matches `package_name`.
/// Used for lock files (`Cargo.lock`, `uv.lock`) that contain one block per package.
fn bump_package_in_lock(
    file: &Path,
    old_version: &str,
    new_version: &str,
    package_name: &str,
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
                if line.trim() == format!("version = \"{old_version}\"") {
                    let indent: String = line.chars().take_while(|c| c.is_whitespace()).collect();
                    *line = format!("{indent}version = \"{new_version}\"");
                    changed = true;
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

    fs::write(file, written).map_err(|e| format!("failed to write '{}': {e}", file.display()))?;
    Ok(true)
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

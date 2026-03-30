use std::borrow::Cow;
use std::fs;
use std::path::Path;

use regex::Regex;

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

fn regex_replace_file(file: &Path, re: &Regex, replacement: &str) -> AppResult<bool> {
    let source = fs::read_to_string(file)
        .map_err(|e| format!("failed to read '{}': {e}", file.display()))?;
    match re.replace(&source, replacement) {
        Cow::Borrowed(_) => Ok(false),
        Cow::Owned(replaced) => {
            fs::write(file, replaced)
                .map_err(|e| format!("failed to write '{}': {e}", file.display()))?;
            Ok(true)
        }
    }
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
    let re = Regex::new(&format!(r#"(?m)^(\s*){} = "[^"]*".*$"#, regex::escape(key))).unwrap();
    regex_replace_file(file, &re, &format!(r#"${{1}}{key} = "{new_version}","#))
}

fn bump_package_json(file: &Path, new_version: &str) -> AppResult<bool> {
    let re = Regex::new(r#"(?m)^(\s*)"version":\s*"[^"]*"(,?)$"#).unwrap();
    regex_replace_file(file, &re, &format!(r#"${{1}}"version": "{new_version}"$2"#))
}

fn bump_package_lock_json(file: &Path, new_version: &str) -> AppResult<bool> {
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
    fs::write(file, written).map_err(|e| format!("failed to write '{}': {e}", file.display()))?;
    Ok(true)
}

fn bump_toml_path(file: &Path, path: &[&str], new_version: &str) -> AppResult<bool> {
    let source = fs::read_to_string(file)
        .map_err(|e| format!("failed to read '{}': {e}", file.display()))?;
    let mut doc: toml_edit::DocumentMut = match source.parse() {
        Ok(doc) => doc,
        Err(_) => {
            if path.len() == 2 {
                return replace_toml_section_key_line(file, &source, path[0], path[1], new_version);
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
    fs::write(file, doc.to_string())
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
    let re = Regex::new(&format!(
        r#"(?m)(^\[{}\][^\[]*?){} = "[^"]*""#,
        regex::escape(section),
        regex::escape(key)
    ))
    .unwrap();
    let replacement = format!(r#"${{1}}{key} = "{new_version}""#);
    match re.replace(source, replacement.as_str()) {
        Cow::Borrowed(_) => Ok(false),
        Cow::Owned(replaced) => {
            fs::write(file, replaced)
                .map_err(|e| format!("failed to write '{}': {e}", file.display()))?;
            Ok(true)
        }
    }
}

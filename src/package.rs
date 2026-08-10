use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::Value as JsonValue;
use toml_edit::{DocumentMut, Table};

/// A package directory within a repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Package {
    /// The absolute, canonical path to the package directory.
    pub root: PathBuf,
    /// The package directory relative to the repository root.
    pub path: PathBuf,
}

/// Returns whether `directory` contains a supported, valid package marker.
///
/// Merely having a similarly named file is not enough: the marker must contain
/// the package identity fields required by its format. Formats that store a
/// version in the manifest must also contain that version.
pub fn is_versioned_package(directory: impl AsRef<Path>) -> bool {
    let directory = directory.as_ref();
    if !directory.is_dir() {
        return false;
    }

    [
        "package.json",
        "Cargo.toml",
        "pyproject.toml",
        "gleam.toml",
        "build.zig.zon",
        "CMakeLists.txt",
        "build.gradle",
        "build.gradle.kts",
        "go.mod",
    ]
    .iter()
    .any(|name| is_package_marker(directory.join(name)))
}

/// Returns whether `path` is a supported, valid package-defining manifest.
pub fn is_package_marker(path: impl AsRef<Path>) -> bool {
    let path = path.as_ref();
    match path.file_name().and_then(|name| name.to_str()) {
        Some("package.json") => is_package_json(path),
        Some("Cargo.toml") => is_sectioned_toml(path, "package"),
        Some("pyproject.toml") => is_sectioned_toml(path, "project"),
        Some("gleam.toml") => is_gleam_toml(path),
        Some("build.zig.zon") => is_zig_zon(path),
        Some("CMakeLists.txt") => is_cmake(path),
        Some("build.gradle" | "build.gradle.kts") => is_gradle(path),
        Some("go.mod") => is_go_mod(path),
        _ => false,
    }
}

/// Resolves `supplied_path` to its nearest package and its package ancestors.
///
/// Packages are returned from nearest to outermost. The canonical repository
/// root is always the final package, even when it has no package marker.
/// Relative supplied paths are interpreted relative to `repo_root`.
pub fn resolve_package_hierarchy(
    repo_root: impl AsRef<Path>,
    supplied_path: impl AsRef<Path>,
) -> io::Result<Vec<Package>> {
    let repo_root = fs::canonicalize(repo_root)?;
    if !repo_root.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "repository root is not a directory",
        ));
    }

    let supplied_path = supplied_path.as_ref();
    let supplied_path = if supplied_path.is_absolute() {
        supplied_path.to_path_buf()
    } else {
        repo_root.join(supplied_path)
    };
    let supplied_path = fs::canonicalize(supplied_path)?;

    if !supplied_path.starts_with(&repo_root) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "supplied path is outside the repository root",
        ));
    }

    let mut directory = if supplied_path.is_dir() {
        supplied_path
    } else {
        supplied_path
            .parent()
            .expect("a canonical file path has a parent")
            .to_path_buf()
    };
    let mut packages = Vec::new();

    loop {
        if directory == repo_root || is_versioned_package(&directory) {
            packages.push(Package {
                path: directory
                    .strip_prefix(&repo_root)
                    .expect("directory was checked to be inside repository")
                    .to_path_buf(),
                root: directory.clone(),
            });
        }

        if directory == repo_root {
            break;
        }

        directory = directory
            .parent()
            .expect("a descendant of the repository root has a parent")
            .to_path_buf();
    }

    Ok(packages)
}

fn read(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok()
}

fn nonempty(value: Option<&str>) -> bool {
    value.is_some_and(|value| !value.trim().is_empty())
}

fn is_package_json(path: &Path) -> bool {
    let Some(value) = read(path).and_then(|source| serde_json::from_str::<JsonValue>(&source).ok())
    else {
        return false;
    };
    nonempty(value.get("name").and_then(JsonValue::as_str))
        && value.get("version").is_some_and(JsonValue::is_string)
}

fn parse_toml(path: &Path) -> Option<DocumentMut> {
    read(path)?.parse().ok()
}

fn table_has_name_and_version(table: &Table) -> bool {
    nonempty(table.get("name").and_then(|item| item.as_str()))
        && table
            .get("version")
            .is_some_and(|item| item.as_str().is_some())
}

fn is_sectioned_toml(path: &Path, section: &str) -> bool {
    let Some(document) = parse_toml(path) else {
        return false;
    };
    document
        .get(section)
        .and_then(|item| item.as_table())
        .is_some_and(table_has_name_and_version)
}

fn is_gleam_toml(path: &Path) -> bool {
    let Some(document) = parse_toml(path) else {
        return false;
    };
    let table = document.as_table();
    nonempty(table.get("name").and_then(|item| item.as_str()))
        && nonempty(table.get("version").and_then(|item| item.as_str()))
}

fn is_zig_zon(path: &Path) -> bool {
    let Some(source) = read(path) else {
        return false;
    };
    let source = strip_c_style_comments(&source);
    let mut name = false;
    let mut version = false;

    for line in source.lines() {
        let line = line.trim();
        let Some((field, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim().trim_end_matches(',').trim();
        let valid_value = zig_value(value).is_some_and(|value| !value.trim().is_empty());
        match field.trim() {
            ".name" => name |= valid_value,
            ".version" => version |= valid_value,
            _ => {}
        }
    }

    name && version
}

fn zig_value(value: &str) -> Option<&str> {
    if let Some(value) = value.strip_prefix('"').and_then(|v| v.strip_suffix('"')) {
        return Some(value);
    }

    value.strip_prefix('.').filter(|value| {
        !value.is_empty()
            && value
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '_')
    })
}

fn strip_c_style_comments(source: &str) -> String {
    let mut output = String::with_capacity(source.len());
    let mut characters = source.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;
    let mut in_block_comment = false;

    while let Some(character) = characters.next() {
        if in_block_comment {
            if character == '*' && characters.peek() == Some(&'/') {
                characters.next();
                in_block_comment = false;
            } else if character == '\n' {
                output.push('\n');
            }
            continue;
        }

        if in_string {
            output.push(character);
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
            continue;
        }

        if character == '"' {
            in_string = true;
            output.push(character);
        } else if character == '/' && characters.peek() == Some(&'/') {
            characters.next();
            for comment_character in characters.by_ref() {
                if comment_character == '\n' {
                    output.push('\n');
                    break;
                }
            }
        } else if character == '/' && characters.peek() == Some(&'*') {
            characters.next();
            in_block_comment = true;
        } else {
            output.push(character);
        }
    }

    output
}

fn is_cmake(path: &Path) -> bool {
    let Some(source) = read(path) else {
        return false;
    };
    let source = source
        .lines()
        .map(|line| line.split('#').next().unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n");
    let lowercase = source.to_ascii_lowercase();
    let mut search_from = 0;

    while let Some(offset) = lowercase[search_from..].find("project") {
        let start = search_from + offset;
        let before_is_identifier = lowercase[..start]
            .chars()
            .next_back()
            .is_some_and(|character| character.is_ascii_alphanumeric() || character == '_');
        let mut open = start + "project".len();
        while lowercase
            .as_bytes()
            .get(open)
            .is_some_and(u8::is_ascii_whitespace)
        {
            open += 1;
        }

        if !before_is_identifier
            && lowercase.as_bytes().get(open) == Some(&b'(')
            && let Some(close_offset) = lowercase[open + 1..].find(')')
        {
            let body = &source[open + 1..open + 1 + close_offset];
            if cmake_project_body_has_version(body) {
                return true;
            }
        }

        search_from = start + "project".len();
    }

    false
}

fn cmake_project_body_has_version(body: &str) -> bool {
    let tokens: Vec<_> = body.split_whitespace().collect();
    if tokens.first().is_none_or(|name| name.is_empty()) {
        return false;
    }

    tokens.windows(2).any(|pair| {
        pair[0].eq_ignore_ascii_case("VERSION")
            && matches!(
                pair[1]
                    .split('.')
                    .map(str::parse::<u64>)
                    .collect::<Result<Vec<_>, _>>(),
                Ok(parts) if (3..=4).contains(&parts.len())
            )
    })
}

fn is_gradle(path: &Path) -> bool {
    let Some(source) = read(path) else {
        return false;
    };
    let source = strip_c_style_comments(&source);

    source.lines().any(|line| {
        if line.starts_with(char::is_whitespace) {
            return false;
        }
        let Some(rest) = line.strip_prefix("version") else {
            return false;
        };
        let rest = rest.trim_start();
        let Some(value) = rest.strip_prefix('=') else {
            return false;
        };
        let value = value.trim().trim_end_matches(';').trim();
        !value.is_empty() && value != "\"\"" && value != "''"
    })
}

fn is_go_mod(path: &Path) -> bool {
    let Some(source) = read(path) else {
        return false;
    };

    let mut found_module = false;
    let mut module_block = false;
    let mut block_paths = 0;

    for line in source.lines() {
        let line = strip_go_line_comment(line).trim();
        if line.is_empty() {
            continue;
        }

        if module_block {
            if line == ")" {
                if block_paths != 1 {
                    return false;
                }
                module_block = false;
                continue;
            }
            if !is_single_go_value(line) {
                return false;
            }
            block_paths += 1;
            if block_paths > 1 {
                return false;
            }
            continue;
        }

        let Some(module) = line.strip_prefix("module") else {
            continue;
        };
        if !module.is_empty()
            && !module.starts_with(char::is_whitespace)
            && !module.starts_with('(')
        {
            continue;
        }
        if found_module {
            return false;
        }

        found_module = true;
        let module = module.trim();
        if module == "(" {
            module_block = true;
        } else if !is_single_go_value(module) {
            return false;
        }
    }

    found_module && !module_block
}

fn strip_go_line_comment(line: &str) -> &str {
    let mut quote = None;
    let mut escaped = false;
    let mut characters = line.char_indices().peekable();

    while let Some((index, character)) = characters.next() {
        if let Some(delimiter) = quote {
            if delimiter == '"' && escaped {
                escaped = false;
            } else if delimiter == '"' && character == '\\' {
                escaped = true;
            } else if character == delimiter {
                quote = None;
            }
        } else if matches!(character, '"' | '`') {
            quote = Some(character);
        } else if character == '/' && characters.peek().is_some_and(|(_, next)| *next == '/') {
            return &line[..index];
        }
    }

    line
}

fn is_single_go_value(value: &str) -> bool {
    let value = value.trim();
    let Some(first) = value.chars().next() else {
        return false;
    };
    if !matches!(first, '"' | '`') {
        return !value.chars().any(char::is_whitespace)
            && !value
                .chars()
                .any(|character| matches!(character, '(' | ')'));
    }

    let mut escaped = false;
    for (index, character) in value.char_indices().skip(1) {
        if first == '"' && escaped {
            escaped = false;
        } else if first == '"' && character == '\\' {
            escaped = true;
        } else if character == first {
            return index > 1 && value[index + character.len_utf8()..].trim().is_empty();
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let unique = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time is after epoch")
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "bumper-package-test-{}-{nanos}-{unique}",
                std::process::id()
            ));
            fs::create_dir(&path).expect("create temp directory");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        fn mkdir(&self, relative: &str) -> PathBuf {
            let path = self.0.join(relative);
            fs::create_dir_all(&path).expect("create test directory");
            path
        }

        fn write(&self, relative: &str, content: &str) -> PathBuf {
            let path = self.0.join(relative);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("create file parent");
            }
            fs::write(&path, content).expect("write test file");
            path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn recognizes_json_and_toml_packages() {
        let temp = TempDir::new();
        let json = temp.mkdir("json");
        temp.write("json/package.json", r#"{"name":"web","version":"1.2.3"}"#);
        let cargo = temp.mkdir("cargo");
        temp.write(
            "cargo/Cargo.toml",
            "[package]\nname = \"cli\"\nversion = \"1.2.3\"\n",
        );
        let python = temp.mkdir("python");
        temp.write(
            "python/pyproject.toml",
            "[project]\nname = \"api\"\nversion = \"1.2.3\"\n",
        );
        let gleam = temp.mkdir("gleam");
        temp.write("gleam/gleam.toml", "name = \"app\"\nversion = \"1.2.3\"\n");

        for directory in [json, cargo, python, gleam] {
            assert!(is_versioned_package(directory));
        }
    }

    #[test]
    fn requires_names_and_string_versions_in_structured_markers() {
        let temp = TempDir::new();
        let missing_name = temp.mkdir("missing-name");
        temp.write("missing-name/package.json", r#"{"version":"1.2.3"}"#);
        let blank_name = temp.mkdir("blank-name");
        temp.write(
            "blank-name/Cargo.toml",
            "[package]\nname = \"  \"\nversion = \"1.2.3\"\n",
        );
        let numeric_version = temp.mkdir("numeric-version");
        temp.write(
            "numeric-version/pyproject.toml",
            "[project]\nname = \"api\"\nversion = 123\n",
        );
        let blank_gleam_version = temp.mkdir("blank-gleam-version");
        temp.write(
            "blank-gleam-version/gleam.toml",
            "name = \"app\"\nversion = \"\"\n",
        );

        for directory in [
            missing_name,
            blank_name,
            numeric_version,
            blank_gleam_version,
        ] {
            assert!(!is_versioned_package(directory));
        }
    }

    #[test]
    fn recognizes_zig_cmake_and_gradle_packages() {
        let temp = TempDir::new();
        let zig = temp.mkdir("zig");
        temp.write(
            "zig/build.zig.zon",
            ".{\n    .name = .demo,\n    .version = \"1.2.3\",\n}\n",
        );
        let cmake = temp.mkdir("cmake");
        temp.write(
            "cmake/CMakeLists.txt",
            "PROJECT(\n  demo\n  VERSION 1.2.3\n  LANGUAGES CXX\n)\n",
        );
        let gradle = temp.mkdir("gradle");
        temp.write("gradle/build.gradle", "version = '1.2.3'\n");
        let kotlin = temp.mkdir("kotlin");
        temp.write("kotlin/build.gradle.kts", "version = \"1.2.3\"\n");

        for directory in [zig, cmake, gradle, kotlin] {
            assert!(is_versioned_package(directory));
        }
    }

    #[test]
    fn recognizes_go_modules_without_a_version() {
        let temp = TempDir::new();
        let module = temp.mkdir("go");
        temp.write("go/go.mod", "module example.com/service\n\ngo 1.24\n");
        let factored = temp.mkdir("factored");
        temp.write(
            "factored/go.mod",
            "module(\n    `example.com/factored` // module path\n)\n\ngo 1.24\n",
        );
        let missing_module = temp.mkdir("missing-module");
        temp.write("missing-module/go.mod", "go 1.24\n");
        let duplicate = temp.mkdir("duplicate");
        temp.write(
            "duplicate/go.mod",
            "module example.com/one\nmodule example.com/two\n",
        );
        let extra_argument = temp.mkdir("extra-argument");
        temp.write(
            "extra-argument/go.mod",
            "module example.com/service extra\n",
        );

        assert!(is_versioned_package(module));
        assert!(is_versioned_package(factored));
        for directory in [missing_module, duplicate, extra_argument] {
            assert!(!is_versioned_package(directory));
        }
    }

    #[test]
    fn ignores_commented_or_nested_text_markers() {
        let temp = TempDir::new();
        let zig = temp.mkdir("zig");
        temp.write(
            "zig/build.zig.zon",
            ".{\n // .name = .demo,\n .version = \"1.2.3\",\n}\n",
        );
        let cmake = temp.mkdir("cmake");
        temp.write(
            "cmake/CMakeLists.txt",
            "# project(demo VERSION 1.2.3)\nproject(demo LANGUAGES C)\n",
        );
        let gradle = temp.mkdir("gradle");
        temp.write(
            "gradle/build.gradle",
            "subprojects {\n    version = '1.2.3'\n}\n",
        );

        for directory in [zig, cmake, gradle] {
            assert!(!is_versioned_package(directory));
        }
    }

    #[test]
    fn ignores_non_marker_files_and_malformed_markers() {
        let temp = TempDir::new();
        let grouping = temp.mkdir("grouping");
        for name in [
            "README.md",
            "action.yml",
            "Cargo.lock",
            "package-lock.json",
            "flake.nix",
        ] {
            temp.write(&format!("grouping/{name}"), "version = \"1.2.3\"\n");
        }
        assert!(!is_versioned_package(grouping));

        let malformed = temp.mkdir("malformed");
        temp.write("malformed/package.json", "not json");
        temp.write("malformed/Cargo.toml", "not = [valid");
        assert!(!is_versioned_package(malformed));
    }

    #[test]
    fn multiple_markers_in_one_directory_are_one_package() {
        let temp = TempDir::new();
        temp.write("package.json", r#"{"name":"web","version":"1.2.3"}"#);
        temp.write(
            "Cargo.toml",
            "[package]\nname = \"core\"\nversion = \"1.2.3\"\n",
        );
        let file = temp.write("src/lib.rs", "");

        let packages = resolve_package_hierarchy(temp.path(), file).expect("resolve packages");
        assert_eq!(packages.len(), 1);
        assert_eq!(packages[0].path, PathBuf::new());
    }

    #[test]
    fn resolves_nearest_package_and_real_ancestors_from_a_file() {
        let temp = TempDir::new();
        temp.write("apps/package.json", r#"{"name":"apps","version":"1.0.0"}"#);
        temp.mkdir("apps/grouping");
        temp.write(
            "apps/grouping/service/Cargo.toml",
            "[package]\nname = \"service\"\nversion = \"1.0.0\"\n",
        );
        let file = temp.write("apps/grouping/service/src/main.rs", "fn main() {}\n");

        let packages = resolve_package_hierarchy(temp.path(), file).expect("resolve packages");
        let paths: Vec<_> = packages
            .iter()
            .map(|package| package.path.as_path())
            .collect();
        assert_eq!(
            paths,
            [
                Path::new("apps/grouping/service"),
                Path::new("apps"),
                Path::new("")
            ]
        );
        assert!(packages.iter().all(|package| package.root.is_absolute()));
    }

    #[test]
    fn a_supplied_package_directory_resolves_to_itself() {
        let temp = TempDir::new();
        let package = temp.mkdir("packages/leaf");
        temp.write(
            "packages/leaf/package.json",
            r#"{"name":"leaf","version":"1.0.0"}"#,
        );

        let packages = resolve_package_hierarchy(temp.path(), &package).expect("resolve packages");
        assert_eq!(packages[0].root, fs::canonicalize(package).unwrap());
        assert_eq!(packages[0].path, Path::new("packages/leaf"));
        assert_eq!(packages.last().unwrap().path, Path::new(""));
    }

    #[test]
    fn root_is_always_included_once_without_a_marker() {
        let temp = TempDir::new();
        let directory = temp.mkdir("one/two");

        let packages = resolve_package_hierarchy(temp.path(), directory).expect("resolve packages");
        assert_eq!(packages.len(), 1);
        assert_eq!(packages[0].root, fs::canonicalize(temp.path()).unwrap());
        assert_eq!(packages[0].path, PathBuf::new());
    }

    #[test]
    fn relative_supplied_paths_are_repository_relative() {
        let temp = TempDir::new();
        temp.write(
            "child/pyproject.toml",
            "[project]\nname = \"child\"\nversion = \"1.0.0\"\n",
        );
        temp.write("child/module.py", "");

        let packages =
            resolve_package_hierarchy(temp.path(), "child/module.py").expect("resolve packages");
        assert_eq!(packages[0].path, Path::new("child"));
    }

    #[test]
    fn rejects_paths_outside_the_repository() {
        let repository = TempDir::new();
        let outside = TempDir::new();
        let outside_file = outside.write("file.txt", "");

        let error = resolve_package_hierarchy(repository.path(), outside_file).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinks_that_escape_the_repository() {
        use std::os::unix::fs::symlink;

        let repository = TempDir::new();
        let outside = TempDir::new();
        symlink(outside.path(), repository.path().join("escape")).expect("create symlink");

        let error = resolve_package_hierarchy(repository.path(), "escape").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }
}

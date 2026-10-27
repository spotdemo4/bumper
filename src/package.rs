use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::Value as JsonValue;
use toml_edit::{DocumentMut, Table};

use crate::go::read_module_path;

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

/// Reads the version from the first valid package marker that stores one.
///
/// Versionless formats such as Go modules return `None`.
pub fn package_version(directory: impl AsRef<Path>) -> Option<String> {
    let directory = directory.as_ref();
    for name in [
        "package.json",
        "Cargo.toml",
        "pyproject.toml",
        "gleam.toml",
        "build.zig.zon",
        "CMakeLists.txt",
        "build.gradle",
        "build.gradle.kts",
        "go.mod",
    ] {
        let path = directory.join(name);
        if !is_package_marker(&path) {
            continue;
        }

        let version = match name {
            "package.json" => read(&path)
                .and_then(|source| serde_json::from_str::<JsonValue>(&source).ok())
                .and_then(|value| value.get("version")?.as_str().map(str::to_string)),
            "Cargo.toml" => toml_version(&path, Some("package")),
            "pyproject.toml" => toml_version(&path, Some("project")),
            "gleam.toml" => toml_version(&path, None),
            "build.zig.zon" => zig_version(&path),
            "CMakeLists.txt" => cmake_version(&path),
            "build.gradle" | "build.gradle.kts" => gradle_version(&path),
            // go.mod has no version, and `is_package_marker` accepts no other names
            _ => None,
        };
        if version.as_deref().is_some_and(is_release_version) {
            return version;
        }
    }
    None
}

fn is_release_version(version: &str) -> bool {
    let mut parts = version.split('.');
    (0..3).all(|_| parts.next().is_some_and(|part| part.parse::<u64>().is_ok()))
        && parts.next().is_none()
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
///
/// # Errors
///
/// Fails when either path can't be canonicalized, when `repo_root` isn't a
/// directory, or when `supplied_path` is outside of it.
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

    let outside_repository = || {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "supplied path is outside the repository root",
        )
    };
    let mut directory = if supplied_path.is_dir() {
        supplied_path.as_path()
    } else {
        supplied_path.parent().ok_or_else(outside_repository)?
    };
    let mut packages = Vec::new();

    loop {
        if directory == repo_root || is_versioned_package(directory) {
            packages.push(Package {
                path: directory
                    .strip_prefix(&repo_root)
                    .map_err(|_| outside_repository())?
                    .to_path_buf(),
                root: directory.to_path_buf(),
            });
        }

        if directory == repo_root {
            break;
        }

        directory = directory.parent().ok_or_else(outside_repository)?;
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

fn toml_version(path: &Path, section: Option<&str>) -> Option<String> {
    let document = parse_toml(path)?;
    let table = match section {
        Some(section) => document.get(section)?.as_table()?,
        None => document.as_table(),
    };
    table.get("version")?.as_str().map(str::to_string)
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

fn zig_version(path: &Path) -> Option<String> {
    let source = strip_c_style_comments(&read(path)?);
    source.lines().find_map(|line| {
        let (field, value) = line.trim().split_once('=')?;
        if field.trim() != ".version" {
            return None;
        }
        zig_value(value.trim().trim_end_matches(',').trim()).map(str::to_string)
    })
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
    cmake_version(path).is_some()
}

fn cmake_version(path: &Path) -> Option<String> {
    let source = read(path)?;
    let source = source
        .lines()
        .map(|line| line.split('#').next().unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n");
    // ASCII lowercasing keeps byte offsets, so matches index into the source too
    let lowercase = source.to_ascii_lowercase();

    for (start, keyword) in lowercase.match_indices("project") {
        let (Some(before), Some(after)) = (
            source.get(..start),
            source
                .get(start..)
                .and_then(|rest| rest.get(keyword.len()..)),
        ) else {
            continue;
        };
        let before_is_identifier = before
            .chars()
            .next_back()
            .is_some_and(|character| character.is_ascii_alphanumeric() || character == '_');

        if !before_is_identifier
            && let Some(arguments) = after
                .trim_start_matches(|character: char| character.is_ascii_whitespace())
                .strip_prefix('(')
            && let Some((body, _)) = arguments.split_once(')')
            && let Some(version) = cmake_project_body_version(body)
        {
            return Some(version.to_string());
        }
    }

    None
}

fn cmake_project_body_version(body: &str) -> Option<&str> {
    let tokens: Vec<_> = body.split_whitespace().collect();
    if tokens.first().is_none_or(|name| name.is_empty()) {
        return None;
    }

    tokens.windows(2).find_map(|pair| {
        let [keyword, version] = pair else {
            return None;
        };
        (keyword.eq_ignore_ascii_case("VERSION")
            && matches!(
                version
                    .split('.')
                    .map(str::parse::<u64>)
                    .collect::<Result<Vec<_>, _>>(),
                Ok(parts) if (3..=4).contains(&parts.len())
            ))
        .then_some(*version)
    })
}

fn is_gradle(path: &Path) -> bool {
    gradle_version(path).is_some()
}

fn gradle_version(path: &Path) -> Option<String> {
    let source = read(path)?;
    let source = strip_c_style_comments(&source);

    source.lines().find_map(|line| {
        if line.starts_with(char::is_whitespace) {
            return None;
        }
        let rest = line.strip_prefix("version")?;
        let rest = rest.trim_start();
        let value = rest.strip_prefix('=')?;
        let value = value.trim().trim_end_matches(';').trim();
        if value.is_empty() || value == "\"\"" || value == "''" {
            return None;
        }
        Some(
            value
                .strip_prefix('"')
                .and_then(|value| value.strip_suffix('"'))
                .or_else(|| {
                    value
                        .strip_prefix('\'')
                        .and_then(|value| value.strip_suffix('\''))
                })
                .unwrap_or(value)
                .to_string(),
        )
    })
}

fn is_go_mod(path: &Path) -> bool {
    read_module_path(path).is_some()
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
            assert!(is_versioned_package(&directory));
            assert_eq!(package_version(directory).as_deref(), Some("1.2.3"));
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
            assert!(is_versioned_package(&directory));
            assert_eq!(package_version(directory).as_deref(), Some("1.2.3"));
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

        assert!(is_versioned_package(&module));
        assert_eq!(package_version(module), None);
        assert!(is_versioned_package(&factored));
        assert_eq!(package_version(factored), None);
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
    fn package_version_skips_an_invalid_earlier_marker() {
        let temp = TempDir::new();
        temp.write("package.json", r#"{"name":"web","version":"next"}"#);
        temp.write(
            "Cargo.toml",
            "[package]\nname = \"core\"\nversion = \"1.2.3\"\n",
        );

        assert_eq!(package_version(temp.path()).as_deref(), Some("1.2.3"));
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

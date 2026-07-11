use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use bumper::bump::{TypedChange, apply_typed_change};

fn copy_fixture(case_name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be after epoch")
        .as_nanos();
    let dest = std::env::temp_dir().join(format!("bumper-case-{case_name}-{nanos}"));

    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join(case_name);
    copy_dir_recursive(&fixture, &dest);

    dest
}

fn copy_dir_recursive(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).expect("failed to create destination directory");
    for entry in fs::read_dir(src).expect("failed to read source directory") {
        let entry = entry.expect("failed to read directory entry");
        let path = entry.path();
        let target = dst.join(entry.file_name());
        if path.is_dir() {
            copy_dir_recursive(&path, &target);
        } else {
            fs::copy(&path, &target).expect("failed to copy fixture file");
        }
    }
}

#[test]
fn node_case_updates_package_files() {
    let dir = copy_fixture("node");

    apply_typed_change(&dir.join("package.json"), "0.13.0", "0.14.0").expect("bump package.json");
    apply_typed_change(&dir.join("package-lock.json"), "0.13.0", "0.14.0")
        .expect("bump package-lock.json");

    let package_json = fs::read_to_string(dir.join("package.json")).expect("read package.json");
    let package_lock =
        fs::read_to_string(dir.join("package-lock.json")).expect("read package-lock.json");

    assert!(package_json.contains("\"version\": \"0.14.0\""));
    assert!(package_lock.contains("\"version\": \"0.14.0\""));
}

#[test]
fn python_case_updates_project_files() {
    let dir = copy_fixture("python");

    apply_typed_change(&dir.join("pyproject.toml"), "0.13.0", "0.14.0")
        .expect("bump pyproject.toml");
    apply_typed_change(&dir.join("uv.lock"), "0.13.0", "0.14.0").expect("bump uv.lock");

    let pyproject = fs::read_to_string(dir.join("pyproject.toml")).expect("read pyproject.toml");
    let uv_lock = fs::read_to_string(dir.join("uv.lock")).expect("read uv.lock");

    assert!(pyproject.contains("version = \"0.14.0\""));
    assert!(
        uv_lock.contains("version = \"0.14.0\""),
        "test package should be bumped"
    );
    assert!(
        uv_lock.contains("version = \"0.13.0\""),
        "dep with same old version should not be changed"
    );
}

#[test]
fn rust_case_updates_cargo_files() {
    let dir = copy_fixture("rust");

    apply_typed_change(&dir.join("Cargo.toml"), "0.13.0", "0.14.0").expect("bump Cargo.toml");
    apply_typed_change(&dir.join("Cargo.lock"), "0.13.0", "0.14.0").expect("bump Cargo.lock");

    let cargo_toml = fs::read_to_string(dir.join("Cargo.toml")).expect("read Cargo.toml");
    let cargo_lock = fs::read_to_string(dir.join("Cargo.lock")).expect("read Cargo.lock");

    assert!(cargo_toml.contains("version = \"0.14.0\""));
    assert!(
        cargo_lock.contains("version = \"0.14.0\""),
        "test package should be bumped"
    );
    assert!(
        cargo_lock.contains("version = \"0.13.0\""),
        "dep with same old version should not be changed"
    );
}

#[test]
fn zig_case_updates_zon_file() {
    let dir = copy_fixture("zig");

    apply_typed_change(&dir.join("build.zig.zon"), "0.13.0", "0.14.0").expect("bump build.zig.zon");

    let zon = fs::read_to_string(dir.join("build.zig.zon")).expect("read build.zig.zon");
    assert!(zon.contains(".version = \"0.14.0\","));
}

#[test]
fn nix_case_updates_flake_files() {
    let dir = copy_fixture("nix");

    apply_typed_change(&dir.join("flake.nix"), "0.13.0", "0.14.0").expect("bump flake.nix");

    let flake_nix = fs::read_to_string(dir.join("flake.nix")).expect("read flake.nix");
    assert!(flake_nix.contains("version = \"0.14.0\""));
}

#[test]
fn gleam_case_updates_gleam_toml() {
    let dir = copy_fixture("gleam");

    apply_typed_change(&dir.join("gleam.toml"), "0.13.0", "0.14.0").expect("bump gleam.toml");

    let gleam_toml = fs::read_to_string(dir.join("gleam.toml")).expect("read gleam.toml");
    assert!(gleam_toml.contains("version = \"0.14.0\""));
}

#[test]
fn gradle_case_updates_project_versions() {
    let dir = copy_fixture("gradle");

    apply_typed_change(&dir.join("build.gradle"), "0.13.0", "0.14.0").expect("bump build.gradle");
    apply_typed_change(&dir.join("build.gradle.kts"), "0.13.0", "0.14.0")
        .expect("bump build.gradle.kts");
    apply_typed_change(&dir.join("gradle.properties"), "0.13.0", "0.14.0")
        .expect("bump gradle.properties");

    let groovy = fs::read_to_string(dir.join("build.gradle")).expect("read build.gradle");
    let kotlin = fs::read_to_string(dir.join("build.gradle.kts")).expect("read build.gradle.kts");
    let properties =
        fs::read_to_string(dir.join("gradle.properties")).expect("read gradle.properties");

    assert!(groovy.contains("version = '0.14.0' // project version"));
    assert!(groovy.contains("id 'com.example.fixture' version '0.13.0'"));
    assert!(groovy.contains("    version = '0.13.0'"));
    assert!(groovy.contains("versionName = '0.13.0'"));
    assert!(groovy.contains("versionCode = 13"));
    assert!(groovy.contains("implementation 'com.example:dependency:0.13.0'"));
    assert!(kotlin.contains("version = \"0.14.0\" // project version"));
    assert!(kotlin.contains("id(\"com.example.fixture\") version \"0.13.0\""));
    assert!(kotlin.contains("    version = \"0.13.0\""));
    assert!(kotlin.contains("versionName = \"0.13.0\""));
    assert!(kotlin.contains("versionCode = 13"));
    assert!(kotlin.contains("implementation(\"com.example:dependency:0.13.0\")"));
    assert!(properties.contains("version = 0.14.0"));
    assert!(properties.contains("dependencyVersion=0.13.0"));

    assert_eq!(
        apply_typed_change(&dir.join("build.gradle"), "0.13.0", "0.15.0")
            .expect("skip mismatched build.gradle version"),
        TypedChange::Unchanged
    );
    assert_eq!(
        apply_typed_change(&dir.join("build.gradle.kts"), "0.13.0", "0.15.0")
            .expect("skip mismatched build.gradle.kts version"),
        TypedChange::Unchanged
    );
    assert_eq!(
        apply_typed_change(&dir.join("gradle.properties"), "0.13.0", "0.15.0")
            .expect("skip mismatched gradle.properties version"),
        TypedChange::Unchanged
    );
}

#[test]
fn cmake_case_updates_project_version() {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be after epoch")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("bumper-case-cmake-{nanos}"));
    fs::create_dir_all(&dir).expect("create cmake fixture directory");
    fs::write(
        dir.join("CMakeLists.txt"),
        r#"cmake_minimum_required(VERSION 3.27)

project(
  bumper_cmake
  VERSION 0.13.0
  DESCRIPTION "Fixture project for bumper"
  LANGUAGES C
)

set(DEPENDENCY_VERSION "0.13.0")
"#,
    )
    .expect("write CMakeLists.txt");

    apply_typed_change(&dir.join("CMakeLists.txt"), "0.13.0", "0.14.0")
        .expect("bump CMakeLists.txt");

    let cmake_lists = fs::read_to_string(dir.join("CMakeLists.txt")).expect("read CMakeLists.txt");
    assert!(cmake_lists.contains("VERSION 0.14.0"));
    assert!(cmake_lists.contains("cmake_minimum_required(VERSION 3.27)"));
    assert!(cmake_lists.contains("set(DEPENDENCY_VERSION \"0.13.0\")"));
}

#[test]
fn readme_case_updates_version_references() {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be after epoch")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("bumper-case-readme-{nanos}"));
    fs::create_dir_all(&dir).expect("create readme fixture directory");
    fs::write(
        dir.join("README.md"),
        r#"# Fixture

Latest tag: v0.13.0

Docker image: ghcr.io/example/fixture:0.13.0
"#,
    )
    .expect("write README.md");

    apply_typed_change(&dir.join("README.md"), "0.13.0", "0.14.0").expect("bump README.md");

    let readme = fs::read_to_string(dir.join("README.md")).expect("read README.md");
    assert!(readme.contains("v0.14.0"));
    assert!(readme.contains("fixture:0.14.0"));
    assert!(!readme.contains("0.13.0"));
}

#[test]
fn action_yaml_updates_literal_version_references() {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be after epoch")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("bumper-case-action-yaml-{nanos}"));
    fs::create_dir_all(&dir).expect("create action fixture directory");
    fs::write(
        dir.join("action.yaml"),
        r#"name: Fixture 0.13.0

metadata:
  image: docker://ghcr.io/example/metadata:0.13.0

runs:
  using: docker
  image: docker://ghcr.io/example/fixture:v0.13.0-alpine
  env:
    FIXTURE_IMAGE: docker://ghcr.io/example/fixture:0.13.0
    FIXTURE_VERSION: 0.13.0
"#,
    )
    .expect("write action.yaml");

    let changed =
        apply_typed_change(&dir.join("action.yaml"), "0.13.0", "0.14.0").expect("bump action.yaml");

    let action = fs::read_to_string(dir.join("action.yaml")).expect("read action.yaml");
    assert_eq!(changed, TypedChange::Changed);
    assert!(action.contains("image: docker://ghcr.io/example/fixture:v0.14.0-alpine"));
    assert!(action.contains("name: Fixture 0.14.0"));
    assert!(action.contains("image: docker://ghcr.io/example/metadata:0.14.0"));
    assert!(action.contains("FIXTURE_IMAGE: docker://ghcr.io/example/fixture:0.14.0"));
    assert!(action.contains("FIXTURE_VERSION: 0.14.0"));
    assert!(!action.contains("0.13.0"));
}

#[test]
fn action_yml_preserves_quoted_image_and_comment() {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be after epoch")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("bumper-case-action-yml-{nanos}"));
    fs::create_dir_all(&dir).expect("create action fixture directory");
    fs::write(
        dir.join("action.yml"),
        r#"runs:
  using: docker
  image: "docker://registry.example.com:5000/example/fixture:0.13.0" # published image
"#,
    )
    .expect("write action.yml");

    let changed =
        apply_typed_change(&dir.join("action.yml"), "0.13.0", "0.14.0").expect("bump action.yml");

    let action = fs::read_to_string(dir.join("action.yml")).expect("read action.yml");
    assert_eq!(changed, TypedChange::Changed);
    assert!(action.contains(
        r#"image: "docker://registry.example.com:5000/example/fixture:0.14.0" # published image"#
    ));
}

#[test]
fn action_yaml_literal_replacement_can_update_non_image_values() {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be after epoch")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("bumper-case-action-dockerfile-{nanos}"));
    fs::create_dir_all(&dir).expect("create action fixture directory");
    fs::write(
        dir.join("action.yaml"),
        r#"runs:
  using: docker
  image: Dockerfile
  env:
    FIXTURE_VERSION: 0.13.0
"#,
    )
    .expect("write action.yaml");

    let changed =
        apply_typed_change(&dir.join("action.yaml"), "0.13.0", "0.14.0").expect("bump action.yaml");

    let action = fs::read_to_string(dir.join("action.yaml")).expect("read action.yaml");
    assert_eq!(changed, TypedChange::Changed);
    assert!(action.contains("image: Dockerfile"));
    assert!(action.contains("FIXTURE_VERSION: 0.14.0"));
}

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use std::collections::HashMap;

use bumper::bump::{
    TypedChange, apply_typed_change, bump_cargo_lock_dependencies, bump_cargo_toml_dependencies,
    bump_package_json_dependencies, bump_package_lock_dependencies,
};

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

    apply_typed_change(&dir.join("package.json"), "0.0.1", "0.0.2").expect("bump package.json");
    apply_typed_change(&dir.join("package-lock.json"), "0.0.1", "0.0.2")
        .expect("bump package-lock.json");

    let package_json = fs::read_to_string(dir.join("package.json")).expect("read package.json");
    let package_lock =
        fs::read_to_string(dir.join("package-lock.json")).expect("read package-lock.json");

    assert!(package_json.contains("\"version\": \"0.0.2\""));
    assert!(package_lock.contains("\"version\": \"0.0.2\""));
}

#[test]
fn node_subpackage_propagates_dependency_versions() {
    let dir = copy_fixture("node");

    // Simulate first-pass bump of the root package `test` from 0.0.1 -> 0.0.2
    apply_typed_change(&dir.join("package.json"), "0.0.1", "0.0.2")
        .expect("bump root package.json");
    apply_typed_change(&dir.join("package-lock.json"), "0.0.1", "0.0.2")
        .expect("bump root package-lock.json");
    // Also bump the sub-package's own version (mirrors first-pass directory scan)
    apply_typed_change(
        &dir.join("packages/consumer/package.json"),
        "0.0.1",
        "0.0.2",
    )
    .expect("bump consumer package.json");
    apply_typed_change(
        &dir.join("packages/consumer/package-lock.json"),
        "0.0.1",
        "0.0.2",
    )
    .expect("bump consumer package-lock.json");

    // Build bumped map as `main.rs` does after first pass (collect package names)
    let mut bumped = HashMap::new();
    bumped.insert(
        "test".to_string(),
        ("0.0.1".to_string(), "0.0.2".to_string()),
    );
    bumped.insert(
        "consumer".to_string(),
        ("0.0.1".to_string(), "0.0.2".to_string()),
    );

    // Second pass: propagate to consumer's dependencies
    let consumer_pkg = dir.join("packages/consumer/package.json");
    let changed =
        bump_package_json_dependencies(&consumer_pkg, &bumped).expect("bump consumer deps");
    assert!(changed, "consumer dependencies should be updated");

    let consumer_content = fs::read_to_string(&consumer_pkg).expect("read consumer package.json");
    assert!(
        consumer_content.contains("\"test\": \"^0.0.2\""),
        "dependencies '^0.0.1' -> '^0.0.2'"
    );
    assert!(
        consumer_content.contains("\"test\": \"0.0.2\""),
        "devDependencies '0.0.1' -> '0.0.2'"
    );
    assert!(
        consumer_content.contains("\"test\": \"~0.0.2\""),
        "peerDependencies '~0.0.1' -> '~0.0.2'"
    );
    assert!(
        consumer_content.contains("\"test\": \">=0.0.2\""),
        "optionalDependencies '>=0.0.1' -> '>=0.0.2'"
    );
    assert!(
        consumer_content.contains("\"other\": \"1.0.0\""),
        "unrelated dep should be untouched"
    );
    // Ensure version field itself was already bumped and not reverted
    assert!(consumer_content.contains("\"version\": \"0.0.2\""));

    // Second pass for package-lock.json: bump installed dependency version
    let lock_path = dir.join("package-lock.json");
    let changed = bump_package_lock_dependencies(&lock_path, &bumped).expect("bump lock deps");
    assert!(changed, "lock dependencies should be updated");

    let lock_content = fs::read_to_string(&lock_path).expect("read lock");
    let v: serde_json::Value = serde_json::from_str(&lock_content).expect("parse lock as json");
    assert_eq!(
        v["packages"]["node_modules/test"]["version"]
            .as_str()
            .unwrap(),
        "0.0.2",
        "packages.node_modules/test version should be bumped"
    );
    assert_eq!(
        v["packages"]["node_modules/other"]["version"]
            .as_str()
            .unwrap(),
        "1.0.0",
        "unrelated package should be untouched"
    );
    assert_eq!(
        v["packages"]["packages/consumer"]["version"]
            .as_str()
            .unwrap(),
        "0.0.2",
        "workspace package consumer version should be bumped via name match"
    );
    assert_eq!(
        v["dependencies"]["test"]["version"].as_str().unwrap(),
        "0.0.2",
        "legacy dependencies.test version should be bumped"
    );
    // Root version already bumped by first pass
    assert_eq!(v["packages"][""]["version"].as_str().unwrap(), "0.0.2");

    // Also verify the consumer's own package-lock.json
    let consumer_lock_path = dir.join("packages/consumer/package-lock.json");
    let changed =
        bump_package_lock_dependencies(&consumer_lock_path, &bumped).expect("bump consumer lock");
    assert!(changed, "consumer lock should be updated");
    let consumer_lock_content =
        fs::read_to_string(&consumer_lock_path).expect("read consumer lock");
    let cv: serde_json::Value =
        serde_json::from_str(&consumer_lock_content).expect("parse consumer lock");
    assert_eq!(
        cv["packages"]["node_modules/test"]["version"]
            .as_str()
            .unwrap(),
        "0.0.2",
        "consumer lock packages.node_modules/test should be bumped"
    );
    assert_eq!(
        cv["dependencies"]["test"]["version"].as_str().unwrap(),
        "0.0.2",
        "consumer lock dependencies.test should be bumped"
    );
    assert_eq!(
        cv["packages"][""]["version"].as_str().unwrap(),
        "0.0.2",
        "consumer lock root version should be bumped (already via first-pass, remains)"
    );
}

#[test]
fn python_case_updates_project_files() {
    let dir = copy_fixture("python");

    apply_typed_change(&dir.join("pyproject.toml"), "0.0.1", "0.0.2").expect("bump pyproject.toml");
    apply_typed_change(&dir.join("uv.lock"), "0.0.1", "0.0.2").expect("bump uv.lock");

    let pyproject = fs::read_to_string(dir.join("pyproject.toml")).expect("read pyproject.toml");
    let uv_lock = fs::read_to_string(dir.join("uv.lock")).expect("read uv.lock");

    assert!(pyproject.contains("version = \"0.0.2\""));
    assert!(
        uv_lock.contains("version = \"0.0.2\""),
        "test package should be bumped"
    );
    assert!(
        uv_lock.contains("version = \"0.0.1\""),
        "dep with same old version should not be changed"
    );
}

#[test]
fn rust_case_updates_cargo_files() {
    let dir = copy_fixture("rust");

    apply_typed_change(&dir.join("Cargo.toml"), "0.0.1", "0.0.2").expect("bump Cargo.toml");
    apply_typed_change(&dir.join("Cargo.lock"), "0.0.1", "0.0.2").expect("bump Cargo.lock");

    let cargo_toml = fs::read_to_string(dir.join("Cargo.toml")).expect("read Cargo.toml");
    let cargo_lock = fs::read_to_string(dir.join("Cargo.lock")).expect("read Cargo.lock");

    assert!(cargo_toml.contains("version = \"0.0.2\""));
    assert!(
        cargo_lock.contains("version = \"0.0.2\""),
        "test package should be bumped"
    );
    assert!(
        cargo_lock.contains("version = \"0.0.1\""),
        "dep with same old version should not be changed"
    );
}

#[test]
fn rust_subpackage_propagates_dependency_versions() {
    let dir = copy_fixture("rust");

    // Simulate first-pass bump of the root crate `test` from 0.0.1 -> 0.0.2
    apply_typed_change(&dir.join("Cargo.toml"), "0.0.1", "0.0.2").expect("bump root Cargo.toml");
    apply_typed_change(&dir.join("Cargo.lock"), "0.0.1", "0.0.2").expect("bump root Cargo.lock");
    apply_typed_change(&dir.join("packages/consumer/Cargo.toml"), "0.0.1", "0.0.2")
        .expect("bump consumer Cargo.toml");
    apply_typed_change(&dir.join("packages/consumer/Cargo.lock"), "0.0.1", "0.0.2")
        .expect("bump consumer Cargo.lock");

    let mut bumped = HashMap::new();
    bumped.insert(
        "test".to_string(),
        ("0.0.1".to_string(), "0.0.2".to_string()),
    );
    bumped.insert(
        "consumer".to_string(),
        ("0.0.1".to_string(), "0.0.2".to_string()),
    );

    let consumer_toml = dir.join("packages/consumer/Cargo.toml");
    let changed =
        bump_cargo_toml_dependencies(&consumer_toml, &bumped).expect("bump consumer deps");
    assert!(
        changed,
        "consumer Cargo.toml dependencies should be updated"
    );

    let content = fs::read_to_string(&consumer_toml).expect("read consumer Cargo.toml");
    // `test = { version = "0.0.1", path = "../.." }` -> `version = "0.0.2"`
    assert!(
        content.contains("test = { version = \"0.0.2\""),
        "inline table version should be bumped: {content}"
    );
    // `test = "0.0.1"` in dev-dependencies
    assert!(
        content.contains("test = \"0.0.2\""),
        "string dep version should be bumped: {content}"
    );
    assert!(
        content.contains("other = \"1.0.0\""),
        "unrelated dep should be untouched: {content}"
    );
    assert!(content.contains("name = \"consumer\""));
    assert!(content.contains("version = \"0.0.2\""));

    // Second-pass for Cargo.lock: update consumer's dependencies on `test`
    // and the consumer's own `version` (since consumer was also bumped via first-pass for its Cargo.toml
    // but not yet for the lock's `[[package]]` entry).
    let lock_path = dir.join("Cargo.lock");
    let changed = bump_cargo_lock_dependencies(&lock_path, &bumped).expect("bump Cargo.lock deps");
    assert!(changed, "Cargo.lock dependencies should be updated");
    let lock_content = fs::read_to_string(&lock_path).expect("read Cargo.lock");
    assert!(
        lock_content.contains("test 0.0.2"),
        "lock should contain bumped dependency version: {lock_content}"
    );
    // Consumer's own version in the lock should now be bumped via second-pass
    assert!(
        lock_content.contains("name = \"consumer\""),
        "consumer package should exist in lock"
    );
    assert!(
        lock_content.contains("version = \"0.0.2\""),
        "lock should contain bumped version 0.0.2"
    );
    // Dep should remain untouched
    assert!(
        lock_content.contains("name = \"dep\""),
        "dep package should still exist"
    );
    // Verify the consumer's dependencies entry specifically
    assert!(
        lock_content.contains("\"test 0.0.2\"") || lock_content.contains("test 0.0.2"),
        "consumer's dependencies should be updated to 0.0.2: {lock_content}"
    );

    // Also verify the consumer's own Cargo.lock (separate from the workspace root)
    let consumer_lock_path = dir.join("packages/consumer/Cargo.lock");
    // Second-pass should bump `test` package version and the dependency string inside consumer's lock
    let _changed =
        bump_cargo_lock_dependencies(&consumer_lock_path, &bumped).expect("bump consumer lock");
    // The consumer lock was already bumped for `consumer` via first-pass, but `test` still needs bump
    // (first-pass only bumps the package matching the adjacent Cargo.toml).
    // After second-pass, both should be 0.0.2 and dependencies updated.
    let consumer_lock_content =
        fs::read_to_string(&consumer_lock_path).expect("read consumer Cargo.lock");
    assert!(
        consumer_lock_content.contains("name = \"consumer\""),
        "consumer lock should contain consumer package"
    );
    assert!(
        consumer_lock_content.contains("name = \"test\""),
        "consumer lock should contain test package"
    );
    // Both packages' versions should be 0.0.2 after propagation (consumer via first-pass, test via second-pass)
    assert!(
        consumer_lock_content.matches("version = \"0.0.2\"").count() >= 2,
        "consumer Cargo.lock should have both packages bumped to 0.0.2: {consumer_lock_content}"
    );
    assert!(
        consumer_lock_content.contains("test 0.0.2"),
        "consumer Cargo.lock dependencies should be updated: {consumer_lock_content}"
    );
}

#[test]
fn zig_case_updates_zon_file() {
    let dir = copy_fixture("zig");

    apply_typed_change(&dir.join("build.zig.zon"), "0.0.1", "0.0.2").expect("bump build.zig.zon");

    let zon = fs::read_to_string(dir.join("build.zig.zon")).expect("read build.zig.zon");
    assert!(zon.contains(".version = \"0.0.2\","));
}

#[test]
fn nix_case_updates_flake_files() {
    let dir = copy_fixture("nix");

    apply_typed_change(&dir.join("flake.nix"), "0.0.1", "0.0.2").expect("bump flake.nix");

    let flake_nix = fs::read_to_string(dir.join("flake.nix")).expect("read flake.nix");
    assert!(flake_nix.contains("version = \"0.0.2\""));
}

#[test]
fn gleam_case_updates_gleam_toml() {
    let dir = copy_fixture("gleam");

    apply_typed_change(&dir.join("gleam.toml"), "0.0.1", "0.0.2").expect("bump gleam.toml");

    let gleam_toml = fs::read_to_string(dir.join("gleam.toml")).expect("read gleam.toml");
    assert!(gleam_toml.contains("version = \"0.0.2\""));
}

#[test]
fn gradle_case_updates_project_versions() {
    let dir = copy_fixture("gradle");

    apply_typed_change(&dir.join("build.gradle"), "0.0.1", "0.0.2").expect("bump build.gradle");
    apply_typed_change(&dir.join("build.gradle.kts"), "0.0.1", "0.0.2")
        .expect("bump build.gradle.kts");
    apply_typed_change(&dir.join("gradle.properties"), "0.0.1", "0.0.2")
        .expect("bump gradle.properties");

    let groovy = fs::read_to_string(dir.join("build.gradle")).expect("read build.gradle");
    let kotlin = fs::read_to_string(dir.join("build.gradle.kts")).expect("read build.gradle.kts");
    let properties =
        fs::read_to_string(dir.join("gradle.properties")).expect("read gradle.properties");

    assert!(groovy.contains("version = '0.0.2' // project version"));
    assert!(groovy.contains("id 'com.example.fixture' version '0.0.1'"));
    assert!(groovy.contains("    version = '0.0.2'"));
    assert!(groovy.contains("versionName = '0.0.1'"));
    assert!(groovy.contains("versionCode = 13"));
    assert!(groovy.contains("implementation 'com.example:dependency:0.0.1'"));
    assert!(kotlin.contains("version = \"0.0.2\" // project version"));
    assert!(kotlin.contains("id(\"com.example.fixture\") version \"0.0.1\""));
    assert!(kotlin.contains("    version = \"0.0.2\""));
    assert!(kotlin.contains("versionName = \"0.0.1\""));
    assert!(kotlin.contains("versionCode = 13"));
    assert!(kotlin.contains("implementation(\"com.example:dependency:0.0.1\")"));
    assert!(properties.contains("version = 0.0.2"));
    assert!(properties.contains("dependencyVersion=0.0.1"));

    assert_eq!(
        apply_typed_change(&dir.join("build.gradle"), "0.0.1", "0.0.3")
            .expect("skip mismatched build.gradle version"),
        TypedChange::Unchanged
    );
    assert_eq!(
        apply_typed_change(&dir.join("build.gradle.kts"), "0.0.1", "0.0.3")
            .expect("skip mismatched build.gradle.kts version"),
        TypedChange::Unchanged
    );
    assert_eq!(
        apply_typed_change(&dir.join("gradle.properties"), "0.0.1", "0.0.3")
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
  VERSION 0.0.1
  DESCRIPTION "Fixture project for bumper"
  LANGUAGES C
)

set(DEPENDENCY_VERSION "0.0.1")
"#,
    )
    .expect("write CMakeLists.txt");

    apply_typed_change(&dir.join("CMakeLists.txt"), "0.0.1", "0.0.2").expect("bump CMakeLists.txt");

    let cmake_lists = fs::read_to_string(dir.join("CMakeLists.txt")).expect("read CMakeLists.txt");
    assert!(cmake_lists.contains("VERSION 0.0.2"));
    assert!(cmake_lists.contains("cmake_minimum_required(VERSION 3.27)"));
    assert!(cmake_lists.contains("set(DEPENDENCY_VERSION \"0.0.1\")"));
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

Latest tag: v0.0.1

Docker image: ghcr.io/example/fixture:0.0.1
"#,
    )
    .expect("write README.md");

    apply_typed_change(&dir.join("README.md"), "0.0.1", "0.0.2").expect("bump README.md");

    let readme = fs::read_to_string(dir.join("README.md")).expect("read README.md");
    assert!(readme.contains("v0.0.2"));
    assert!(readme.contains("fixture:0.0.2"));
    assert!(!readme.contains("0.0.1"));
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
        r#"name: Fixture 0.0.1

metadata:
  image: docker://ghcr.io/example/metadata:0.0.1

runs:
  using: docker
  image: docker://ghcr.io/example/fixture:v0.0.1-alpine
  env:
    FIXTURE_IMAGE: docker://ghcr.io/example/fixture:0.0.1
    FIXTURE_VERSION: 0.0.1
"#,
    )
    .expect("write action.yaml");

    let changed =
        apply_typed_change(&dir.join("action.yaml"), "0.0.1", "0.0.2").expect("bump action.yaml");

    let action = fs::read_to_string(dir.join("action.yaml")).expect("read action.yaml");
    assert_eq!(changed, TypedChange::Changed);
    assert!(action.contains("image: docker://ghcr.io/example/fixture:v0.0.2-alpine"));
    assert!(action.contains("name: Fixture 0.0.2"));
    assert!(action.contains("image: docker://ghcr.io/example/metadata:0.0.2"));
    assert!(action.contains("FIXTURE_IMAGE: docker://ghcr.io/example/fixture:0.0.2"));
    assert!(action.contains("FIXTURE_VERSION: 0.0.2"));
    assert!(!action.contains("0.0.1"));
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
  image: "docker://registry.example.com:5000/example/fixture:0.0.1" # published image
"#,
    )
    .expect("write action.yml");

    let changed =
        apply_typed_change(&dir.join("action.yml"), "0.0.1", "0.0.2").expect("bump action.yml");

    let action = fs::read_to_string(dir.join("action.yml")).expect("read action.yml");
    assert_eq!(changed, TypedChange::Changed);
    assert!(action.contains(
        r#"image: "docker://registry.example.com:5000/example/fixture:0.0.2" # published image"#
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
    FIXTURE_VERSION: 0.0.1
"#,
    )
    .expect("write action.yaml");

    let changed =
        apply_typed_change(&dir.join("action.yaml"), "0.0.1", "0.0.2").expect("bump action.yaml");

    let action = fs::read_to_string(dir.join("action.yaml")).expect("read action.yaml");
    assert_eq!(changed, TypedChange::Changed);
    assert!(action.contains("image: Dockerfile"));
    assert!(action.contains("FIXTURE_VERSION: 0.0.2"));
}

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use bumper::bump::apply_typed_change;

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

    apply_typed_change(&dir.join("package.json"), "0.11.0", "0.12.0").expect("bump package.json");
    apply_typed_change(&dir.join("package-lock.json"), "0.11.0", "0.12.0")
        .expect("bump package-lock.json");

    let package_json = fs::read_to_string(dir.join("package.json")).expect("read package.json");
    let package_lock =
        fs::read_to_string(dir.join("package-lock.json")).expect("read package-lock.json");

    assert!(package_json.contains("\"version\": \"0.12.0\""));
    assert!(package_lock.contains("\"version\": \"0.12.0\""));
}

#[test]
fn python_case_updates_project_files() {
    let dir = copy_fixture("python");

    apply_typed_change(&dir.join("pyproject.toml"), "0.11.0", "0.12.0")
        .expect("bump pyproject.toml");
    apply_typed_change(&dir.join("uv.lock"), "0.11.0", "0.12.0").expect("bump uv.lock");

    let pyproject = fs::read_to_string(dir.join("pyproject.toml")).expect("read pyproject.toml");
    let uv_lock = fs::read_to_string(dir.join("uv.lock")).expect("read uv.lock");

    assert!(pyproject.contains("version = \"0.12.0\""));
    assert!(
        uv_lock.contains("version = \"0.12.0\""),
        "test package should be bumped"
    );
    assert!(
        uv_lock.contains("version = \"0.11.0\""),
        "dep with same old version should not be changed"
    );
}

#[test]
fn rust_case_updates_cargo_files() {
    let dir = copy_fixture("rust");

    apply_typed_change(&dir.join("Cargo.toml"), "0.11.0", "0.12.0").expect("bump Cargo.toml");
    apply_typed_change(&dir.join("Cargo.lock"), "0.11.0", "0.12.0").expect("bump Cargo.lock");

    let cargo_toml = fs::read_to_string(dir.join("Cargo.toml")).expect("read Cargo.toml");
    let cargo_lock = fs::read_to_string(dir.join("Cargo.lock")).expect("read Cargo.lock");

    assert!(cargo_toml.contains("version = \"0.12.0\""));
    assert!(
        cargo_lock.contains("version = \"0.12.0\""),
        "test package should be bumped"
    );
    assert!(
        cargo_lock.contains("version = \"0.11.0\""),
        "dep with same old version should not be changed"
    );
}

#[test]
fn zig_case_updates_zon_file() {
    let dir = copy_fixture("zig");

    apply_typed_change(&dir.join("build.zig.zon"), "0.10.4", "0.11.0").expect("bump build.zig.zon");

    let zon = fs::read_to_string(dir.join("build.zig.zon")).expect("read build.zig.zon");
    assert!(zon.contains(".version = \"0.11.0\","));
}

#[test]
fn nix_case_updates_flake_files() {
    let dir = copy_fixture("nix");

    apply_typed_change(&dir.join("flake.nix"), "0.11.2", "0.12.0").expect("bump flake.nix");

    let flake_nix = fs::read_to_string(dir.join("flake.nix")).expect("read flake.nix");
    assert!(flake_nix.contains("version = \"0.12.0\""));
}

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn run_cmd(cwd: &Path, args: &[&str]) {
    let status = Command::new(args[0])
        .current_dir(cwd)
        .args(&args[1..])
        .status()
        .expect("failed to run command");
    assert!(status.success(), "command failed: {}", args.join(" "));
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

fn setup_repo(case_name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be after epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("bumper-case-{case_name}-{nanos}"));
    fs::create_dir_all(&root).expect("failed to create temp root");

    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join(case_name);
    let case_dir = root.join(case_name);
    copy_dir_recursive(&fixture, &case_dir);

    run_cmd(&root, &["git", "init"]);
    run_cmd(&root, &["git", "config", "user.name", "bumper-tests"]);
    run_cmd(
        &root,
        &[
            "git",
            "config",
            "user.email",
            "bumper-tests@example.invalid",
        ],
    );

    run_cmd(&root, &["git", "add", "."]);
    run_cmd(&root, &["git", "commit", "-m", "chore: init"]);

    // bumper runs `git fetch --all --tags`; providing an origin keeps this deterministic.
    let remote = root.join("remote.git");
    run_cmd(
        &root,
        &[
            "git",
            "init",
            "--bare",
            remote.to_str().expect("valid remote path"),
        ],
    );
    run_cmd(
        &root,
        &[
            "git",
            "remote",
            "add",
            "origin",
            remote.to_str().expect("valid remote path"),
        ],
    );

    root
}

fn run_bumper_case(root: &Path, case_name: &str) {
    let status = Command::new(env!("CARGO_BIN_EXE_bumper"))
        .current_dir(root)
        .arg(case_name)
        .env("COMMIT", "false")
        .env("TAG", "false")
        .env("PUSH", "false")
        .status()
        .expect("failed to run bumper binary");

    assert!(
        status.success(),
        "bumper exited with failure for {case_name}"
    );
}

#[test]
fn node_case_updates_package_files() {
    let root = setup_repo("node");

    run_cmd(&root, &["git", "tag", "-a", "v0.11.0", "-m", "tag"]);
    run_cmd(
        &root,
        &["git", "commit", "--allow-empty", "-m", "feat: node changes"],
    );

    run_bumper_case(&root, "node");

    let package_json =
        fs::read_to_string(root.join("node/package.json")).expect("read package.json");
    let package_lock =
        fs::read_to_string(root.join("node/package-lock.json")).expect("read package-lock.json");

    assert!(package_json.contains("\"version\": \"0.12.0\""));
    assert!(package_lock.contains("\"version\": \"0.12.0\""));
}

#[test]
fn python_case_updates_project_files() {
    let root = setup_repo("python");

    run_cmd(&root, &["git", "tag", "-a", "v0.11.0", "-m", "tag"]);
    run_cmd(
        &root,
        &[
            "git",
            "commit",
            "--allow-empty",
            "-m",
            "feat: python changes",
        ],
    );

    run_bumper_case(&root, "python");

    let pyproject =
        fs::read_to_string(root.join("python/pyproject.toml")).expect("read pyproject.toml");
    let uv_lock = fs::read_to_string(root.join("python/uv.lock")).expect("read uv.lock");

    assert!(pyproject.contains("version = \"0.12.0\""));
    assert!(uv_lock.contains("version = \"0.12.0\""));
}

#[test]
fn rust_case_updates_cargo_files() {
    let root = setup_repo("rust");

    run_cmd(&root, &["git", "tag", "-a", "v0.11.0", "-m", "tag"]);
    run_cmd(
        &root,
        &["git", "commit", "--allow-empty", "-m", "feat: rust changes"],
    );

    run_bumper_case(&root, "rust");

    let cargo_toml = fs::read_to_string(root.join("rust/Cargo.toml")).expect("read Cargo.toml");
    let cargo_lock = fs::read_to_string(root.join("rust/Cargo.lock")).expect("read Cargo.lock");

    assert!(cargo_toml.contains("version = \"0.12.0\""));
    assert!(cargo_lock.contains("version = \"0.12.0\""));
}

#[test]
fn zig_case_updates_zon_file() {
    let root = setup_repo("zig");

    run_cmd(&root, &["git", "tag", "-a", "v0.11.0", "-m", "tag"]);
    run_cmd(
        &root,
        &["git", "commit", "--allow-empty", "-m", "feat: zig changes"],
    );

    run_bumper_case(&root, "zig");

    let zon = fs::read_to_string(root.join("zig/build.zig.zon")).expect("read build.zig.zon");
    assert!(zon.contains(".version = \"0.12.0\","));
}

#[test]
fn nix_case_updates_flake_files() {
    let root = setup_repo("nix");

    // Ensure flake.lock has the old semantic version so literal replacement can update it.
    let lock_path = root.join("nix/flake.lock");
    let lock = fs::read_to_string(&lock_path).expect("read flake.lock");
    let lock = lock.replacen(
        "\"version\": 7",
        "\"bumperVersion\": \"0.11.2\",\n  \"version\": 7",
        1,
    );
    fs::write(&lock_path, lock).expect("write flake.lock");

    run_cmd(&root, &["git", "add", "nix/flake.lock"]);
    run_cmd(
        &root,
        &["git", "commit", "-m", "chore: add lock version marker"],
    );

    run_cmd(&root, &["git", "tag", "-a", "v0.11.2", "-m", "tag"]);
    run_cmd(
        &root,
        &["git", "commit", "--allow-empty", "-m", "feat: nix changes"],
    );

    run_bumper_case(&root, "nix");

    let flake_nix = fs::read_to_string(root.join("nix/flake.nix")).expect("read flake.nix");
    let flake_lock = fs::read_to_string(root.join("nix/flake.lock")).expect("read flake.lock");

    assert!(flake_nix.contains("version = \"0.12.0\""));
    assert!(flake_lock.contains("\"bumperVersion\": \"0.12.0\""));
}

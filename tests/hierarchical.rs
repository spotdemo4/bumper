use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use git2::{Oid, Repository, Signature};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let unique = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time is after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "bumper-{name}-{}-{nanos}-{unique}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("create temp directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn commit_all(repo: &Repository, message: &str) -> Oid {
    let mut config = repo.config().expect("open repository config");
    config
        .set_str("user.name", "Bumper Test")
        .expect("set test user name");
    config
        .set_str("user.email", "bumper@example.com")
        .expect("set test user email");
    let mut index = repo.index().expect("open index");
    index
        .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
        .expect("add files");
    index.write().expect("write index");
    let tree_id = index.write_tree().expect("write tree");
    let tree = repo.find_tree(tree_id).expect("find tree");
    let signature = Signature::now("Bumper Test", "bumper@example.com").expect("signature");
    let parents = repo
        .head()
        .ok()
        .and_then(|head| head.peel_to_commit().ok())
        .into_iter()
        .collect::<Vec<_>>();
    let parent_refs = parents.iter().collect::<Vec<_>>();
    repo.commit(
        Some("HEAD"),
        &signature,
        &signature,
        message,
        &tree,
        &parent_refs,
    )
    .expect("commit")
}

fn tag(repo: &Repository, name: &str, commit: Oid) {
    let commit = repo.find_commit(commit).expect("find commit");
    repo.tag_lightweight(name, commit.as_object(), false)
        .expect("create tag");
}

#[test]
fn go_module_without_a_file_version_bumps_its_existing_package_tag() {
    let local = TempDir::new("go-module-local");
    let remote = TempDir::new("go-module-remote");
    let repo = Repository::init(local.path()).expect("init repository");
    Repository::init_bare(remote.path()).expect("init bare remote");
    repo.remote("origin", remote.path().to_str().expect("UTF-8 remote path"))
        .expect("add remote");
    fs::create_dir_all(local.path().join("packages/service")).expect("create Go module");
    let go_mod = "module example.com/service\n\ngo 1.24\n";
    fs::write(local.path().join("packages/service/go.mod"), go_mod).expect("write go.mod");
    fs::write(
        local.path().join("packages/service/main.go"),
        "package main\n\nfunc main() {}\n",
    )
    .expect("write Go source");
    let baseline = commit_all(&repo, "chore: initialize Go module");
    tag(&repo, "v1.0.0", baseline);
    tag(&repo, "packages/service/v2.3.4", baseline);
    fs::write(
        local.path().join("packages/service/main.go"),
        "package main\n\nfunc main() { println(\"fixed\") }\n",
    )
    .expect("update Go source");
    let changed = commit_all(&repo, "fix: repair service");

    let output = Command::new(env!("CARGO_BIN_EXE_bumper"))
        .current_dir(local.path())
        .args(["--no-push", "packages/service/go.mod"])
        .output()
        .expect("run bumper");

    assert!(
        output.status.success(),
        "bumper failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Release plan:\n. (1.0.0 -> 1.0.1, patch)"));
    assert!(stdout.contains("propagated from child packages/service"));
    assert!(stdout.contains("service (2.3.4 -> 2.3.5, patch)"));
    assert!(stdout.contains("direct patch"));
    assert!(!stdout.contains("Would you like to proceed?"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("Would you like to proceed?"));
    let package_tag = repo
        .revparse_single("refs/tags/packages/service/v2.3.5")
        .expect("find Go package tag")
        .peel_to_commit()
        .expect("peel Go package tag");
    assert_eq!(package_tag.id(), changed);
    repo.revparse_single("refs/tags/v1.0.1")
        .expect("find propagated root tag");
    assert_eq!(
        fs::read_to_string(local.path().join("packages/service/go.mod")).expect("read go.mod"),
        go_mod
    );
}

#[test]
fn untagged_package_uses_its_manifest_version() {
    let local = TempDir::new("untagged-manifest-local");
    let remote = TempDir::new("untagged-manifest-remote");
    let repo = Repository::init(local.path()).expect("init repository");
    Repository::init_bare(remote.path()).expect("init bare remote");
    repo.remote("origin", remote.path().to_str().expect("UTF-8 remote path"))
        .expect("add remote");
    fs::create_dir_all(local.path().join("packages/service")).expect("create package");
    fs::write(
        local.path().join("package.json"),
        "{\n  \"name\": \"root\",\n  \"version\": \"1.0.0\"\n}\n",
    )
    .expect("write root manifest");
    fs::write(
        local.path().join("packages/service/package.json"),
        "{\n  \"name\": \"service\",\n  \"version\": \"2.3.4\"\n}\n",
    )
    .expect("write service manifest");
    fs::write(
        local.path().join("packages/service/README.md"),
        "service v2.3.4\n",
    )
    .expect("write service readme");
    let baseline = commit_all(&repo, "chore: initialize workspace");
    tag(&repo, "v1.0.0", baseline);
    fs::write(
        local.path().join("packages/service/README.md"),
        "updated service v2.3.4\n",
    )
    .expect("update service readme");
    commit_all(&repo, "fix: repair service");

    let output = Command::new(env!("CARGO_BIN_EXE_bumper"))
        .current_dir(local.path())
        .args(["--no-push", "packages/service/README.md"])
        .output()
        .expect("run bumper");

    assert!(
        output.status.success(),
        "bumper failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let package_tag = repo
        .revparse_single("refs/tags/packages/service/v2.3.5")
        .expect("find package tag")
        .peel_to_commit()
        .expect("peel package tag");
    assert_eq!(package_tag.id(), repo.head().unwrap().target().unwrap());
    repo.revparse_single("refs/tags/v1.0.1")
        .expect("find propagated root tag");
    assert!(
        fs::read_to_string(local.path().join("packages/service/package.json"))
            .expect("read service manifest")
            .contains("\"version\": \"2.3.5\"")
    );
}

#[test]
fn untagged_versionless_package_uses_nearest_parent_version() {
    let local = TempDir::new("untagged-go-local");
    let remote = TempDir::new("untagged-go-remote");
    let repo = Repository::init(local.path()).expect("init repository");
    Repository::init_bare(remote.path()).expect("init bare remote");
    repo.remote("origin", remote.path().to_str().expect("UTF-8 remote path"))
        .expect("add remote");
    fs::create_dir_all(local.path().join("packages/parent/service")).expect("create Go package");
    fs::write(
        local.path().join("package.json"),
        "{\n  \"name\": \"root\",\n  \"version\": \"1.0.0\"\n}\n",
    )
    .expect("write root manifest");
    fs::write(
        local.path().join("packages/parent/package.json"),
        "{\n  \"name\": \"parent\",\n  \"version\": \"2.3.4\"\n}\n",
    )
    .expect("write parent manifest");
    let go_mod = "module example.com/service\n\ngo 1.24\n";
    fs::write(local.path().join("packages/parent/service/go.mod"), go_mod).expect("write go.mod");
    fs::write(
        local.path().join("packages/parent/service/main.go"),
        "package main\n\nfunc main() {}\n",
    )
    .expect("write Go source");
    let baseline = commit_all(&repo, "chore: initialize workspace");
    tag(&repo, "v1.0.0", baseline);
    tag(&repo, "packages/parent/v2.3.4", baseline);
    fs::write(
        local.path().join("packages/parent/service/main.go"),
        "package main\n\nfunc main() { println(\"fixed\") }\n",
    )
    .expect("update Go source");
    commit_all(&repo, "fix: repair service");

    let output = Command::new(env!("CARGO_BIN_EXE_bumper"))
        .current_dir(local.path())
        .args(["--no-push", "packages/parent/service/go.mod"])
        .output()
        .expect("run bumper");

    assert!(
        output.status.success(),
        "bumper failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    repo.revparse_single("refs/tags/packages/parent/service/v2.3.5")
        .expect("find inherited-version package tag");
    repo.revparse_single("refs/tags/packages/parent/v2.3.5")
        .expect("find propagated parent tag");
    repo.revparse_single("refs/tags/v1.0.1")
        .expect("find propagated root tag");
    assert_eq!(
        fs::read_to_string(local.path().join("packages/parent/service/go.mod"))
            .expect("read go.mod"),
        go_mod
    );
}

#[test]
fn untagged_root_uses_manifest_version_and_full_history() {
    let local = TempDir::new("untagged-root-local");
    let remote = TempDir::new("untagged-root-remote");
    let repo = Repository::init(local.path()).expect("init repository");
    Repository::init_bare(remote.path()).expect("init bare remote");
    repo.remote("origin", remote.path().to_str().expect("UTF-8 remote path"))
        .expect("add remote");
    fs::write(
        local.path().join("package.json"),
        "{\n  \"name\": \"root\",\n  \"version\": \"1.0.0\"\n}\n",
    )
    .expect("write root manifest");
    fs::write(local.path().join("README.md"), "root v1.0.0\n").expect("write readme");
    commit_all(&repo, "chore: initialize repository");
    fs::write(local.path().join("README.md"), "fixed root v1.0.0\n").expect("update readme");
    commit_all(&repo, "fix: repair root");

    let output = Command::new(env!("CARGO_BIN_EXE_bumper"))
        .current_dir(local.path())
        .args(["--no-push", "README.md"])
        .output()
        .expect("run bumper");

    assert!(
        output.status.success(),
        "bumper failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("README.md (1.0.0 -> 1.0.1)"));
    assert!(stdout.contains("package.json (1.0.0 -> 1.0.1)"));
    assert!(!stdout.contains("Would you like to proceed?"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("Would you like to proceed?"));
    repo.revparse_single("refs/tags/v1.0.1")
        .expect("find first root tag");
    assert!(
        fs::read_to_string(local.path().join("package.json"))
            .expect("read root manifest")
            .contains("\"version\": \"1.0.1\"")
    );
}

#[test]
fn ignored_directory_changes_do_not_trigger_or_receive_a_release() {
    let local = TempDir::new("ignored-directory-local");
    let remote = TempDir::new("ignored-directory-remote");
    let repo = Repository::init(local.path()).expect("init repository");
    Repository::init_bare(remote.path()).expect("init bare remote");
    repo.remote("origin", remote.path().to_str().expect("UTF-8 remote path"))
        .expect("add remote");
    fs::create_dir(local.path().join("generated")).expect("create generated directory");
    fs::write(
        local.path().join("package.json"),
        "{\n  \"name\": \"root\",\n  \"version\": \"1.0.0\"\n}\n",
    )
    .expect("write root package");
    fs::write(
        local.path().join("generated/package.json"),
        "{\n  \"name\": \"generated\",\n  \"version\": \"1.0.0\"\n}\n",
    )
    .expect("write generated package");
    let initial = commit_all(&repo, "chore: initial");
    tag(&repo, "v1.0.0", initial);
    fs::write(
        local.path().join("generated/package.json"),
        "{\n  \"name\": \"generated\",\n  \"version\": \"1.0.0\",\n  \"generated\": true\n}\n",
    )
    .expect("update generated package");
    commit_all(&repo, "feat: regenerate package");

    let output = Command::new(env!("CARGO_BIN_EXE_bumper"))
        .current_dir(local.path())
        .args(["--no-push", "--ignore-directories", "generated"])
        .output()
        .expect("run bumper");

    assert!(
        output.status.success(),
        "bumper failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout)
            .contains("no new impactful commits for the selected packages")
    );
    assert!(
        fs::read_to_string(local.path().join("package.json"))
            .expect("read root package")
            .contains("\"version\": \"1.0.0\"")
    );
    assert!(
        fs::read_to_string(local.path().join("generated/package.json"))
            .expect("read generated package")
            .contains("\"version\": \"1.0.0\"")
    );
}

#[test]
fn readme_uses_nearest_package_stream_and_propagates_to_root() {
    let local = TempDir::new("hierarchical-local");
    let remote = TempDir::new("hierarchical-remote");
    let repo = Repository::init(local.path()).expect("init repository");
    Repository::init_bare(remote.path()).expect("init bare remote");
    repo.remote("origin", remote.path().to_str().expect("UTF-8 remote path"))
        .expect("add remote");
    fs::create_dir_all(local.path().join("packages/consumer")).expect("create package");
    fs::write(
        local.path().join("package.json"),
        "{\n  \"name\": \"root\",\n  \"version\": \"1.0.0\"\n}\n",
    )
    .expect("write root manifest");
    fs::write(
        local.path().join("packages/consumer/package.json"),
        "{\n  \"name\": \"consumer\",\n  \"version\": \"2.0.0\"\n}\n",
    )
    .expect("write package manifest");
    fs::write(
        local.path().join("packages/consumer/README.md"),
        "consumer v2.0.0\n",
    )
    .expect("write package readme");
    let baseline = commit_all(&repo, "chore: initialize packages");
    tag(&repo, "v1.0.0", baseline);
    tag(&repo, "packages/consumer/v2.0.0", baseline);

    fs::write(
        local.path().join("packages/consumer/README.md"),
        "updated consumer v2.0.0\n",
    )
    .expect("update package readme");
    commit_all(&repo, "fix: update consumer documentation");
    fs::write(
        local.path().join("package-lock.json"),
        r#"{"name":"root","version":"1.0.0"}"#,
    )
    .expect("write untracked lockfile");

    let output = Command::new(env!("CARGO_BIN_EXE_bumper"))
        .current_dir(local.path())
        .args(["--no-push", "packages/consumer/README.md"])
        .output()
        .expect("run bumper");

    assert!(
        output.status.success(),
        "bumper failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("propagated from child packages/consumer"),
        "{stdout}"
    );
    assert!(
        stdout.contains("consumer (2.0.0 -> 2.0.1, patch)"),
        "{stdout}"
    );
    assert!(stdout.contains("direct patch"), "{stdout}");
    let root_tag = repo
        .revparse_single("refs/tags/v1.0.1")
        .expect("find propagated root tag")
        .peel_to_commit()
        .expect("peel root tag");
    let package_tag = repo
        .revparse_single("refs/tags/packages/consumer/v2.0.1")
        .expect("find package tag")
        .peel_to_commit()
        .expect("peel package tag");
    assert_eq!(root_tag.id(), package_tag.id());
    assert!(repo.find_reference("refs/tags/packages/v1.0.1").is_err());
    assert!(
        fs::read_to_string(local.path().join("package.json"))
            .expect("read root manifest")
            .contains("\"version\": \"1.0.1\"")
    );
    assert!(
        fs::read_to_string(local.path().join("packages/consumer/package.json"))
            .expect("read package manifest")
            .contains("\"version\": \"2.0.1\"")
    );
    assert_eq!(
        fs::read_to_string(local.path().join("packages/consumer/README.md"))
            .expect("read package readme"),
        "updated consumer v2.0.1\n"
    );
    assert_eq!(
        fs::read_to_string(local.path().join("package-lock.json"))
            .expect("read untracked lockfile"),
        r#"{"name":"root","version":"1.0.0"}"#
    );
    assert!(
        repo.status_file(Path::new("package-lock.json"))
            .expect("read lockfile status")
            .contains(git2::Status::WT_NEW)
    );
}

#[test]
fn dependency_updates_release_an_unselected_sibling_package() {
    let local = TempDir::new("dependency-local");
    let remote = TempDir::new("dependency-remote");
    let repo = Repository::init(local.path()).expect("init repository");
    Repository::init_bare(remote.path()).expect("init bare remote");
    repo.remote("origin", remote.path().to_str().expect("UTF-8 remote path"))
        .expect("add remote");
    fs::create_dir_all(local.path().join("packages/library")).expect("create library");
    fs::create_dir_all(local.path().join("packages/app")).expect("create app");
    fs::write(
        local.path().join("package.json"),
        "{\n  \"name\": \"root\",\n  \"version\": \"1.0.0\"\n}\n",
    )
    .expect("write root manifest");
    fs::write(
        local.path().join("packages/library/package.json"),
        "{\n  \"name\": \"library\",\n  \"version\": \"2.0.0\"\n}\n",
    )
    .expect("write library manifest");
    fs::write(
        local.path().join("packages/library/README.md"),
        "library v2.0.0\n",
    )
    .expect("write library readme");
    fs::write(
        local.path().join("packages/app/package.json"),
        "{\n  \"name\": \"app\",\n  \"version\": \"3.0.0\",\n  \"dependencies\": {\"library\": \"2.0.0\"}\n}\n",
    )
    .expect("write app manifest");
    fs::write(local.path().join("packages/app/README.md"), "app v3.0.0\n")
        .expect("write app readme");
    let baseline = commit_all(&repo, "chore: initialize workspace");
    tag(&repo, "v1.0.0", baseline);
    tag(&repo, "packages/library/v2.0.0", baseline);
    tag(&repo, "packages/app/v3.0.0", baseline);
    fs::write(
        local.path().join("packages/library/README.md"),
        "updated library v2.0.0\n",
    )
    .expect("update library readme");
    commit_all(&repo, "fix: update library");

    let output = Command::new(env!("CARGO_BIN_EXE_bumper"))
        .current_dir(local.path())
        .args([
            "--no-push",
            "packages/library/README.md",
            "packages/app/README.md",
        ])
        .output()
        .expect("run bumper");

    assert!(
        output.status.success(),
        "bumper failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(
            "propagated from dependency packages/library (library via packages/app/package.json)"
        ),
        "{stdout}"
    );
    assert!(
        stdout.contains("package.json (3.0.0 -> 3.0.1; library 2.0.0 -> 2.0.1)"),
        "{stdout}"
    );
    repo.revparse_single("refs/tags/packages/library/v2.0.1")
        .expect("find library tag");
    repo.revparse_single("refs/tags/packages/app/v3.0.1")
        .expect("find dependent app tag");
    repo.revparse_single("refs/tags/v1.0.1")
        .expect("find root tag");
    let commit = repo
        .head()
        .expect("find HEAD")
        .peel_to_commit()
        .expect("peel HEAD to commit");
    assert_eq!(
        commit.message().expect("read commit message"),
        "bump: v1.0.1\n\nPackage: packages/app/v3.0.1\nPackage: packages/library/v2.0.1"
    );
    let root_tag = repo
        .revparse_single("refs/tags/v1.0.1")
        .expect("find root tag object");
    assert_eq!(
        root_tag
            .as_tag()
            .expect("root tag is annotated")
            .message()
            .expect("read tag message"),
        Some(
            "bump: v1.0.0 -> v1.0.1, packages/app/v3.0.0 -> packages/app/v3.0.1, packages/library/v2.0.0 -> packages/library/v2.0.1"
        )
    );
    let app = fs::read_to_string(local.path().join("packages/app/package.json"))
        .expect("read app manifest");
    assert!(app.contains("\"version\": \"3.0.1\""));
    assert!(app.contains("\"library\": \"2.0.1\""));
    assert_eq!(
        fs::read_to_string(local.path().join("packages/app/README.md")).expect("read app readme"),
        "app v3.0.1\n"
    );
}

#[test]
fn explicit_file_is_added_to_discovered_package_targets() {
    let local = TempDir::new("pathless-packages-local");
    let remote = TempDir::new("pathless-packages-remote");
    let repo = Repository::init(local.path()).expect("init repository");
    Repository::init_bare(remote.path()).expect("init bare remote");
    repo.remote("origin", remote.path().to_str().expect("UTF-8 remote path"))
        .expect("add remote");
    fs::create_dir_all(local.path().join("packages/library/plugin"))
        .expect("create nested library package");
    fs::create_dir_all(local.path().join("packages/app")).expect("create app package");
    fs::create_dir_all(local.path().join("packages/idle")).expect("create idle package");
    fs::write(
        local.path().join("package.json"),
        "{\n  \"name\": \"root\",\n  \"version\": \"1.0.0\"\n}\n",
    )
    .expect("write root manifest");
    fs::write(
        local.path().join("packages/library/package.json"),
        "{\n  \"name\": \"library\",\n  \"version\": \"2.0.0\"\n}\n",
    )
    .expect("write library manifest");
    fs::write(
        local.path().join("packages/library/README.md"),
        "library v2.0.0\n",
    )
    .expect("write library readme");
    fs::write(
        local.path().join("packages/library/plugin/Cargo.toml"),
        "[package]\nname = \"plugin\"\nversion = \"3.0.0\"\n",
    )
    .expect("write plugin manifest");
    fs::write(
        local.path().join("packages/library/plugin/README.md"),
        "plugin v3.0.0\n",
    )
    .expect("write plugin readme");
    fs::write(
        local.path().join("packages/library/plugin/version.txt"),
        "plugin version 3.0.0\n",
    )
    .expect("write explicit version file");
    fs::write(
        local.path().join("packages/app/package.json"),
        "{\n  \"name\": \"app\",\n  \"version\": \"4.0.0\",\n  \"dependencies\": {\"library\": \"2.0.0\"}\n}\n",
    )
    .expect("write app manifest");
    fs::write(
        local.path().join("packages/idle/package.json"),
        "{\n  \"name\": \"idle\",\n  \"version\": \"5.0.0\"\n}\n",
    )
    .expect("write idle manifest");
    let baseline = commit_all(&repo, "chore: initialize workspace");
    tag(&repo, "v1.0.0", baseline);
    tag(&repo, "packages/library/v2.0.0", baseline);
    tag(&repo, "packages/library/plugin/v3.0.0", baseline);
    tag(&repo, "packages/app/v4.0.0", baseline);
    tag(&repo, "packages/idle/v5.0.0", baseline);
    fs::write(
        local.path().join("packages/library/README.md"),
        "updated library v2.0.0\n",
    )
    .expect("update library readme");
    fs::write(
        local.path().join("packages/library/plugin/README.md"),
        "updated plugin v3.0.0\n",
    )
    .expect("update plugin readme");
    commit_all(&repo, "fix: update library packages");

    let output = Command::new(env!("CARGO_BIN_EXE_bumper"))
        .current_dir(local.path())
        .args([
            "--no-push",
            "packages/library/plugin/version.txt",
            "./packages/library/plugin/version.txt",
        ])
        .output()
        .expect("run bumper");

    assert!(
        output.status.success(),
        "bumper failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("packages/idle: (skipped)"));
    assert!(!stdout.contains("packages/app: (skipped)"));
    assert!(!stdout.contains('\x1b'));
    repo.revparse_single("refs/tags/packages/library/v2.0.1")
        .expect("find library tag");
    repo.revparse_single("refs/tags/packages/library/plugin/v3.0.1")
        .expect("find nested plugin tag");
    repo.revparse_single("refs/tags/packages/app/v4.0.1")
        .expect("find dependent app tag");
    repo.revparse_single("refs/tags/v1.0.1")
        .expect("find propagated root tag");
    let app = fs::read_to_string(local.path().join("packages/app/package.json"))
        .expect("read app manifest");
    assert!(app.contains("\"version\": \"4.0.1\""));
    assert!(app.contains("\"library\": \"2.0.1\""));
    assert_eq!(
        fs::read_to_string(local.path().join("packages/library/README.md"))
            .expect("read library readme"),
        "updated library v2.0.1\n"
    );
    assert_eq!(
        fs::read_to_string(local.path().join("packages/library/plugin/README.md"))
            .expect("read plugin readme"),
        "updated plugin v3.0.1\n"
    );
    assert_eq!(
        fs::read_to_string(local.path().join("packages/library/plugin/version.txt"))
            .expect("read explicit version file"),
        "plugin version 3.0.1\n"
    );
    assert!(repo.statuses(None).expect("read status").is_empty());
}

#[test]
fn explicit_parent_file_is_additive_to_nested_package_detection() {
    let local = TempDir::new("unselected-child-local");
    let remote = TempDir::new("unselected-child-remote");
    let repo = Repository::init(local.path()).expect("init repository");
    Repository::init_bare(remote.path()).expect("init bare remote");
    repo.remote("origin", remote.path().to_str().expect("UTF-8 remote path"))
        .expect("add remote");
    fs::create_dir_all(local.path().join("packages/parent/plugins/cache"))
        .expect("create nested package");
    fs::write(
        local.path().join("package.json"),
        "{\n  \"name\": \"root\",\n  \"version\": \"1.0.0\"\n}\n",
    )
    .expect("write root manifest");
    fs::write(
        local.path().join("packages/parent/package.json"),
        "{\n  \"name\": \"parent\",\n  \"version\": \"2.0.0\"\n}\n",
    )
    .expect("write parent manifest");
    fs::write(
        local.path().join("packages/parent/README.md"),
        "parent v2.0.0\n",
    )
    .expect("write parent readme");
    fs::write(
        local
            .path()
            .join("packages/parent/plugins/cache/package.json"),
        "{\n  \"name\": \"cache\",\n  \"version\": \"3.0.0\"\n}\n",
    )
    .expect("write cache manifest");
    fs::write(
        local.path().join("packages/parent/plugins/cache/README.md"),
        "cache v3.0.0\n",
    )
    .expect("write cache readme");
    let baseline = commit_all(&repo, "chore: initialize packages");
    tag(&repo, "v1.0.0", baseline);
    tag(&repo, "packages/parent/v2.0.0", baseline);
    fs::write(
        local.path().join("packages/parent/plugins/cache/README.md"),
        "new cache feature v3.0.0\n",
    )
    .expect("update cache readme");
    commit_all(&repo, "feat: add cache feature");
    let before = repo.head().expect("read HEAD").target();

    let output = Command::new(env!("CARGO_BIN_EXE_bumper"))
        .current_dir(local.path())
        .args(["--no-push", "packages/parent/README.md"])
        .output()
        .expect("run bumper");

    assert!(
        output.status.success(),
        "bumper failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.contains("packages/parent: (skipped)"));
    assert!(
        stdout.contains("propagated from child packages/parent/plugins/cache"),
        "{stdout}"
    );
    assert!(
        stdout.contains("propagated from child packages/parent"),
        "{stdout}"
    );
    repo.revparse_single("refs/tags/packages/parent/plugins/cache/v3.1.0")
        .expect("find nested cache tag");
    repo.revparse_single("refs/tags/packages/parent/v2.0.1")
        .expect("find parent tag");
    repo.revparse_single("refs/tags/v1.0.1")
        .expect("find root tag");
    assert_ne!(repo.head().expect("read HEAD").target(), before);
    assert_eq!(
        fs::read_to_string(local.path().join("packages/parent/README.md"))
            .expect("read parent readme"),
        "parent v2.0.1\n"
    );
}

#[test]
fn package_impact_path_releases_nested_package_and_propagates_to_ancestors() {
    let local = TempDir::new("package-impact-path-local");
    let remote = TempDir::new("package-impact-path-remote");
    let repo = Repository::init(local.path()).expect("init repository");
    Repository::init_bare(remote.path()).expect("init bare remote");
    repo.remote("origin", remote.path().to_str().expect("UTF-8 remote path"))
        .expect("add remote");
    fs::create_dir_all(local.path().join("packages/parent/artifact"))
        .expect("create nested artifact package");
    fs::create_dir_all(local.path().join("packages/native/src"))
        .expect("create native source package");
    fs::create_dir_all(local.path().join("packages/idle")).expect("create idle package");
    fs::write(
        local.path().join("package.json"),
        "{\n  \"name\": \"root\",\n  \"version\": \"1.0.0\"\n}\n",
    )
    .expect("write root manifest");
    fs::write(
        local.path().join("packages/parent/package.json"),
        "{\n  \"name\": \"parent\",\n  \"version\": \"2.0.0\"\n}\n",
    )
    .expect("write parent manifest");
    fs::write(
        local
            .path()
            .join("packages/parent/artifact/package.json"),
        "{\n  \"name\": \"artifact\",\n  \"version\": \"3.0.0\",\n  \"bumper\": {\n    \"impactPaths\": [\"../../../packages/native/src\"],\n    \"futureOption\": true\n  }\n}\n",
    )
    .expect("write artifact manifest");
    fs::write(
        local.path().join("packages/native/package.json"),
        "{\n  \"name\": \"native\",\n  \"version\": \"4.0.0\"\n}\n",
    )
    .expect("write native manifest");
    fs::write(
        local.path().join("packages/native/src/lib.rs"),
        "pub fn native() {}\n",
    )
    .expect("write native source");
    fs::write(
        local.path().join("packages/idle/package.json"),
        "{\n  \"name\": \"idle\",\n  \"version\": \"5.0.0\"\n}\n",
    )
    .expect("write idle manifest");
    let baseline = commit_all(&repo, "chore: initialize native artifact workspace");
    tag(&repo, "v1.0.0", baseline);
    tag(&repo, "packages/parent/v2.0.0", baseline);
    tag(&repo, "packages/parent/artifact/v3.0.0", baseline);
    tag(&repo, "packages/native/v4.0.0", baseline);
    tag(&repo, "packages/idle/v5.0.0", baseline);

    fs::write(
        local.path().join("packages/native/src/lib.rs"),
        "pub fn native() { println!(\"feature\"); }\n",
    )
    .expect("update native source");
    commit_all(&repo, "feat: add native capability");

    let output = Command::new(env!("CARGO_BIN_EXE_bumper"))
        .current_dir(local.path())
        .args(["--no-push"])
        .output()
        .expect("run bumper");

    assert!(
        output.status.success(),
        "bumper failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("packages/idle: (skipped)"), "{stdout}");
    assert!(
        stdout.contains("artifact (3.0.0 -> 3.1.0, minor)"),
        "{stdout}"
    );
    assert!(
        stdout.contains("native (4.0.0 -> 4.1.0, minor)"),
        "{stdout}"
    );
    assert!(
        stdout.contains("propagated from child packages/parent/artifact"),
        "{stdout}"
    );
    repo.revparse_single("refs/tags/packages/parent/artifact/v3.1.0")
        .expect("find artifact minor tag");
    repo.revparse_single("refs/tags/packages/parent/v2.0.1")
        .expect("find propagated parent patch tag");
    repo.revparse_single("refs/tags/packages/native/v4.1.0")
        .expect("find native package minor tag");
    repo.revparse_single("refs/tags/v1.0.1")
        .expect("find propagated root patch tag");
    assert!(
        repo.find_reference("refs/tags/packages/idle/v5.0.1")
            .is_err()
    );
    let artifact: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(local.path().join("packages/parent/artifact/package.json"))
            .expect("read artifact manifest"),
    )
    .expect("parse updated artifact manifest");
    assert_eq!(artifact["version"], "3.1.0");
    assert_eq!(
        artifact["bumper"],
        serde_json::json!({
            "impactPaths": ["../../../packages/native/src"],
            "futureOption": true,
        })
    );
}

#[test]
fn untagged_dependent_uses_manifest_version() {
    let local = TempDir::new("missing-dependent-tag-local");
    let remote = TempDir::new("missing-dependent-tag-remote");
    let repo = Repository::init(local.path()).expect("init repository");
    Repository::init_bare(remote.path()).expect("init bare remote");
    repo.remote("origin", remote.path().to_str().expect("UTF-8 remote path"))
        .expect("add remote");
    fs::create_dir_all(local.path().join("packages/library")).expect("create library");
    fs::create_dir_all(local.path().join("packages/app")).expect("create app");
    fs::write(
        local.path().join("package.json"),
        "{\n  \"name\": \"root\",\n  \"version\": \"1.0.0\"\n}\n",
    )
    .expect("write root manifest");
    fs::write(
        local.path().join("packages/library/package.json"),
        "{\n  \"name\": \"library\",\n  \"version\": \"2.0.0\"\n}\n",
    )
    .expect("write library manifest");
    fs::write(
        local.path().join("packages/library/README.md"),
        "library v2.0.0\n",
    )
    .expect("write library readme");
    let app_path = local.path().join("packages/app/package.json");
    let app_source = "{\n  \"name\": \"app\",\n  \"version\": \"3.0.0\",\n  \"dependencies\": {\"library\": \"2.0.0\"}\n}\n";
    fs::write(&app_path, app_source).expect("write app manifest");
    let baseline = commit_all(&repo, "chore: initialize workspace");
    tag(&repo, "v1.0.0", baseline);
    tag(&repo, "packages/library/v2.0.0", baseline);
    fs::write(
        local.path().join("packages/library/README.md"),
        "updated library v2.0.0\n",
    )
    .expect("update library readme");
    commit_all(&repo, "fix: update library");
    let output = Command::new(env!("CARGO_BIN_EXE_bumper"))
        .current_dir(local.path())
        .args(["--no-push", "packages/library/README.md"])
        .output()
        .expect("run bumper");

    assert!(
        output.status.success(),
        "bumper failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(
            "propagated from dependency packages/library (library via packages/app/package.json)"
        ),
        "{stdout}"
    );
    assert!(
        fs::read_to_string(local.path().join("packages/library/package.json"))
            .expect("read library manifest")
            .contains("\"version\": \"2.0.1\"")
    );
    let app = fs::read_to_string(&app_path).expect("read app manifest");
    assert!(app.contains("\"version\": \"3.0.1\""));
    assert!(
        app.contains("\"library\": \"2.0.1\""),
        "dependency was not updated: {app}"
    );
    assert!(repo.statuses(None).expect("read status").is_empty());
    repo.revparse_single("refs/tags/v1.0.1")
        .expect("find propagated root tag");
    repo.revparse_single("refs/tags/packages/library/v2.0.1")
        .expect("find library tag");
    repo.revparse_single("refs/tags/packages/app/v3.0.1")
        .expect("find dependent tag");
}

#[test]
fn dependency_propagation_tree_shows_each_immediate_hop() {
    let local = TempDir::new("dependency-chain-local");
    let remote = TempDir::new("dependency-chain-remote");
    let repo = Repository::init(local.path()).expect("init repository");
    Repository::init_bare(remote.path()).expect("init bare remote");
    repo.remote("origin", remote.path().to_str().expect("UTF-8 remote path"))
        .expect("add remote");
    for package in ["library", "app", "cli"] {
        fs::create_dir_all(local.path().join(format!("packages/{package}")))
            .expect("create package");
    }
    fs::write(
        local.path().join("package.json"),
        "{\n  \"name\": \"root\",\n  \"version\": \"1.0.0\"\n}\n",
    )
    .expect("write root manifest");
    fs::write(
        local.path().join("packages/library/package.json"),
        "{\n  \"name\": \"library\",\n  \"version\": \"2.0.0\"\n}\n",
    )
    .expect("write library manifest");
    fs::write(
        local.path().join("packages/library/README.md"),
        "library v2.0.0\n",
    )
    .expect("write library readme");
    fs::write(
        local.path().join("packages/app/package.json"),
        "{\n  \"name\": \"app\",\n  \"version\": \"3.0.0\",\n  \"dependencies\": {\"library\": \"2.0.0\"}\n}\n",
    )
    .expect("write app manifest");
    fs::write(
        local.path().join("packages/cli/package.json"),
        "{\n  \"name\": \"cli\",\n  \"version\": \"4.0.0\",\n  \"dependencies\": {\"app\": \"3.0.0\"}\n}\n",
    )
    .expect("write cli manifest");
    let baseline = commit_all(&repo, "chore: initialize dependency chain");
    tag(&repo, "v1.0.0", baseline);
    tag(&repo, "packages/library/v2.0.0", baseline);
    tag(&repo, "packages/app/v3.0.0", baseline);
    tag(&repo, "packages/cli/v4.0.0", baseline);
    fs::write(
        local.path().join("packages/library/README.md"),
        "updated library v2.0.0\n",
    )
    .expect("update library readme");
    commit_all(&repo, "fix: update library");

    let output = Command::new(env!("CARGO_BIN_EXE_bumper"))
        .current_dir(local.path())
        .args(["--no-push"])
        .output()
        .expect("run bumper");

    assert!(
        output.status.success(),
        "bumper failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(
            "propagated from dependency packages/library (library via packages/app/package.json)"
        ),
        "{stdout}"
    );
    assert!(
        stdout.contains(
            "propagated from dependency packages/app (app via packages/cli/package.json)"
        ),
        "{stdout}"
    );
    assert!(
        !stdout.contains(
            "propagated from dependency packages/library (library via packages/cli/package.json)"
        ),
        "{stdout}"
    );
    assert!(
        stdout.contains("package.json (4.0.0 -> 4.0.1; app 3.0.0 -> 3.0.1)"),
        "{stdout}"
    );

    for tag_name in [
        "v1.0.1",
        "packages/library/v2.0.1",
        "packages/app/v3.0.1",
        "packages/cli/v4.0.1",
    ] {
        repo.revparse_single(&format!("refs/tags/{tag_name}"))
            .unwrap_or_else(|_| panic!("find {tag_name}"));
    }
    let app = fs::read_to_string(local.path().join("packages/app/package.json"))
        .expect("read app manifest");
    assert!(app.contains("\"version\": \"3.0.1\""));
    assert!(app.contains("\"library\": \"2.0.1\""));
    let cli = fs::read_to_string(local.path().join("packages/cli/package.json"))
        .expect("read cli manifest");
    assert!(cli.contains("\"version\": \"4.0.1\""));
    assert!(cli.contains("\"app\": \"3.0.1\""));
    assert!(repo.statuses(None).expect("read status").is_empty());
}

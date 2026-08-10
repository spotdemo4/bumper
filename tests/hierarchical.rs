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
    repo.revparse_single("refs/tags/packages/library/v2.0.1")
        .expect("find library tag");
    repo.revparse_single("refs/tags/packages/app/v3.0.1")
        .expect("find dependent app tag");
    repo.revparse_single("refs/tags/v1.0.1")
        .expect("find root tag");
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
fn unselected_nested_package_changes_do_not_bump_the_parent_directly() {
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
    assert!(
        repo.revparse_single("refs/tags/packages/parent/v2.1.0")
            .is_err()
    );
    assert!(repo.revparse_single("refs/tags/v1.1.0").is_err());
    assert_eq!(repo.head().expect("read HEAD").target(), before);
}

#[test]
fn missing_dependent_tag_fails_before_modifying_files() {
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
    let before_head = repo.head().expect("read HEAD").target();
    let before_library = fs::read_to_string(local.path().join("packages/library/package.json"))
        .expect("read library manifest");

    let output = Command::new(env!("CARGO_BIN_EXE_bumper"))
        .current_dir(local.path())
        .args(["--no-push", "packages/library/README.md"])
        .output()
        .expect("run bumper");

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("package stream 'packages/app'"),
        "unexpected stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(repo.head().expect("read HEAD").target(), before_head);
    assert_eq!(
        fs::read_to_string(local.path().join("packages/library/package.json"))
            .expect("read library manifest"),
        before_library
    );
    assert_eq!(
        fs::read_to_string(&app_path).expect("read app manifest"),
        app_source
    );
    assert!(repo.statuses(None).expect("read status").is_empty());
    assert!(repo.find_reference("refs/tags/v1.0.1").is_err());
    assert!(
        repo.find_reference("refs/tags/packages/library/v2.0.1")
            .is_err()
    );
}

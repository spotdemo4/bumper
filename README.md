# git version bumper

[![check](https://trev.zip/llc/bumper/actions/workflows/check.yaml/badge.svg?branch=main&logo=forgejo&logoColor=%23bac2de&label=check&labelColor=%23313244)](https://trev.zip/llc/bumper/actions?workflow=check.yaml)
[![vulnerable](https://trev.zip/llc/bumper/actions/workflows/vulnerable.yaml/badge.svg?branch=main&logo=forgejo&logoColor=%23bac2de&label=vulnerable&labelColor=%23313244)](https://trev.zip/llc/bumper/actions?workflow=vulnerable.yaml)
[![nixpkgs](https://nix-shield.trev.zip/?url=https://trev.zip/llc/bumper/raw/branch/main/flake.lock&input=nixpkgs&logoColor=%23bac2de&labelColor=%23313244&color=%235277C3)](https://nixos.org/)
[![rust](https://shields.trev.zip/badge/dynamic/toml?url=https%3A%2F%2Ftrev.zip%2Fllc%2Fbumper%2Fraw%2Fbranch%2Fmain%2FCargo.toml&query=%24.package.rust-version&logo=rust&logoColor=%23bac2de&label=version&labelColor=%23313244&color=%23D34516)](https://releases.rs/)

- determines the [semantic versioning](https://semver.org/) impact (major, minor or patch) of the [conventional commits](https://www.conventionalcommits.org) since each package's last git tag
- increments the git tag by the impact (v0.0.1 -> PATCH -> v0.0.2)
- creates hierarchical tags for packages in subdirectories (`packages/consumer/v0.0.2`)
- releases containing packages with a patch whenever one of their nested packages releases
- releases [groups](#version-groups) of packages together at one version, while other packages keep their own versions
- reads repository settings from a [`bumper.toml`](#configuration-file) at the repository root
- applies the version bump to every discovered package and to additional files given as arguments (`bumper [files...]`)
- applies the version bump in directories given as arguments to supported project files (`README.md`, `action.yaml`, `action.yml`, `openapi*.yaml` (case-sensitive, top-level `info.version` only), `package.json`, `package-lock.json`, `build.gradle`, `build.gradle.kts`, `gradle.properties`, `Cargo.toml`, `Cargo.lock`, `pyproject.toml`, `uv.lock`, `build.zig.zon`, `gleam.toml`, `*.app.src` (`{vsn, "x.y.z"}`), `*.nix` (`version = "x.y.z";`), `CMakeLists.txt`)
- skips configured directories, likely vendored paths (`vendor`, `node_modules`), and symlinks during directory scans
- commits the bumped files and pushes them with the new git tag

This works well as a github action. Have it run on every push to main and it will bump the version for every change, or run it on a schedule to increase the version if there were any new changes.

## Usage

```elm
bumper [paths...]
bumper init
```

Before changing files, bumper reports selected packages without impactful changes as skipped, then prints one release-plan tree containing every releasing package, why its bump happened, and every file it will update. Interactive terminals color each package group and ask for confirmation; non-interactive runs such as CI print the same information as a plain tree and continue without prompting. Set `NO_COLOR` to disable colors.

```text
Release plan:
. (1.0.0 -> 1.0.1, patch)
|-- propagated from child packages/library
`-- packages
    |-- app (3.0.0 -> 3.0.1, patch)
    |   |-- propagated from dependency packages/library (library via packages/app/package.json)
    |   `-- package.json (3.0.0 -> 3.0.1; library 2.0.0 -> 2.0.1)
    `-- library (2.0.0 -> 2.0.1, patch)
        |-- direct patch
        `-- package.json (2.0.0 -> 2.0.1)
```

Direct conventional-commit impacts, forced bumps, immediate child releases, and dependency updates are retained as separate reasons, so a package can show more than one cause. Ancestor and dependency chains use immediate edges (`library -> app -> cli`) rather than flattening every downstream release back to the original package.

Use `--ignore-directories generated,packages/legacy` or set `IGNORE_DIRECTORIES` to a whitespace- or newline-separated list of repository-relative directories. Ignored trees do not contribute commits, packages, version files, or dependency updates. Explicitly selected paths inside them are skipped.

### Configuration file

Settings that describe the repository rather than a single run can live in a `bumper.toml` at the repository root. Run `bumper init` to create one that lists every setting with its default, commented out, and suggests a [version group](#version-groups) of the repository's nested packages; it won't replace an existing `bumper.toml`. The `#:schema` comment gives editors with TOML language servers such as [Tombi](https://tombi-toml.github.io/tombi/) or [Taplo](https://taplo.tamasfe.dev/) completion and validation from [`bumper.schema.json`](bumper.schema.json):

```toml
#:schema https://trev.zip/llc/bumper/raw/branch/main/bumper.schema.json

paths = ["README.md", "action.yaml"] # additional files or directories to bump
ignore-directories = ["generated", "packages/legacy"]
major-types = []
minor-types = ["feat"]
patch-types = ["fix"]
skip-scopes = ["ci", "docs"]
breaking-changes = true
fixed-version = false

[groups.sdk]
packages = ["packages/sdk-*", "packages/cli"]
```

A `!` after the commit type or scope (`feat!:`, `feat(api)!:`) or a `BREAKING CHANGE:` footer bumps the major version whatever the commit type, and `major-types` adds commit types that always bump it. Set `breaking-changes = false`, pass `--no-breaking-changes` or set `BREAKING_CHANGES=false` to ignore those markers, so `feat!:` bumps the minor version like any other `feat`.

Every key is optional, and unknown keys are an error. Paths are relative to the repository root. Command line flags beat environment variables, which beat `bumper.toml`, which beats the defaults; `paths` and `ignore-directories` add to the paths given by flags and environment variables instead. Unlike the flags and environment variables, `bumper.toml` can set a commit type or scope list to empty (`skip-scopes = []`). Settings for a single run, such as forcing, committing, tagging and pushing, are only flags and environment variables.

### Package tags

Each supplied file or directory is an additional bump target belonging to its nearest containing package. A package is a directory with a valid `package.json`, `Cargo.toml`, `pyproject.toml`, `gleam.toml`, `build.zig.zon`, `CMakeLists.txt`, `build.gradle`, `build.gradle.kts`, `go.mod`, or an Erlang/OTP application resource file in its `src` directory (`src/my_app.app.src` with a string `{vsn, "x.y.z"}`; rebar3's `{vsn, git}` is not a package). Go modules use their package tag as the version because `go.mod` has no project version to update; see [Go modules](#go-modules) for their major versions. The repository root is always the root package. Documentation, lockfiles, action files, generic Nix files, and grouping directories do not create package boundaries.

Bumper always checks every discovered package for a release. Supplying paths adds files or directories to the normal package-root scans; explicit files also support literal version replacement when their format has no dedicated writer. Dependency updates can release packages that reference another bumped package: dependency versions in `package.json`, `package-lock.json`, `Cargo.toml` and `Cargo.lock` follow the packages they name, and in a Gleam `manifest.toml` the locked `version` of each `source = "local"` package follows the package its `path` points at. Hex and git packages in the manifest are left alone, even when they share the old version.

OpenAPI documents are version targets owned by their nearest package; they do not create package boundaries.

Because every package is selected, `--force` forces a bump for every discovered package rather than only packages containing supplied paths, except [unversioned packages](#unversioned-packages). To force specific packages instead, pass their repository-relative directories with `--force-package packages/app` (repeatable or comma-separated; `.` is the root) or set `FORCE_PACKAGES` to a whitespace- or newline-separated list. A path that is not a discovered package is an error. Forcing a nested package still releases its ancestors and dependents with a patch, as with any other release. Forced bumps default to PATCH; use `--force-bump-type minor` (or `major`) or set `FORCE_BUMP_TYPE` to choose a different minimum impact.

To release with a different bump type than the commits imply, pass `--bump-type minor` (or `patch`, `major`) or set `BUMP_TYPE`. It replaces the commit-derived impact of every package that has impactful commits, so a breaking change can ship as a minor release; packages without impactful commits still skip. Forced bumps keep their own `--force-bump-type`, ancestors and dependents still release with a patch, and the release plan shows the replaced impact (`direct minor (bump type overrides major)`).

Run `bumper -i` in a terminal to pick forced packages from a checklist. Each package shows the release it would get without forcing, packages from `--force` or `--force-package` start checked, and the usual release plan and confirmation follow the selection. Press Esc to abort.

Root releases use `vX.Y.Z`. Packages below the repository root use their repository-relative path as the tag prefix:

```text
v3.0.1
packages/consumer/v1.2.3
packages/consumer/plugins/cache/v0.5.0
```

For example, `bumper packages/consumer/README.md` adds the README to the normal repository-wide bump and assigns it to `packages/consumer`; it does not create a README-specific tag. If the consumer releases, each real package containing it also releases. Descendant propagation is a patch unless the containing package has its own minor or major conventional commit. Shared ancestors release only once.

When a package stream has no tag yet, bumper uses the version in its package manifest. Versionless package formats such as Go modules inherit the version of their nearest package ancestor. The nearest ancestor tag is used as the release-history boundary; if no ancestor has a tag, bumper considers the package's full git history. Existing root repositories continue to use their current `vX.Y.Z` tags.

### Go modules

When a Go module releases, other Go modules in the repository that build against its local copy, through a `replace` directive pointing at its directory or the repository's `go.work`, require the new version (`require example.com/lib v1.4.0`), and so does a versioned `replace` of it. Those dependent modules release with a patch, so their published `go.mod` never requires an older version than the one they were built against:

```text
Release plan:
. (1.3.2 -> 1.4.0, minor)
|-- direct minor
|-- propagated from child otel
`-- otel (1.3.2 -> 1.3.3, patch)
    |-- propagated from dependency root (example.com/lib via otel/go.mod)
    `-- go.mod (example.com/lib 1.3.2 -> 1.4.0)
```

The `require` names the new version before its tag exists. Go never downloads a module replaced with a local directory, so the dependent still builds and its `go.sum` stays the same. Modules that require a published version without a `replace` or `go.work` keep it.

Go puts the major version of a module at v2 or above into its module path (`example.com/lib/v2`), and ignores tags whose major version doesn't match the path. When a Go module releases at a major version its path doesn't allow, such as `example.com/lib` going from `1.4.0` to `2.0.0`, bumper moves the module to the new path:

- its `go.mod` `module` directive becomes `example.com/lib/v2`
- import declarations of its packages (`example.com/lib`, `example.com/lib/internal/text`) move to the new path in its `.go` files, except in `testdata` and in directories the go command ignores
- other Go modules in the repository that build against the local copy, through a `replace` directive pointing at its directory or the repository's `go.work`, get the same import changes, and their `require` and `replace` directives move to the new path at the new version (`example.com/lib/v2 v2.0.0`)
- Go packages of generated protobuf code inside the module move to the new path in tracked `.proto` files (`option go_package = "example.com/lib/gen/foo/v1;foov1";`) and buf templates (`buf.gen.yaml`, `buf.gen.*.yaml`): `managed` `go_package_prefix` and `go_package` values and plugin `opt` entries such as `module=example.com/lib/gen` or `Mfoo.proto=example.com/lib/gen/foo`. Generated `.go` files get the same import changes as other `.go` files, so code generation doesn't need to run again

Those dependent modules release with a patch, and the release plan lists every rewritten file:

```text
Release plan:
. (1.4.0 -> 2.0.0, major)
|-- direct major
|-- propagated from child tools
|-- go.mod (example.com/lib -> example.com/lib/v2)
`-- tools (0.4.0 -> 0.4.1, patch)
    |-- propagated from dependency root (example.com/lib via tools/go.mod)
    |-- go.mod (example.com/lib -> example.com/lib/v2)
    `-- main.go (example.com/lib -> example.com/lib/v2)
```

Modules that require a published version keep using it, since v1 and v2 are different modules to Go. A dependent that requires the new version without a `replace` or `go.work` can't download it until the tag is pushed, so run `go mod tidy` there afterwards. The same applies whenever a release moves a module path, including a nested Go module that inherits a v2 or higher version from its ancestor, or one released with `--fixed-version`.

A module in a major version subdirectory, such as `v2/go.mod` with `module example.com/lib/v2`, is tagged with its parent directory's prefix (`v2.1.0` rather than `v2/v2.1.0`), as Go expects. The parent package then ignores tags of that major version. Such a module can't move to a new major version, so a breaking change fails with an error asking you to copy it to a `v3` directory. Releases also fail when a module path names a newer major version than the release (`example.com/lib/v3` releasing `2.1.0`), or when a `gopkg.in/name.vN` path doesn't match the release.

### Unversioned packages

A package below the repository root whose stream has no tag yet and whose version is `0.0.0` is unversioned: it never releases from its own commits, child releases, dependency updates, `--force` or `--fixed-version`, and the release plan reports it as `packages/web: (skipped, unversioned 0.0.0)`. Its commits count for its nearest versioned ancestor instead, as if the files belonged to that package, so a `feat` commit there releases the ancestor with a minor bump. Versioned packages nested inside it keep their own commits. Dependency versions in its manifests are still updated when the packages they reference release, so workspaces stay consistent.

To start releasing one, force it by name with `--force-package packages/web` or check it in `bumper -i`. That releases it like any other forced package (`0.0.0 -> 0.0.1`, or `0.1.0` with `--force-bump-type minor` or a `feat` commit) and creates its first tag, after which it versions normally and claims its own commits. The root package is never unversioned; a `0.0.0` root releases as usual.

### Version groups

Packages listed in the same `[groups.<name>]` of [`bumper.toml`](#configuration-file) always release together at one version. Other packages keep their own versions.

```toml
[groups.sdk]
packages = ["packages/sdk-*", "packages/cli"]
```

Each entry in `packages` is a repository-relative package directory (`.` is the root) or a pattern, where `*` matches within one directory name and `**` matches any number of directories. Every entry must match a discovered package, and a package can belong to only one group.

When any member of a group releases, whether from its own commits, forcing, a nested package or a dependency outside the group, every member releases at the highest impact in the group. That impact is applied to the highest current version among the members, so no member moves backwards. Releases then propagate from the group's members to their ancestors and dependents as usual, and those can release another group in turn. The release plan names the group and the members whose changes set its impact:

```text
Release plan:
. (1.0.0 -> 1.0.1, patch)
|-- propagated from child packages/app
|-- propagated from child packages/library
`-- packages
    |-- app (2.0.0 -> 3.1.0, minor)
    |   |-- direct patch
    |   |-- group sdk (minor from packages/library)
    |   `-- package.json (2.0.0 -> 3.1.0; library 3.0.0 -> 3.1.0)
    `-- library (3.0.0 -> 3.1.0, minor)
        |-- direct minor
        `-- package.json (3.0.0 -> 3.1.0)
```

Releases between members of the same group aren't listed, since the group already explains them. Each member keeps its own tag, and [unversioned](#unversioned-packages) members stay unversioned until forced by name, so a pattern can't start releasing a package by accident.

### Fixed versions

Pass `--fixed-version`, set `FIXED_VERSION=true` or set `fixed-version = true` in [`bumper.toml`](#configuration-file) to give every package in the repository the same version. This puts every package in one [version group](#version-groups), in place of any configured groups. When any package has a direct or forced impact, every discovered package releases at the highest of those impacts, applied to the highest current version among all packages so no package moves backwards. Each package still gets its own tag (`v3.1.0`, `packages/app/v3.1.0`, ...), which keeps Go submodules resolvable and lets you turn the mode off later.

When neither the flag, `FIXED_VERSION` nor `fixed-version` is set, no groups are configured, and every package (two or more) currently has the same version, an interactive terminal run asks whether to release them with a fixed version, but only when the answer would change the release plan (or when `-i` lets you choose packages afterwards). Pass `--fixed-version` or `--no-fixed-version`, or set `FIXED_VERSION` to `true` or `false`, to skip the question. Non-interactive runs never ask and version packages independently.

Packages keep their own release-history boundaries, so commits are never counted twice when switching to fixed versions. The first fixed release aligns packages whose versions differ; the release plan shows each package's old version.

```text
Release plan:
. (1.0.0 -> 3.1.0, minor)
|-- fixed version (minor from packages/library)
|-- package.json (1.0.0 -> 3.1.0)
`-- packages
    |-- app (2.0.0 -> 3.1.0, minor)
    |   |-- fixed version (minor from packages/library)
    |   `-- package.json (2.0.0 -> 3.1.0; library 3.0.0 -> 3.1.0)
    `-- library (3.0.0 -> 3.1.0, minor)
        |-- direct minor
        `-- package.json (3.0.0 -> 3.1.0)
```

## Why

Most of the popular actions are antagonistic about making _any_ changes to the source code during version bumps. Unfortunately for me, two of the technologies I use quite heavily (nix & npm) use version numbers in source, and I would rather deal with the occasional rebase than have version numbers out of sync. Of those that support bumping versions in source, I didn't find any I liked that also supported bumping for arbitrary files. I've found it quite common to have a version that needs to be updated in a readme, or a hardcoded version in the source code.

## Install

### Action

```yaml
- name: Bump
  uses: spotdemo4/bumper@v1.0.0
  with:
    commit: true # commit changes after bumping, default true
    push: true # push changes after bumping, default true
    force: false # force a bump for every package, default false

    # repository-relative package directories to force a bump for
    force_packages: |-
      packages/app
    bump_type: minor # patch, minor or major; overrides the bump type implied by commits
    force_bump_type: patch # patch, minor or major; default patch
    fixed_version: false # give every package the same version, default false

    # additional files or directories to bump versions in
    files: |-
      action.yaml
      README.md

    # repository-relative directories to ignore
    ignore_directories: |-
      generated
      packages/legacy

    # conventional commit types for MAJOR version bumps, default none
    major_types: |-
      major

    # conventional commit types for MINOR version bumps, default "feat"
    minor_types: |-
      feat

    # conventional commit types for PATCH version bumps, default "fix"
    patch_types: |-
      fix

    # conventional commit scopes to skip over, default "ci", "docs"
    skip_scopes: |-
      ci
      docs

    # bump the major version for a "!" after the commit type or a BREAKING CHANGE footer, default true
    breaking_changes: true
```

### Nix

```sh
nix run github:spotdemo4/bumper
```

#### Flake

```nix
inputs = {
    bumper = {
        url = "github:spotdemo4/bumper";
    };
};

outputs = { bumper, ... }: {
    devShells.x86_64-linux.default = pkgs.mkShell {
        packages = [ bumper.packages.x86_64-linux.default ];
    };
}
```

also available from the [nix user repository](https://nur.nix-community.org/repos/trev/) as `nur.repos.trev.bumper`

### Docker

```sh
docker run -it \
  -w /app \
  -v "$(pwd):/app" \
  -v "$HOME/.ssh:/root/.ssh" \
  ghcr.io/spotdemo4/bumper:1.0.0
```

### Downloads

https://trev.zip/llc/bumper/releases

## Contributing

see [CONTRIBUTING.md](CONTRIBUTING.md) for requirements and getting started

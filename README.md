# git version bumper

[![check](https://trev.zip/llc/bumper/actions/workflows/check.yaml/badge.svg?branch=main&logo=forgejo&logoColor=%23bac2de&label=check&labelColor=%23313244)](https://trev.zip/llc/bumper/actions?workflow=check.yaml)
[![vulnerable](https://trev.zip/llc/bumper/actions/workflows/vulnerable.yaml/badge.svg?branch=main&logo=forgejo&logoColor=%23bac2de&label=vulnerable&labelColor=%23313244)](https://trev.zip/llc/bumper/actions?workflow=vulnerable.yaml)
[![rust](https://img.shields.io/badge/dynamic/toml?url=https://trev.zip/llc/bumper/raw/branch/main/Cargo.toml&query=%24.package.rust-version&logo=rust&logoColor=%23bac2de&label=version&labelColor=%23313244&color=%23D34516)](https://releases.rs/)
[![flakehub](https://img.shields.io/endpoint?url=https://flakehub.com/f/spotdemo4/bumper/badge&labelColor=%23313244)](https://flakehub.com/flake/spotdemo4/bumper)

- determines the [semantic versioning](https://semver.org/) impact (major, minor or patch) of the [conventional commits](https://www.conventionalcommits.org) since each package's last git tag
- increments the git tag by the impact (v0.0.1 -> PATCH -> v0.0.2)
- creates hierarchical tags for packages in subdirectories (`packages/consumer/v0.0.2`)
- releases containing packages with a patch whenever one of their nested packages releases
- applies the version bump to files given as arguments (`bumper [files...]`)
- applies the version bump in directories given as arguments to supported project files (`README.md`, `action.yaml`, `action.yml`, `package.json`, `package-lock.json`, `build.gradle`, `build.gradle.kts`, `gradle.properties`, `Cargo.toml`, `Cargo.lock`, `pyproject.toml`, `uv.lock`, `build.zig.zon`, `gleam.toml`, `*.nix` (`version = "x.y.z";`), `CMakeLists.txt`)
- skips configured directories, likely vendored paths (`vendor`, `node_modules`), and symlinks during directory scans
- commits the bumped files and pushes them with the new git tag

This works well as a github action. Have it run on every push to main and it will bump the version for every change, or run it on a schedule to increase the version if there were any new changes.

## Usage

```elm
bumper [paths...]
```

Use `--ignore-directories generated,packages/legacy` or set `IGNORE_DIRECTORIES` to a whitespace- or newline-separated list of repository-relative directories. Ignored trees do not contribute commits, packages, version files, or dependency updates. Explicitly selected paths inside them are skipped.

### Package tags

Each supplied file or directory belongs to its nearest containing package. A package is a directory with a valid versioned `package.json`, `Cargo.toml`, `pyproject.toml`, `gleam.toml`, `build.zig.zon`, `CMakeLists.txt`, `build.gradle`, or `build.gradle.kts`. The repository root is always the root package. Documentation, lockfiles, action files, generic Nix files, and grouping directories do not create package boundaries.

Root releases use `vX.Y.Z`. Packages below the repository root use their repository-relative path as the tag prefix:

```text
v3.0.1
packages/consumer/v1.2.3
packages/consumer/plugins/cache/v0.5.0
```

For example, `bumper packages/consumer/README.md` assigns the README to `packages/consumer`; it does not create a README-specific tag. If the consumer releases, each real package containing it also releases. Descendant propagation is a patch unless the containing package has its own minor or major conventional commit. Multiple supplied paths may release sibling packages, while shared ancestors release only once.

Every independent package stream needs an initial tag before bumper can calculate its first release. For a package currently at `1.2.2`, create `packages/consumer/v1.2.2`. Existing root repositories continue to use their current `vX.Y.Z` tags.

## Why

Most of the popular actions are antagonistic about making _any_ changes to the source code during version bumps. Unfortunately for me, two of the technologies I use quite heavily (nix & npm) use version numbers in source, and I would rather deal with the occasional rebase than have version numbers out of sync. Of those that support bumping versions in source, I didn't find any I liked that also supported bumping for arbitrary files. I've found it quite common to have a version that needs to be updated in a readme, or a hardcoded version in the source code.

## Install

### Action

```yaml
- name: Bump
  uses: spotdemo4/bumper@v0.16.3
  with:
    commit: true # commit changes after bumping, default true
    push: true # push changes after bumping, default true
    force: false # force at least a PATCH version bump, default false

    # list of files to bump versions in
    files: |-
      action.yaml
      README.md

    # repository-relative directories to ignore
    ignore_directories: |-
      generated
      packages/legacy

    # conventional commit types for MAJOR version bumps, default "BREAKING CHANGE"
    major_types: |-
      BREAKING CHANGE

    # conventional commit types for MINOR version bumps, default "feat"
    minor_types: |-
      feat

    # conventional commit types for PATCH version bumps, default "fix"
    patch_types: |-
      fix

    # conventional commit scopes to skip over, default "ci"
    skip_scopes: |-
      ci
```

### Nix

```elm
nix run github:spotdemo4/bumper
```

#### Flake

```nix
inputs = {
    bumper = {
        url = "github:spotdemo4/bumper";
        inputs.nixpkgs.follows = "nixpkgs";
    };
};

outputs = { bumper, ... }: {
    devShells.x86_64-linux.default = pkgs.mkShell {
        packages = [
            bumper.x86_64-linux.default
        ];
    };
}
```

also available from the [nix user repository](https://nur.nix-community.org/repos/trev/) as `nur.repos.trev.bumper`

### Docker

```elm
docker run -it \
  -w /app \
  -v "$(pwd):/app" \
  -v "$HOME/.ssh:/root/.ssh" \
  ghcr.io/spotdemo4/bumper:0.16.3
```

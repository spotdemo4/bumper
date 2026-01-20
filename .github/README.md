# git version bumper

![check](https://github.com/spotdemo4/bumper/actions/workflows/check.yaml/badge.svg?branch=main)
![vulnerable](https://github.com/spotdemo4/bumper/actions/workflows/vulnerable.yaml/badge.svg?branch=main)

A simple shell script that:

- determines the [semantic versioning](https://semver.org/) impact (major, minor or patch) of the [conventional commits](https://www.conventionalcommits.org) since the last git tag
- increments the git tag by the impact (v0.0.1 -> PATCH -> v0.0.2)
- applies the version bump to files given as arguments (`bumper [files...]`)
- applies the version bump in directories given as arguemnts to select files (`package.json`, `Cargo.toml`, `flake.nix`)
- commits the bumped files and pushes them with the new git tag

This works well as a github action. Have it run on every push to main and it will bump the version for every change, or run it on a schedule to increase the version if there were any new changes.

## Usage

```elm
bumper [paths...]
```

## Why

Most of the popular actions are antagonistic about making _any_ changes to the source code during version bumps. Unfortunately for me, two of the technologies I use quite heavily (nix & npm) use version numbers in source, and I would rather deal with the occasional rebase than have version numbers out of sync. Of those that support bumping versions in source, I didn't find any I liked that also supported bumping for arbitrary files. I've found it quite common to have a version that needs to be updated in a readme, or a hardcoded version in the source code.

## Install

### Action

```yaml
- name: Bump
  uses: spotdemo4/bumper@v0.8.3
  with:
    commit: true # commit changes after bumping, default true
    push: true # push changes after bumping, default true
    force: false # force at least a PATCH version bump, default false

    # list of files to bump versions in
    files: |-
      action.yaml
      README.md

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
    devShells."${system}".default = pkgs.mkShell {
        packages = [
            bumper."${system}".default
        ];
    };
}
```

also available from the [nix user repository](https://nur.nix-community.org/repos/trev/) as `nur.repos.trev.bumper`

### Docker

```elm
docker run -it --rm \
  -w /app \
  -v "$(pwd):/app" \
  -v "$HOME/.ssh:/root/.ssh" \
  ghcr.io/spotdemo4/bumper:0.8.3
```

### Downloads

#### [bumper.sh](/src/bumper.sh) - bash script

requires [jq](https://jqlang.org/), [cargo-edit](https://github.com/killercup/cargo-edit) (rust), [nix-update](https://github.com/Mic92/nix-update) (nix), [nodejs](https://nodejs.org/) (node)

```elm
git clone https://github.com/spotdemo4/bumper &&
./bumper/src/bumper.sh
```

#### [bumper-0.8.3.tar.xz](https://github.com/spotdemo4/nix-scan/releases/latest/download/bumper-0.8.3.tar.xz) - bundle

contains all dependencies, only use if necessary

```elm
wget https://github.com/spotdemo4/nix-scan/releases/latest/download/bumper-0.8.3.tar.xz &&
tar xf bumper-0.8.3.tar.xz &&
./release
```

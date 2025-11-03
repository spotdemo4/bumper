# git version bumper

![check](https://github.com/spotdemo4/bumper/actions/workflows/check.yaml/badge.svg)
![vulnerable](https://github.com/spotdemo4/bumper/actions/workflows/vulnerable.yaml/badge.svg)

a simple shell script that

- determines the [semantic versioning](https://semver.org/) impact (major, minor or patch) of the [conventional commits](https://www.conventionalcommits.org) since the last git tag
- increments the git tag by the impact (v0.0.1 -> PATCH -> v0.0.2)
- applies the version bump to discovered files (`package.json`, `flake.nix`)
- applies the version bump to files given as arguments (`bumper [files...]`)
- commits the bumped files and pushes them with the new git tag

this works well as a github action. have it run on every push to main and it will bump the version for every change, or run it on a schedule to increase the version if there were any new changes

## usage

```console
$ bumper action.yaml
impact: patch
0.0.1 -> 0.0.2
changed: action.yaml

committing: v0.0.1 -> v0.0.2
creating tag: v0.0.2
pushing changes to origin main
```

## why

why create this when there are a million other actions that do something similar? well, most of the popular actions are antagonistic about making _any_ changes to the source code during version bumps. unfortunately for me, two of the technologies I use quite heavily (nix & npm) use version numbers in source, and I would rather deal with the occasional rebase than have version numbers out of sync. of those that support bumping versions in source, I didn't find any I liked that also supported bumping for arbitrary files. I've found it quite common to have a version that needs to be updated in a readme, or a hardcoded version in the source code. If you know of an action that does what this does but better, let me know!

## install

### github actions

```yaml
- name: Bump
  uses: spotdemo4/bumper@v0.1.20
  with:
    commit: true # optional, default true
    push: true # optional, default true

    # optional, files to manually update
    files: |
      action.yaml
      README.md

    # optional, default "BREAKING CHANGE"
    major_types: |
      BREAKING CHANGE

    # optional, default "feat"
    minor_types: |
      feat

    # optional, default "fix"
    patch_types: |
      fix

    # optional, commit scopes to skip
    skip_scopes: |
      nix
```

### script

[`bumper.sh`](https://raw.githubusercontent.com/spotdemo4/bumper/refs/heads/main/bumper.sh)

### nix

```console
$ nix run github:spotdemo4/bumper
impact: patch
0.0.1 -> 0.0.2

committing: v0.0.1 -> v0.0.2
creating tag: v0.0.2
pushing changes to origin main
```

#### flake

```nix
inputs = {
    bumper = {
        url = "github:spotdemo4/bumper";
        inputs.nixpkgs.follows = "nixpkgs";
    };
};

outputs = { bumper, ... }: {
    devShells."${system}".default = pkgs.mkShell {
        packages = with pkgs; [
            bumper."${system}".default
        ];
    };
}
```

### binary

[release](https://github.com/spotdemo4/bumper/releases/latest)

### container

```console
$ docker pull ghcr.io/spotdemo4/bumper:0.1.20
```

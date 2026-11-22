---
name: fix-vulnerable
description: Fixes failures reported by the vulnerable workflow, which runs cargo audit, flake-checker and zizmor. Use when the vulnerable workflow fails, or when asked about a RUSTSEC advisory, an outdated nixpkgs input or a zizmor finding.
---

# Fix Vulnerable

The `vulnerable` workflow runs weekly in each forge directory (`.github`, `.forgejo`), with a job for each tool. Read the failed job's log to find which tool failed and why, then reproduce it locally in the `vulnerable` dev shell:

```sh
nix develop .#vulnerable -c cargo-audit audit
nix develop .#vulnerable -c flake-checker -f
nix develop .#vulnerable -c zizmor .github/workflows
```

## cargo audit

Renovate opens automerging fix PRs for vulnerable direct dependencies, so check for an open Renovate PR first. A failure without one is usually a transitive dependency.

1. Update the vulnerable crate within its current requirements with `cargo update -p <crate>`, adding `--precise <version>` to pick the patched version.
2. If a dependent's version requirement rules out the patched version, update the direct dependency that pulls it in, in `Cargo.toml`. Find it with `cargo tree -i <crate>`.
3. Only when no patched version exists, or the vulnerable code is unreachable, ignore the advisory in `.cargo/audit.toml` with a comment explaining why, and tell the user:

   ```toml
   [advisories]
   ignore = [
     "RUSTSEC-0000-0000", # reason
   ]
   ```

Warnings for unmaintained, unsound or yanked crates don't fail the job.

## flake-checker

flake-checker fails when a nixpkgs input is outdated (over 30 days old), not owned by `NixOS`, or tracks an unsupported branch. Renovate only updates Nix inputs during its weekly lock file maintenance, which automerges.

1. Check for an open Renovate lock file maintenance PR. If one exists, its checks are probably failing, so fix the failure on that PR's branch instead of opening a competing one.
2. Otherwise run `nix flake update`, then fix anything the update breaks.
3. For an owner or branch failure, change the input's URL in `flake.nix` to a supported `NixOS/nixpkgs` branch.

## zizmor

zizmor audits the workflows for security issues. CI runs its online audits with a GitHub token. Locally, set `GH_TOKEN` to enable them, for example with `GH_TOKEN=$(gh auth token)`, or they're skipped.

1. Fix the finding in every forge directory that has the workflow, not only the one that failed. The forges' workflows differ: GitHub sets `permissions`, while Forgejo doesn't support them and uses `enable-openid-connect`. Adapt the fix to each forge rather than copying it.
2. Pinned action updates (`unpinned-uses`, known vulnerable versions) are Renovate's job, so check for an open Renovate PR first.
3. Only when a finding doesn't apply to a forge, ignore it with an inline `# zizmor: ignore[<audit>]` comment on the flagged line, as the Forgejo workflows do for `excessive-permissions`.

## Finish

Run `nix flake check`, and confirm each tool that failed now passes locally.

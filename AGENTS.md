# AGENTS.md

## Task Completion Requirements

- `nix flake check` must pass before considering tasks completed.
- NEVER run `cargo test`. Always use `nix flake check`.

## Project Snapshot

Bumper is a minimal CLI for version bumps.

This repository is a VERY EARLY WIP. Proposing sweeping changes that improve long-term maintainability is encouraged.

## Core Priorities

1. Performance first.
2. Reliability first.

If a tradeoff is required, choose correctness and robustness over short-term convenience.

## Maintainability

Long term maintainability is a core priority. If you add new functionality, first check if there is shared logic that can be extracted to a separate module. Duplicate logic across multiple files is a code smell and should be avoided. Don't be afraid to change existing code. Don't take shortcuts by just adding local logic to solve a problem.

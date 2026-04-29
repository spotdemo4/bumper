# AGENTS.md

## Task Completion Requirements

- `nix flake check` must pass before considering tasks completed.
- NEVER run `cargo test`. Always use `nix flake check`.

- `treefmt` must be used for formatting.
- NEVER run `cargo fmt`. NEVER run `rustfmt`. Always use `treefmt`.

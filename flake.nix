{
  description = "rust template";

  nixConfig = {
    extra-substituters = [
      "https://nix.trev.zip"
      "https://nix-community.cachix.org"
    ];
    extra-trusted-public-keys = [
      "trev:I39N/EsnHkvfmsbx8RUW+ia5dOzojTQNCTzKYij1chU="
      "nix-community.cachix.org-1:mB9FSh9qf2dCimDSUo8Zy7bkq5CX+/rkCWyvRCYg3Fs="
    ];
  };

  inputs = {
    systems.url = "github:nix-systems/default";
    nixpkgs.url = "github:nixos/nixpkgs/nixpkgs-unstable";
    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    trev = {
      url = "github:spotdemo4/nur";
      inputs.systems.follows = "systems";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      fenix,
      trev,
      ...
    }:
    trev.libs.mkFlake (
      system: init:
      let
        pkgs = init.appendOverlays [ fenix.overlays.default ];
        rustToolchain = pkgs.fenix.fromToolchainFile {
          file = ./rust-toolchain.toml;
          sha256 = "sha256-qqF33vNuAdU5vua96VKVIwuc43j4EFeEXbjQ6+l4mO4=";
        };
      in
      {
        devShells = {
          default = pkgs.mkShell {
            shellHook = pkgs.shellhook.ref;
            packages = with pkgs; [
              # rust
              rustToolchain
              openssl
              pkg-config

              # formatters
              nixfmt
              prettier
              tombi

              # util
              bumper
              flake-release
            ];
          };

          bump = pkgs.mkShell {
            packages = with pkgs; [
              bumper
              rustToolchain
              openssl
              pkg-config
            ];
          };

          release = pkgs.mkShell {
            packages = with pkgs; [
              flake-release
              rustToolchain
            ];
          };

          update = pkgs.mkShell {
            packages = with pkgs; [
              renovate
              cargo # rust
            ];
          };

          vulnerable = pkgs.mkShell {
            packages = with pkgs; [
              cargo-audit # rust
              flake-checker # flake
              octoscan # actions
            ];
          };
        };

        checks = pkgs.mkChecks {
          rust = {
            src = self.packages.${system}.default;
            packages = [
              rustToolchain
            ];
            script = ''
              cargo fmt --check
              cargo test --offline
              cargo clippy --offline -- -D warnings
            '';
          };

          nix = {
            root = ./.;
            filter = file: file.hasExt "nix";
            packages = with pkgs; [
              nixfmt
            ];
            forEach = ''
              nixfmt --check "$file"
            '';
          };

          renovate = {
            root = ./.github;
            fileset = ./.github/renovate.json;
            packages = with pkgs; [
              renovate
            ];
            script = ''
              renovate-config-validator renovate.json
            '';
          };

          actions = {
            root = ./.;
            fileset = pkgs.lib.fileset.unions [
              ./action.yaml
              ./.github/workflows
            ];
            packages = with pkgs; [
              action-validator
              octoscan
            ];
            forEach = ''
              action-validator "$file"
              octoscan scan "$file"
            '';
          };

          tombi = {
            root = ./.;
            filter = file: file.hasExt "toml";
            packages = with pkgs; [
              tombi
            ];
            forEach = ''
              tombi format --offline --check "$file"
              tombi lint --offline --error-on-warnings "$file"
            '';
          };

          prettier = {
            root = ./.;
            filter = file: file.hasExt "yaml" || file.hasExt "json" || file.hasExt "md";
            packages = with pkgs; [
              prettier
            ];
            forEach = ''
              prettier --check "$file"
            '';
          };
        };

        apps = pkgs.mkApps {
          dev = "cargo run";
        };

        packages = pkgs.mkPackages pkgs (
          pkgs:
          let
            rustPlatform = pkgs.makeRustPlatform {
              cargo = rustToolchain;
              rustc = rustToolchain;
            };
          in
          {
            default = rustPlatform.buildRustPackage (finalAttrs: {
              pname = "bumper";
              version = "0.11.2";

              src = pkgs.lib.fileset.toSource {
                root = ./.;
                fileset = pkgs.lib.fileset.unions [
                  ./Cargo.lock
                  ./Cargo.toml
                  ./rust-toolchain.toml
                  ./src
                  ./tests
                ];
              };

              cargoLock.lockFile = ./Cargo.lock;

              nativeBuildInputs = with pkgs; [
                autoPatchelfHook
                pkg-config
              ];

              buildInputs = with pkgs; [
                libgcc
                openssl
              ];

              doCheck = false;

              meta = {
                description = "Git semantic version bumper";
                mainProgram = "bumper";
                license = pkgs.lib.licenses.mit;
                platforms = pkgs.lib.platforms.all;
                homepage = "https://github.com/spotdemo4/bumper";
                changelog = "https://github.com/spotdemo4/bumper/releases/tag/v${finalAttrs.version}";
                downloadPage = "https://github.com/spotdemo4/bumper/releases/tag/v${finalAttrs.version}";
              };
            });
          }
        );

        images = pkgs.mkImages pkgs (pkgs: {
          default = pkgs.mkImage self.packages.${system}.default {
            contents = with pkgs; [ dockerTools.caCertificates ];
          };
        });

        formatter = pkgs.nixfmt-tree;
        schemas = trev.schemas;
      }
    );
}

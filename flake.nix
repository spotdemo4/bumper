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
    systems.url = "github:spotdemo4/systems";
    nixpkgs.url = "github:nixos/nixpkgs/nixpkgs-unstable";
    trev = {
      url = "github:spotdemo4/nur";
      inputs.systems.follows = "systems";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      trev,
      ...
    }:
    trev.libs.mkFlake (
      system: pkgs: {
        devShells = {
          default = pkgs.mkShell {
            shellHook = pkgs.shellhook.ref;
            RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
            packages = with pkgs; [
              # rust
              rustc
              cargo

              # deps
              openssl
              pkg-config

              # linters
              clippy
              tombi

              # formatters
              rustfmt
              nixfmt
              prettier

              # util
              bumper
            ];
          };

          bump = pkgs.mkShell {
            packages = with pkgs; [
              bumper
            ];
          };

          release = pkgs.mkShell {
            packages = with pkgs; [
              flake-release
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
            packages = with pkgs; [
              rustfmt
              clippy
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
            files = ./.github/renovate.json;
            packages = with pkgs; [
              renovate
            ];
            script = ''
              renovate-config-validator renovate.json
            '';
          };

          actions = {
            root = ./.;
            files = [
              ./action.yaml
              ./.github/workflows
            ];
            filter = file: file.hasExt "yaml";
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

        formatter = pkgs.treefmt.withConfig {
          configFile = ./treefmt.toml;
          runtimeInputs = with pkgs; [
            rustfmt
            nixfmt
            tombi
            prettier
          ];
        };

        apps = pkgs.mkApps {
          dev = "cargo run";
        };

        packages.default = pkgs.rustPlatform.buildRustPackage (
          final: with pkgs.lib; {
            pname = "bumper";
            version = "0.15.0";

            src = fileset.toSource {
              root = ./.;
              fileset = fileset.unions [
                ./Cargo.lock
                ./Cargo.toml
                ./src
                ./tests
              ];
            };
            cargoLock.lockFile = ./Cargo.lock;

            nativeBuildInputs = [
              pkgs.pkg-config
            ]
            ++ optional (
              !pkgs.stdenv.hostPlatform.isStatic && pkgs.stdenv.hostPlatform.isLinux
            ) pkgs.autoPatchelfHook;

            buildInputs = with pkgs; [
              libgcc
              openssl
            ];

            meta = {
              description = "Git semantic version bumper";
              mainProgram = "bumper";
              license = licenses.mit;
              platforms = platforms.all;
              homepage = "https://github.com/spotdemo4/bumper";
              changelog = "https://github.com/spotdemo4/bumper/releases/tag/v${final.version}";
              downloadPage = "https://github.com/spotdemo4/bumper/releases/tag/v${final.version}";
            };
          }
        );

        images.default = pkgs.mkImage {
          src = self.packages.${system}.default;
          contents = with pkgs; [ dockerTools.caCertificates ];
        };

        schemas = trev.schemas;
      }
    );
}

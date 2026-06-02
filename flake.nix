{
  description = "git semantic version bumper";

  nixConfig = {
    extra-substituters = [
      "https://nix.trev.zip"
    ];
    extra-trusted-public-keys = [
      "trev:I39N/EsnHkvfmsbx8RUW+ia5dOzojTQNCTzKYij1chU="
    ];
  };

  inputs = {
    systems.url = "github:spotdemo4/systems";
    nixpkgs.url = "github:nixos/nixpkgs/nixpkgs-unstable";
    trevpkgs = {
      url = "github:spotdemo4/trevpkgs";
      inputs.systems.follows = "systems";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      trevpkgs,
      ...
    }:
    trevpkgs.libs.mkFlake (
      system: pkgs: {

        # nix develop [#...]
        devShells = {
          default = pkgs.mkShell {
            RUST_SRC_PATH = pkgs.rustPlatform.rustLibSrc;
            shellHook = pkgs.shellhook.ref;
            packages = with pkgs; [
              # rust
              rustc
              cargo

              # deps
              openssl
              pkg-config

              # lint
              clippy
              cargo-audit
              nixd
              nil

              # format
              rustfmt
              nixfmt
              oxfmt
              treefmt

              # util
              bumper
              fix-hash
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
              rustc
              cargo
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
              flake-checker # nix
              zizmor # actions
            ];
          };
        };

        # nix run [#...]
        apps = pkgs.mkApps {
          dev = "cargo run";
          test = "cargo test";
        };

        # nix build [#...]
        packages = {
          default = pkgs.rustPlatform.buildRustPackage (
            final: with pkgs.lib; {
              pname = "bumper";
              version = "0.17.0";

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

              nativeBuildInputs =
                with pkgs;
                [
                  pkg-config
                ]
                ++ optional (!stdenv.hostPlatform.isStatic && stdenv.hostPlatform.isLinux) autoPatchelfHook;

              buildInputs = with pkgs; [
                libgcc
                openssl
              ];

              meta = {
                mainProgram = "bumper";
                description = "Git semantic version bumper";
                license = licenses.mit;
                platforms = platforms.all;
                homepage = "https://github.com/spotdemo4/bumper";
                changelog = "https://github.com/spotdemo4/bumper/releases/tag/v${final.version}";
                downloadPage = "https://github.com/spotdemo4/bumper/releases/tag/v${final.version}";
              };
            }
          );
        };

        # nix build #images.[...]
        images = {
          default = pkgs.mkImage {
            src = self.packages.${system}.default;
            contents = with pkgs; [ dockerTools.caCertificates ];
          };
        };

        # nix fmt
        formatter = pkgs.treefmt.withConfig {
          configFile = ./treefmt.toml;
          runtimeInputs = with pkgs; [
            rustfmt
            nixfmt
            oxfmt
          ];
        };

        # nix flake check
        checks = pkgs.mkChecks {
          nix = {
            root = ./.;
            filter = file: file.hasExt "nix";
            packages = with pkgs; [
              nixfmt
            ];
            script = ''
              nixfmt --check "$file"
            '';
          };

          actions-gh = {
            root = ./.github/workflows;
            filter = file: file.hasExt "yaml";
            packages = with pkgs; [
              action-validator
              zizmor
            ];
            forEach = ''
              action-validator "$file"
              zizmor --offline "$file"
            '';
          };

          actions-fj = {
            root = ./.forgejo/workflows;
            filter = file: file.hasExt "yaml";
            packages = with pkgs; [
              zizmor
            ];
            forEach = ''
              zizmor --offline "$file"
            '';
          };

          renovate = {
            root = ./.forgejo;
            files = ./.forgejo/renovate.json;
            packages = with pkgs; [
              renovate
            ];
            script = ''
              renovate-config-validator renovate.json
            '';
          };

          rust = {
            src = self.packages.${system}.default;
            packages = with pkgs; [
              clippy
            ];
            script = ''
              cargo clippy --offline -- -D warnings
            '';
          };

          config = {
            root = ./.;
            filter = file: file.hasExt "json" || file.hasExt "yaml" || file.hasExt "toml" || file.hasExt "md";
            packages = with pkgs; [
              oxfmt
            ];
            script = ''
              oxfmt --check
            '';
          };
        };
      }
    );
}

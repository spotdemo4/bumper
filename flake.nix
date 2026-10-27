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
            shellHook = pkgs.shellhook.ref;
            env.RUST_SRC_PATH = pkgs.rustPlatform.rustLibSrc;
            packages = with pkgs; [
              # rust
              rustc
              cargo
              rust-analyzer
              rustfmt
              clippy
              cargo-audit

              # deps
              openssl
              pkg-config

              vscode-json-languageserver # json
              yaml-language-server # yaml
              tombi # toml
              oxfmt # format

              # nix
              nixd
              nixfmt

              # util
              treefmt
              bumper
              fix-hash
            ];
          };

          bump = pkgs.mkShell {
            packages = [
              self.packages.${system}.default
            ];
          };

          release = pkgs.mkShell {
            packages = with pkgs; [
              flake-release

              # cargo publish
              rustc
              cargo
              openssl
              pkg-config
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

        # nix build [#...]
        packages = {
          default = pkgs.mkRustPackage (
            final: with pkgs.lib; {
              pname = "bumper";
              version = "0.35.0";

              src = fileset.toSource {
                root = ./.;
                fileset = fileset.unions [
                  ./Cargo.lock
                  ./Cargo.toml
                  ./clippy.toml
                  ./LICENSE
                  ./README.md
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

              checkPhase = ''
                runHook preCheck
                cargo test --offline
                runHook postCheck
              '';

              # also check the dependencies for clippy, the lints run in the rust-lint flake check
              cargoArtifactsArgs = {
                nativeCheckInputs = with pkgs; [ clippy ];
                checkPhase = ''
                  runHook preCheck
                  cargo test --offline
                  cargo clippy --offline --all-targets
                  runHook postCheck
                '';
              };

              installCheckPhase = ''
                runHook preInstallCheck
                test "$("$out/bin/bumper" --version)" = "bumper ${final.version}"
                runHook postInstallCheck
              '';

              meta = {
                mainProgram = "bumper";
                description = "Git semantic version bumper";
                license = licenses.mit;
                platforms = platforms.all;
                homepage = "https://trev.zip/llc/bumper";
                changelog = "https://trev.zip/llc/bumper/releases";
                downloadPage = "https://trev.zip/llc/bumper/releases/tag/v${final.version}";
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
          inherit (self.packages.${system}) default;

          rust-lint = {
            src = self.packages.${system}.default;
            packages = with pkgs; [
              rustfmt
              clippy
            ];
            script = ''
              cargo fmt --check
              cargo clippy --offline --all-targets -- -D warnings
            '';
          };

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
            script = ''
              action-validator "$file"
              zizmor --offline "$file"
            '';
          };

          actions-fj = {
            root = ./.forgejo/workflows;
            filter = file: file.hasExt "yaml";
            packages = with pkgs; [
              forgejo-runner
              zizmor
            ];
            script = ''
              forgejo-runner validate --workflow --path "$file"
              zizmor --offline "$file"
            '';
          };

          renovate-fj = {
            root = ./.forgejo;
            files = ./.forgejo/renovate.json;
            packages = with pkgs; [
              renovate
            ];
            script = ''
              renovate-config-validator renovate.json
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

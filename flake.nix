{
  description = "bumper";

  nixConfig = {
    extra-substituters = [
      "https://nix.trev.zip"
    ];
    extra-trusted-public-keys = [
      "trev:I39N/EsnHkvfmsbx8RUW+ia5dOzojTQNCTzKYij1chU="
    ];
  };

  inputs = {
    systems.url = "github:nix-systems/default";
    nixpkgs.url = "github:nixos/nixpkgs/nixpkgs-unstable";
    trev = {
      url = "github:spotdemo4/nur";
      inputs.systems.follows = "systems";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      nixpkgs,
      trev,
      ...
    }:
    trev.libs.mkFlake (
      system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [
            trev.overlays.packages
            trev.overlays.libs
            trev.overlays.images
          ];
        };
        fs = pkgs.lib.fileset;
        deps = with pkgs; [
          jq
          ncurses
          gnused

          # rust
          cargo
          cargo-edit

          # nix
          nix-update

          # node
          nodejs_latest

          # python
          python3
          uv
        ];
      in
      {
        devShells = {
          default = pkgs.mkShell {
            shellHook = pkgs.shellhook.ref;
            packages =
              with pkgs;
              [
                # lint
                shellcheck

                # format
                nixfmt
                prettier

                # util
                bumper
                flake-release
                renovate
              ]
              ++ deps;
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
            ];
          };

          vulnerable = pkgs.mkShell {
            packages = with pkgs; [
              # nix
              flake-checker

              # actions
              octoscan
            ];
          };
        };

        checks = pkgs.lib.mkChecks {
          shellcheck = {
            src = fs.toSource {
              root = ./.;
              fileset = fs.unions [
                (fs.fileFilter (file: file.hasExt "sh") ./.)
                ./.shellcheckrc
              ];
            };
            deps = with pkgs; [
              shellcheck
            ];
            script = ''
              shellcheck **/*.sh
            '';
          };

          actions = {
            src = fs.toSource {
              root = ./.;
              fileset = fs.unions [
                ./action.yaml
                ./.github/workflows
              ];
            };
            deps = with pkgs; [
              action-validator
              octoscan
            ];
            script = ''
              action-validator **/*.yaml
              octoscan scan .
            '';
          };

          renovate = {
            src = fs.toSource {
              root = ./.github;
              fileset = ./.github/renovate.json;
            };
            deps = with pkgs; [
              renovate
            ];
            script = ''
              renovate-config-validator renovate.json
            '';
          };

          nix = {
            src = fs.toSource {
              root = ./.;
              fileset = fs.fileFilter (file: file.hasExt "nix") ./.;
            };
            deps = with pkgs; [
              nixfmt-tree
            ];
            script = ''
              treefmt --ci
            '';
          };

          prettier = {
            src = fs.toSource {
              root = ./.;
              fileset = fs.fileFilter (file: file.hasExt "yaml" || file.hasExt "json" || file.hasExt "md") ./.;
            };
            deps = with pkgs; [
              prettier
            ];
            script = ''
              prettier --check .
            '';
          };
        };

        apps = pkgs.lib.mkApps {
          dev.script = "./src/bumper.sh";
        };

        packages = with pkgs.lib; rec {
          default = pkgs.stdenv.mkDerivation (finalAttrs: {
            pname = "bumper";
            version = "0.10.2";

            src = fs.toSource {
              root = ./.;
              fileset = fs.unions [
                (fs.fileFilter (file: file.hasExt "sh") ./.)
                ./.shellcheckrc
              ];
            };

            nativeBuildInputs = with pkgs; [
              makeWrapper
              shellcheck
            ];

            runtimeInputs = deps;

            unpackPhase = ''
              cp -a "$src/." .
            '';

            dontBuild = true;

            configurePhase = ''
              chmod +w src
              sed -i '1c\#!${pkgs.runtimeShell}' src/bumper.sh
              sed -i '2c\export PATH="${makeBinPath finalAttrs.runtimeInputs}:$PATH"' src/bumper.sh
            '';

            doCheck = true;
            checkPhase = ''
              shellcheck src/*.sh
            '';

            installPhase = ''
              mkdir -p $out/lib/bumper
              cp -R src/*.sh $out/lib/bumper

              mkdir -p $out/bin
              makeWrapper "$out/lib/bumper/bumper.sh" "$out/bin/bumper"
            '';

            dontFixup = true;

            meta = {
              description = "Git semantic version bumper";
              mainProgram = "bumper";
              homepage = "https://github.com/spotdemo4/bumper";
              changelog = "https://github.com/spotdemo4/bumper/releases/tag/v${finalAttrs.version}";
              license = licenses.mit;
              platforms = platforms.all;
            };
          });

          image = pkgs.dockerTools.buildLayeredImage {
            name = default.pname;
            tag = default.version;

            fromImage = pkgs.image.nix;
            contents = with pkgs; [
              dockerTools.caCertificates
            ];

            created = "now";
            meta = default.meta;

            config = {
              Entrypoint = [ "${meta.getExe default}" ];
              Env = [ "DOCKER=true" ];
              Labels = {
                "org.opencontainers.image.title" = default.pname;
                "org.opencontainers.image.description" = default.meta.description;
                "org.opencontainers.image.source" = default.meta.homepage;
                "org.opencontainers.image.version" = default.version;
                "org.opencontainers.image.licenses" = default.meta.license.spdxId;
              };
            };
          };
        };

        formatter = pkgs.nixfmt-tree;
      }
    );
}

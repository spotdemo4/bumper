{
  description = "bumper";

  nixConfig = {
    extra-substituters = [
      "https://cache.trev.zip/nur"
    ];
    extra-trusted-public-keys = [
      "nur:70xGHUW1+1b8FqBchldaunN//pZNVo6FKuPL4U/n844="
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
          ];
        };
      in
      rec {
        devShells = {
          default = pkgs.mkShell {
            packages = with pkgs; [
              # bash
              gnused
              nix-update
              nodejs_latest

              # util
              bumper

              # lint
              nixfmt
              prettier
            ];
            shellHook = pkgs.shellhook.ref;
          };

          bump = pkgs.mkShell {
            packages = with pkgs; [
              nix-update
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
          bash = {
            src = packages.default;
            deps = with pkgs; [
              shellcheck
            ];
            script = ''
              shellcheck src/*.sh
            '';
          };

          action = {
            src = ./.;
            deps = with pkgs; [
              action-validator
            ];
            script = ''
              action-validator action.yaml
            '';
          };

          nix = {
            src = ./.;
            deps = with pkgs; [
              nixfmt-tree
            ];
            script = ''
              treefmt --ci
            '';
          };

          actions = {
            src = ./.;
            deps = with pkgs; [
              prettier
              action-validator
              octoscan
              renovate
            ];
            script = ''
              prettier --check "**/*.json" "**/*.yaml"
              action-validator .github/**/*.yaml
              octoscan scan .github
              renovate-config-validator .github/renovate.json
            '';
          };
        };

        apps = pkgs.lib.mkApps {
          dev.script = "./src/bumper.sh";
        };

        packages = {
          default = pkgs.stdenv.mkDerivation (finalAttrs: {
            pname = "bumper";
            version = "0.3.1";

            src = builtins.path {
              name = "root";
              path = ./.;
            };

            nativeBuildInputs = with pkgs; [
              shellcheck
            ];

            runtimeInputs = with pkgs; [
              gnused
              nix-update
              nodejs_latest
            ];

            unpackPhase = ''
              cp -a "$src/." .
            '';

            dontBuild = true;

            configurePhase = ''
              chmod +w src
              sed -i '1c\#!${pkgs.runtimeShell}' src/bumper.sh
              sed -i '2c\export PATH="${pkgs.lib.makeBinPath finalAttrs.runtimeInputs}:$PATH"' src/bumper.sh
            '';

            doCheck = true;
            checkPhase = ''
              shellcheck src/*.sh
            '';

            installPhase = ''
              mkdir -p $out/bin
              cp -R src/*.sh $out/bin
            '';

            dontFixup = true;

            meta = {
              description = "version bumper";
              mainProgram = "bumper.sh";
              homepage = "https://github.com/spotdemo4/bumper";
              platforms = pkgs.lib.platforms.all;
            };
          });

          image = pkgs.dockerTools.buildLayeredImage {
            fromImage = pkgs.dockerTools.pullImage {
              imageName = "nixos/nix";
              imageDigest = "sha256:0d9c872db1ca2f3eaa4a095baa57ed9b72c09d53a0905a4428813f61f0ea98db";
              hash = "sha256-H7uT+XPp5xadUzP2GEq031yZSIfzpZ1Ps6KVeBTIhOg=";
            };

            name = packages.default.pname;
            tag = packages.default.version;
            created = "now";
            meta = packages.default.meta;
            contents = with pkgs; [
              packages.default
              dockerTools.caCertificates
            ];

            config.Cmd = [
              "${pkgs.lib.meta.getExe packages.default}"
            ];
          };
        };

        formatter = pkgs.nixfmt-tree;
      }
    );
}

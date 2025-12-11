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
            trev.overlays.images
          ];
        };
      in
      rec {
        devShells = {
          default = pkgs.mkShell {
            packages = with pkgs; [
              jq
              sd

              # rust
              cargo
              cargo-edit

              # nix
              nix-update

              # node
              nodejs_latest

              # lint
              shellcheck # bash
              nixfmt # nix
              prettier # json/yaml
            ];
            shellHook = pkgs.shellhook.ref;
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
          build-image.script = ''
            nix build .#image
            docker load -i result
            docker run \
              --rm \
              -v "$PWD:/app" \
              -v "$HOME/.ssh:/root/.ssh" \
              -w /app \
              -e DEBUG=true \
              -e FORCE=true \
              -e COMMIT=false \
              "bumper:${packages.default.version}"
          '';
        };

        packages = {
          default = pkgs.stdenv.mkDerivation (finalAttrs: {
            pname = "bumper";
            version = "0.6.2";

            src = builtins.path {
              name = "root";
              path = ./.;
            };

            nativeBuildInputs = with pkgs; [
              makeWrapper
              shellcheck
            ];

            runtimeInputs = with pkgs; [
              jq
              sd

              # rust
              cargo
              cargo-edit

              # nix
              nix-update

              # node
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
              mkdir -p $out/lib/bumper
              cp -R src/*.sh $out/lib/bumper

              mkdir -p $out/bin
              makeWrapper "$out/lib/bumper/bumper.sh" "$out/bin/bumper"
            '';

            dontFixup = true;

            meta = {
              description = "git semantic version bumper";
              mainProgram = "bumper";
              homepage = "https://github.com/spotdemo4/bumper";
              changelog = "https://github.com/spotdemo4/bumper/releases/tag/v${finalAttrs.version}";
              license = pkgs.lib.licenses.mit;
              platforms = pkgs.lib.platforms.all;
            };
          });

          image = pkgs.dockerTools.buildLayeredImage {
            name = packages.default.pname;
            tag = packages.default.version;

            fromImage = pkgs.image.nix;
            contents = with pkgs; [
              packages.default
              dockerTools.caCertificates
            ];

            created = "now";
            meta = packages.default.meta;

            config = {
              Cmd = [ "${pkgs.lib.meta.getExe packages.default}" ];
              Env = [ "DOCKER=true" ];
            };
          };
        };

        formatter = pkgs.nixfmt-tree;
      }
    );
}

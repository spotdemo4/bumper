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
      self,
      trev,
      ...
    }:
    trev.libs.mkFlake (
      system: pkgs: {
        devShells = {
          default = pkgs.mkShell {
            shellHook = pkgs.shellhook.ref;
            packages = with pkgs; [
              # deps
              ncurses
              gnused
              jq
              nix-update

              # lint
              shellcheck

              # format
              nixfmt
              prettier

              # util
              bumper
              flake-release
              renovate
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
            ];
          };

          vulnerable = pkgs.mkShell {
            packages = with pkgs; [
              flake-checker # nix
              octoscan # actions
            ];
          };
        };

        checks = pkgs.mkChecks {
          shellcheck = {
            root = ./.;
            fileset = pkgs.lib.fileset.unions [
              ./.shellcheckrc
              (pkgs.lib.fileset.fileFilter (file: file.hasExt "sh") ./.)
            ];
            deps = with pkgs; [
              shellcheck
            ];
            forEach = ''
              shellcheck $file
            '';
          };

          actions = {
            root = ./.;
            fileset = pkgs.lib.fileset.unions [
              ./action.yaml
              ./.github/workflows
            ];
            deps = with pkgs; [
              action-validator
              octoscan
            ];
            forEach = ''
              action-validator $file
              octoscan scan $file
            '';
          };

          renovate = {
            root = ./.github;
            fileset = ./.github/renovate.json;
            deps = with pkgs; [
              renovate
            ];
            script = ''
              renovate-config-validator renovate.json
            '';
          };

          nix = {
            root = ./.;
            filter = file: file.hasExt "nix";
            deps = with pkgs; [
              nixfmt
            ];
            forEach = ''
              nixfmt --check $file
            '';
          };

          prettier = {
            root = ./.;
            filter = file: file.hasExt "yaml" || file.hasExt "json" || file.hasExt "md";
            deps = with pkgs; [
              prettier
            ];
            forEach = ''
              prettier --check $file
            '';
          };
        };

        apps = pkgs.mkApps {
          dev = "./src/bumper.sh";
        };

        packages.default = pkgs.stdenv.mkDerivation (finalAttrs: {
          pname = "bumper";
          version = "0.11.2";

          src = pkgs.lib.fileset.toSource {
            root = ./.;
            fileset = pkgs.lib.fileset.unions [
              (pkgs.lib.fileset.fileFilter (file: file.hasExt "sh") ./.)
              ./.shellcheckrc
            ];
          };

          nativeBuildInputs = with pkgs; [
            makeWrapper
            shellcheck
          ];

          runtimeInputs = with pkgs; [
            # deps
            ncurses
            gnused
            jq
            nix-update
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
            description = "Git semantic version bumper";
            mainProgram = "bumper";
            license = pkgs.lib.licenses.mit;
            platforms = pkgs.lib.platforms.all;
            homepage = "https://github.com/spotdemo4/bumper";
            changelog = "https://github.com/spotdemo4/bumper/releases/tag/v${finalAttrs.version}";
          };
        });

        images.default = pkgs.mkImage self.packages.${system}.default {
          contents = with pkgs; [ dockerTools.caCertificates ];
        };

        schemas = trev.schemas;
        formatter = pkgs.nixfmt-tree;
      }
    );
}

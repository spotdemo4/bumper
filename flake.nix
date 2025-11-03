{
  description = "bumper: git semantic version bumper";

  nixConfig = {
    extra-substituters = [
      "https://cache.trev.zip/nur"
    ];
    extra-trusted-public-keys = [
      "nur:70xGHUW1+1b8FqBchldaunN//pZNVo6FKuPL4U/n844="
    ];
  };

  inputs = {
    systems.url = "systems";
    nixpkgs.url = "github:nixos/nixpkgs/nixpkgs-unstable";
    utils = {
      url = "github:numtide/flake-utils";
      inputs.systems.follows = "systems";
    };
    nur = {
      url = "github:spotdemo4/nur";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = {
    nixpkgs,
    utils,
    nur,
    ...
  }:
    utils.lib.eachDefaultSystem (system: let
      pkgs = import nixpkgs {
        inherit system;
        overlays = [nur.overlays.default];
      };
    in {
      devShells = {
        default = pkgs.mkShell {
          packages = with pkgs; [
            nix-update
          ];
          shellHook = pkgs.trev.shellhook.ref;
        };

        ci = pkgs.mkShell {
          packages = with pkgs; [
            flake-checker
            trev.renovate
          ];
        };
      };

      checks = pkgs.trev.lib.mkChecks {
        shell = {
          src = ./.;
          deps = with pkgs; [
            shellcheck
          ];
          script = ''
            shellcheck bumper.sh
          '';
        };

        nix = {
          src = ./.;
          deps = with pkgs; [
            alejandra
          ];
          script = ''
            alejandra -c .
          '';
        };

        actions = {
          src = ./.;
          deps = with pkgs; [
            prettier
            action-validator
            trev.renovate
          ];
          script = ''
            prettier --check .
            action-validator action.yaml
            action-validator .github/**/*.yaml
            renovate-config-validator .github/renovate.json
          '';
        };
      };

      packages.default = pkgs.stdenv.mkDerivation (finalAttrs: {
        pname = "bumper";
        version = "0.1.18";
        src = ./.;

        nativeBuildInputs = with pkgs; [
          shellcheck
        ];

        runtimeInputs = with pkgs; [
          git
          nodejs_24
          nix-update
        ];

        unpackPhase = ''
          cp "$src/bumper.sh" .
        '';

        dontBuild = true;

        configurePhase = ''
          echo "#!${pkgs.runtimeShell}" >> bumper
          echo "${pkgs.lib.concatMapStringsSep "\n" (option: "set -o ${option}") [
            "errexit"
            "nounset"
            "pipefail"
          ]}" >> bumper
          echo 'export PATH="${pkgs.lib.makeBinPath finalAttrs.runtimeInputs}:$PATH"' >> bumper
          tail -n +2 bumper.sh >> bumper
          chmod +x bumper
        '';

        doCheck = true;
        checkPhase = ''
          shellcheck ./bumper
        '';

        installPhase = ''
          mkdir -p $out/bin
          cp bumper $out/bin/bumper
        '';

        meta = {
          description = "git semantic version bumper";
          mainProgram = "bumper";
          homepage = "https://github.com/spotdemo4/bumper";
          platforms = pkgs.lib.platforms.all;
        };
      });

      formatter = pkgs.alejandra;
    });
}

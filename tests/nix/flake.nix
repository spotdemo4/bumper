{
  description = "bumper";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixpkgs-unstable";
  };

  outputs =
    {
      nixpkgs,
      ...
    }:
    {
      packages.x86_64-linux =
        let
          pkgs = import nixpkgs {
            system = "x86_64-linux";
          };
        in
        {
          default = pkgs.stdenv.mkDerivation (finalAttrs: {
            pname = "test";
            version = "0.7.3";

            src = builtins.path {
              name = "root";
              path = ./.;
            };

            installPhase = "touch $out";
          });

          another = pkgs.stdenv.mkDerivation (finalAttrs: {
            pname = "test2";
            version = "0.7.3";

            src = builtins.path {
              name = "root";
              path = ./.;
            };

            installPhase = "touch $out";
          });
        };
    };
}

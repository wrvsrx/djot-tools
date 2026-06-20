{
  description = "djot-language-server";

  inputs = {
    nur-wrvsrx.url = "github:wrvsrx/nur-packages";
    nixpkgs.follows = "nur-wrvsrx/nixpkgs";
    flake-parts.follows = "nur-wrvsrx/flake-parts";
    crane.url = "github:ipetkov/crane/v0.23.4";
  };

  outputs =
    inputs:
    inputs.flake-parts.lib.mkFlake { inherit inputs; } (
      { inputs, ... }:
      {
        systems = [ "x86_64-linux" ];
        perSystem =
          { pkgs, ... }:
          let
            craneLib = inputs.crane.mkLib pkgs;
          in
          {
            packages.default = pkgs.callPackage ./default.nix {
              inherit craneLib;
            };
            devShells.default = pkgs.callPackage ./shell.nix { };
            formatter = pkgs.nixfmt-rfc-style;
          };
      }
    );
}

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
            cargoToml = builtins.fromTOML (builtins.readFile ./Cargo.toml);
          in
          {
            packages.default = craneLib.buildPackage {
              pname = "djot-tools";
              version = cargoToml.workspace.package.version;

              src = craneLib.cleanCargoSource ./.;

              meta = {
                description = "Language server and tools for Djot documents";
                license = pkgs.lib.licenses.mit;
                mainProgram = "djot-ls";
              };
            };
            devShells.default = craneLib.devShell { };
            formatter = pkgs.nixfmt-rfc-style;
          };
      }
    );
}

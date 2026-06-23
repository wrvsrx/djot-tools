{
  description = "djot-tools";

  inputs = {
    nur-wrvsrx.url = "github:wrvsrx/nur-packages";
    nixpkgs.follows = "nur-wrvsrx/nixpkgs";
    flake-parts.follows = "nur-wrvsrx/flake-parts";
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
            cargoToml = builtins.fromTOML (builtins.readFile ./Cargo.toml);
          in
          {
            packages.default = pkgs.rustPlatform.buildRustPackage {
              pname = "djot-tools";
              version = cargoToml.workspace.package.version;

              src = pkgs.lib.cleanSource ./.;

              cargoLock = {
                lockFile = ./Cargo.lock;
              };

              meta = {
                description = "Language server and tools for Djot documents";
                license = pkgs.lib.licenses.mit;
                mainProgram = "djot-ls";
              };
            };
            devShells.default = pkgs.mkShell {
              packages = with pkgs; [
                cargo
                rustc
                rustfmt
              ];
            };
            formatter = pkgs.nixfmt-rfc-style;
          };
      }
    );
}

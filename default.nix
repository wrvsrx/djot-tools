{
  pkgs ? import <nixpkgs> { },
  lib ? pkgs.lib,
  craneLib,
}:

let
  cargoToml = builtins.fromTOML (builtins.readFile ./Cargo.toml);
in
craneLib.buildPackage {
  pname = "djot-tools";
  version = cargoToml.workspace.package.version;

  src = craneLib.cleanCargoSource ./.;

  meta = {
    description = "Language server and tools for Djot documents";
    license = lib.licenses.mit;
    mainProgram = "djot-ls";
  };
}

{
  mkShell,
  cargo,
  nodejs,
}:
mkShell {
  nativeBuildInputs = [
    cargo
    nodejs
  ];
}

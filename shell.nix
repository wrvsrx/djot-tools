{
  mkShell,
  cargo,
  nodejs,
  tree-sitter,
}:
mkShell {
  nativeBuildInputs = [
    cargo
    nodejs
    tree-sitter
  ];
}

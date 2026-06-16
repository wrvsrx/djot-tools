{
  mkShell,
  cargo,
  rustc,
  nodejs,
  tree-sitter,
  rustfmt,
}:
mkShell {
  nativeBuildInputs = [
    cargo
    rustc
    nodejs
    tree-sitter
    rustfmt
  ];
}

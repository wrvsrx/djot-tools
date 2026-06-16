vim.lsp.config['djot-language-server'] = {
  cmd = { './target/debug/djot-ls' },
  filetypes = { 'djot' },
  root_dir = vim.fs.root(0, { '.git' }),
}

vim.lsp.enable('djot-language-server')

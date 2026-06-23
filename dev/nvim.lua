local capabilities = vim.lsp.protocol.make_client_capabilities()
capabilities.workspace.workspaceEdit.documentChanges = true
-- Neovim disables workspace/didChangeWatchedFiles by default on Linux. Opt in
-- for local djot-ls testing; install inotify-tools for the inotify backend.
capabilities.workspace.didChangeWatchedFiles =
  capabilities.workspace.didChangeWatchedFiles or {}
capabilities.workspace.didChangeWatchedFiles.dynamicRegistration = true
capabilities.workspace.didChangeWatchedFiles.relativePatternSupport = true

vim.lsp.config['djot-ls'] = {
  cmd = { './target/debug/djot-ls' },
  filetypes = { 'djot' },
  root_dir = vim.fs.root(0, { '.git' }),
  capabilities = capabilities,
}

vim.lsp.enable('djot-ls')

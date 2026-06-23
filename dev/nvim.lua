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

local function set_djot_semantic_highlights()
  vim.api.nvim_set_hl(0, '@lsp.typemod.task.completed.djot', {
    link = 'Comment',
    default = true,
  })
end

set_djot_semantic_highlights()

vim.api.nvim_create_autocmd('ColorScheme', {
  callback = set_djot_semantic_highlights,
})

vim.lsp.enable('djot-ls')

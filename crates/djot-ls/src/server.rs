use std::collections::HashSet;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};

use async_lsp::{ClientSocket, LanguageClient, LanguageServer, ResponseError};
use djot_core::{resolve_target, PathRenameError, RefTarget, RenameTargetError, Workspace};
use futures::future::BoxFuture;
use lsp_types::{
    CodeActionKind, CodeActionOptions, CodeActionParams, CodeActionProviderCapability,
    CodeActionResponse, CompletionItem, CompletionItemKind, CompletionOptions, CompletionParams,
    CompletionResponse, DidChangeConfigurationParams, DidChangeTextDocumentParams,
    DidChangeWatchedFilesParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    DidSaveTextDocumentParams, DocumentSymbolParams, DocumentSymbolResponse, FileChangeType,
    FileSystemWatcher, GlobPattern, GotoDefinitionParams, GotoDefinitionResponse, Hover,
    HoverContents, HoverParams, HoverProviderCapability, InitializeParams, InitializeResult,
    InitializedParams, Location, MarkupContent, MarkupKind, OneOf, Position, PrepareRenameResponse,
    ReferenceParams, Registration, RegistrationParams, RenameOptions, RenameParams,
    SemanticTokensParams, SemanticTokensResult, ServerCapabilities, TextDocumentPositionParams,
    TextDocumentSyncCapability, TextDocumentSyncKind, Url, WatchKind, WorkDoneProgressOptions,
    WorkspaceEdit,
};

use crate::code_action::resolve_code_actions as resolve_code_actions_for_document;
use crate::completion::*;
use crate::hover::{anchor_hover_markdown, file_hover_markdown, task_hover_markdown, TaskHover};
use crate::lsp_utils::*;
use crate::path_utils::{is_djot_file, relative_link_path};
use crate::position::{byte_range_to_lsp, position_to_offset};
use crate::rename::{anchor_rename_workspace_edit, path_rename_workspace_edit};
use crate::semantic_tokens::{semantic_tokens_provider, task_semantic_tokens};
use crate::symbols::{document_symbols, document_title};

/// Server state. async-lsp's omni-trait hands us `&mut self` on every request and
/// notification, so plain owned state needs no locking.
pub(crate) struct ServerState {
    #[allow(dead_code)]
    pub(crate) client: ClientSocket,
    /// Parsed documents, keyed by file path. Open buffers are inserted on
    /// did_open/did_change; cross-file link targets are loaded from disk lazily.
    pub(crate) workspace: Workspace,
    /// Roots supplied by the LSP client during initialize.
    pub(crate) workspace_roots: Vec<PathBuf>,
    /// Client support for workspace edits that include resource operations.
    #[allow(dead_code)]
    pub(crate) workspace_edit_capabilities: ClientWorkspaceEditCapabilities,
    /// Client support for dynamically registering workspace file watchers.
    pub(crate) file_watch_capabilities: ClientFileWatchCapabilities,
    /// Path renames returned optimistically before the client applies the
    /// workspace edit and file watchers report the real file-system outcome.
    pub(crate) pending_path_renames: Vec<PendingPathRename>,
    /// Open buffers that should receive publishDiagnostics updates.
    pub(crate) open_documents: HashSet<PathBuf>,
}

pub(crate) struct PendingPathRename {
    old_path: PathBuf,
    new_path: PathBuf,
    old_removed: bool,
    new_seen: bool,
}

impl ServerState {
    pub(crate) fn new(client: ClientSocket) -> Self {
        Self {
            client,
            workspace: Workspace::new(),
            workspace_roots: Vec::new(),
            workspace_edit_capabilities: ClientWorkspaceEditCapabilities::default(),
            file_watch_capabilities: ClientFileWatchCapabilities::default(),
            pending_path_renames: Vec::new(),
            open_documents: HashSet::new(),
        }
    }
}

impl LanguageServer for ServerState {
    type Error = ResponseError;
    type NotifyResult = ControlFlow<async_lsp::Result<()>>;

    fn initialize(
        &mut self,
        params: InitializeParams,
    ) -> BoxFuture<'static, Result<InitializeResult, Self::Error>> {
        self.workspace_roots = workspace_roots(&params);
        self.workspace_edit_capabilities = client_workspace_edit_capabilities(&params);
        self.file_watch_capabilities = client_file_watch_capabilities(&params);

        Box::pin(async move {
            Ok(InitializeResult {
                capabilities: ServerCapabilities {
                    // Full-document sync keeps things simple for now.
                    text_document_sync: Some(TextDocumentSyncCapability::Kind(
                        TextDocumentSyncKind::FULL,
                    )),
                    document_symbol_provider: Some(OneOf::Left(true)),
                    definition_provider: Some(OneOf::Left(true)),
                    references_provider: Some(OneOf::Left(true)),
                    hover_provider: Some(HoverProviderCapability::Simple(true)),
                    rename_provider: Some(OneOf::Right(RenameOptions {
                        prepare_provider: Some(true),
                        work_done_progress_options: WorkDoneProgressOptions::default(),
                    })),
                    completion_provider: Some(CompletionOptions {
                        resolve_provider: Some(false),
                        trigger_characters: Some(vec![
                            "[".to_string(),
                            "(".to_string(),
                            "/".to_string(),
                            "#".to_string(),
                        ]),
                        all_commit_characters: None,
                        work_done_progress_options: WorkDoneProgressOptions::default(),
                        completion_item: None,
                    }),
                    code_action_provider: Some(CodeActionProviderCapability::Options(
                        CodeActionOptions {
                            code_action_kinds: Some(vec![
                                CodeActionKind::QUICKFIX,
                                CodeActionKind::REFACTOR_REWRITE,
                            ]),
                            resolve_provider: Some(false),
                            work_done_progress_options: WorkDoneProgressOptions::default(),
                        },
                    )),
                    semantic_tokens_provider: Some(semantic_tokens_provider()),
                    ..ServerCapabilities::default()
                },
                server_info: None,
            })
        })
    }

    fn initialized(&mut self, _params: InitializedParams) -> Self::NotifyResult {
        self.index_workspace_roots_with_progress();
        self.register_workspace_file_watchers();
        self.publish_open_document_diagnostics();
        ControlFlow::Continue(())
    }

    fn did_open(&mut self, params: DidOpenTextDocumentParams) -> Self::NotifyResult {
        let doc = params.text_document;
        if let Ok(path) = doc.uri.to_file_path() {
            self.workspace.insert(path.clone(), doc.text);
            self.open_documents.insert(path);
            self.publish_open_document_diagnostics();
        }
        ControlFlow::Continue(())
    }

    fn did_change(&mut self, params: DidChangeTextDocumentParams) -> Self::NotifyResult {
        // FULL sync: the last change contains the entire document.
        if let Some(change) = params.content_changes.into_iter().last() {
            if let Ok(path) = params.text_document.uri.to_file_path() {
                self.workspace.insert(path, change.text);
                self.publish_open_document_diagnostics();
            }
        }
        ControlFlow::Continue(())
    }

    fn did_close(&mut self, params: DidCloseTextDocumentParams) -> Self::NotifyResult {
        if let Ok(path) = params.text_document.uri.to_file_path() {
            self.open_documents.remove(&path);
            self.clear_diagnostics_for(&path);
            // Drop the open-buffer text. For workspace files, keep the disk
            // version indexed so cross-file lookups and references remain
            // available after the editor closes the buffer.
            if self.is_in_workspace(&path) {
                if let Ok(text) = std::fs::read_to_string(&path) {
                    self.workspace.insert(path, text);
                } else {
                    self.workspace.remove(&path);
                }
            } else {
                self.workspace.remove(&path);
            }
            self.publish_open_document_diagnostics();
        }
        ControlFlow::Continue(())
    }

    // async-lsp breaks the main loop on any notification we don't explicitly
    // handle (the omni-trait default is `ControlFlow::Break(Routing(..))`), so
    // editors sending these would otherwise kill the server. Accept and ignore
    // them for now; `did_save` is a natural hook for re-running diagnostics later.
    fn did_save(&mut self, _params: DidSaveTextDocumentParams) -> Self::NotifyResult {
        ControlFlow::Continue(())
    }

    fn did_change_configuration(
        &mut self,
        _params: DidChangeConfigurationParams,
    ) -> Self::NotifyResult {
        ControlFlow::Continue(())
    }

    fn did_change_watched_files(
        &mut self,
        params: DidChangeWatchedFilesParams,
    ) -> Self::NotifyResult {
        for change in params.changes {
            let Ok(path) = change.uri.to_file_path() else {
                continue;
            };
            if !is_djot_file(&path) {
                continue;
            }
            match change.typ {
                FileChangeType::CREATED | FileChangeType::CHANGED => {
                    self.confirm_pending_path_rename(&path);
                    if !self.open_documents.contains(&path) {
                        if let Ok(text) = std::fs::read_to_string(&path) {
                            self.workspace.insert(path, text);
                        }
                    }
                }
                FileChangeType::DELETED => {
                    self.confirm_pending_path_rename(&path);
                    if !self.open_documents.contains(&path) {
                        self.workspace.remove(&path);
                    }
                }
                _ => {}
            }
        }
        self.publish_open_document_diagnostics();
        ControlFlow::Continue(())
    }

    fn document_symbol(
        &mut self,
        params: DocumentSymbolParams,
    ) -> BoxFuture<'static, Result<Option<DocumentSymbolResponse>, Self::Error>> {
        let symbols = params
            .text_document
            .uri
            .to_file_path()
            .ok()
            .and_then(|path| {
                self.workspace
                    .get(&path)
                    .map(|entry| document_symbols(&entry.text))
            });
        Box::pin(async move { Ok(symbols.map(DocumentSymbolResponse::Nested)) })
    }

    fn definition(
        &mut self,
        params: GotoDefinitionParams,
    ) -> BoxFuture<'static, Result<Option<GotoDefinitionResponse>, Self::Error>> {
        let pos = params.text_document_position_params;
        let location = self.resolve_definition(&pos.text_document.uri, pos.position);
        Box::pin(async move { Ok(location.map(GotoDefinitionResponse::Scalar)) })
    }

    fn references(
        &mut self,
        params: ReferenceParams,
    ) -> BoxFuture<'static, Result<Option<Vec<Location>>, Self::Error>> {
        let pos = params.text_document_position;
        let locations = self.resolve_references(
            &pos.text_document.uri,
            pos.position,
            params.context.include_declaration,
        );
        Box::pin(async move { Ok(locations) })
    }

    fn hover(
        &mut self,
        params: HoverParams,
    ) -> BoxFuture<'static, Result<Option<Hover>, Self::Error>> {
        let pos = params.text_document_position_params;
        let hover = self.resolve_hover(&pos.text_document.uri, pos.position);
        Box::pin(async move { Ok(hover) })
    }

    fn completion(
        &mut self,
        params: CompletionParams,
    ) -> BoxFuture<'static, Result<Option<CompletionResponse>, Self::Error>> {
        let pos = params.text_document_position;
        let completions = self.resolve_completion(&pos.text_document.uri, pos.position);
        Box::pin(async move { Ok(completions.map(CompletionResponse::Array)) })
    }

    fn code_action(
        &mut self,
        params: CodeActionParams,
    ) -> BoxFuture<'static, Result<Option<CodeActionResponse>, Self::Error>> {
        let actions = self.resolve_code_actions(&params);
        Box::pin(async move { Ok(actions) })
    }

    fn semantic_tokens_full(
        &mut self,
        params: SemanticTokensParams,
    ) -> BoxFuture<'static, Result<Option<SemanticTokensResult>, Self::Error>> {
        let tokens = params
            .text_document
            .uri
            .to_file_path()
            .ok()
            .and_then(|path| {
                self.workspace.get(&path).map(|entry| {
                    task_semantic_tokens(
                        &entry.text,
                        &entry.analysis.tasks,
                        &entry.analysis.native_task_list_items,
                    )
                    .into()
                })
            });
        Box::pin(async move { Ok(tokens) })
    }

    fn prepare_rename(
        &mut self,
        params: TextDocumentPositionParams,
    ) -> BoxFuture<'static, Result<Option<PrepareRenameResponse>, Self::Error>> {
        let response = self.resolve_prepare_rename(&params.text_document.uri, params.position);
        Box::pin(async move { response })
    }

    fn rename(
        &mut self,
        params: RenameParams,
    ) -> BoxFuture<'static, Result<Option<WorkspaceEdit>, Self::Error>> {
        let pos = params.text_document_position;
        let edit = self.resolve_rename(&pos.text_document.uri, pos.position, params.new_name);
        Box::pin(async move { edit })
    }
}

impl ServerState {
    fn register_workspace_file_watchers(&self) {
        if !self.file_watch_capabilities.dynamic_registration || self.workspace_roots.is_empty() {
            return;
        }

        let params = RegistrationParams {
            registrations: vec![Registration {
                id: "djot-ls-workspace-djot-files".to_string(),
                method: "workspace/didChangeWatchedFiles".to_string(),
                register_options: Some(
                    serde_json::to_value(lsp_types::DidChangeWatchedFilesRegistrationOptions {
                        watchers: vec![
                            FileSystemWatcher {
                                glob_pattern: GlobPattern::String("**/*.dj".to_string()),
                                kind: Some(
                                    WatchKind::Create | WatchKind::Change | WatchKind::Delete,
                                ),
                            },
                            FileSystemWatcher {
                                glob_pattern: GlobPattern::String("**/*.djot".to_string()),
                                kind: Some(
                                    WatchKind::Create | WatchKind::Change | WatchKind::Delete,
                                ),
                            },
                        ],
                    })
                    .expect("watched files registration options should serialize"),
                ),
            }],
        };

        let mut client = self.client.clone();
        tokio::spawn(async move {
            let _ = client
                .register_capability(params)
                .await
                .inspect_err(|err| tracing::warn!("failed to register file watchers: {err}"));
        });
    }

    /// Resolve goto-definition for the link under `position` in `uri`. Same-file
    /// `#id` links and cross-file `path#id` links are handled uniformly through
    /// the workspace index; a cross-file target not yet indexed is loaded from
    /// disk on demand.
    fn resolve_definition(&mut self, uri: &Url, position: Position) -> Option<Location> {
        let from = uri.to_file_path().ok()?;
        let offset = position_to_offset(&self.workspace.get(&from)?.text, position);

        // Resolve the link under the cursor to a (path, id) target.
        let target = {
            let reference = self.workspace.reference_at(&from, offset)?;
            resolve_target(&from, &reference.target)?
        };

        // Pull the target file into the index if we have not parsed it yet.
        if !self.workspace.contains(&target.path) {
            if let Ok(text) = std::fs::read_to_string(&target.path) {
                self.workspace.insert(target.path.clone(), text);
            }
        }

        let entry = self.workspace.get(&target.path)?;
        let range = match &target.id {
            Some(id) => entry.analysis.index.anchors.get(id)?.range.clone(),
            None => 0..0, // a link to the file itself jumps to its top
        };
        Some(Location {
            uri: Url::from_file_path(&target.path).ok()?,
            range: byte_range_to_lsp(&entry.text, &range),
        })
    }

    /// Resolve find-references for either an anchor under the cursor or a link
    /// under the cursor. Only anchored targets (`#id` / `path#id`) have
    /// references; file-only links do not name a symbol.
    fn resolve_references(
        &mut self,
        uri: &Url,
        position: Position,
        include_declaration: bool,
    ) -> Option<Vec<Location>> {
        let from = uri.to_file_path().ok()?;
        let offset = position_to_offset(&self.workspace.get(&from)?.text, position);
        let (target_path, target_id) = self.reference_target_at(&from, offset)?;

        let mut locations = Vec::new();
        if include_declaration {
            let entry = self.workspace.get(&target_path)?;
            let anchor = entry.analysis.index.anchors.get(&target_id)?;
            locations.push(Location {
                uri: Url::from_file_path(&target_path).ok()?,
                range: byte_range_to_lsp(&entry.text, &anchor.range),
            });
        }

        for (path, range) in self.workspace.references_to(&target_path, &target_id) {
            let Some(entry) = self.workspace.get(&path) else {
                continue;
            };
            let Some(uri) = Url::from_file_path(&path).ok() else {
                continue;
            };
            locations.push(Location {
                uri,
                range: byte_range_to_lsp(&entry.text, &range),
            });
        }

        Some(locations)
    }

    fn reference_target_at(&self, path: &Path, offset: usize) -> Option<(PathBuf, String)> {
        if let Some((id, _)) = self.workspace.anchor_at(path, offset) {
            return Some((path.to_path_buf(), id.to_string()));
        }

        let reference = self.workspace.reference_at(path, offset)?;
        let target = resolve_target(path, &reference.target)?;
        target.id.map(|id| (target.path, id))
    }

    fn resolve_prepare_rename(
        &self,
        uri: &Url,
        position: Position,
    ) -> Result<Option<PrepareRenameResponse>, ResponseError> {
        let from = match uri.to_file_path() {
            Ok(path) => path,
            Err(()) => return Ok(None),
        };
        let Some(entry) = self.workspace.get(&from) else {
            return Ok(None);
        };
        let offset = position_to_offset(&entry.text, position);
        match self.workspace.rename_target_at(&from, offset) {
            Ok(target) => {
                return Ok(Some(PrepareRenameResponse::RangeWithPlaceholder {
                    range: byte_range_to_lsp(&entry.text, &target.range),
                    placeholder: target.id,
                }));
            }
            Err(RenameTargetError::NotRenameable) => {}
            Err(RenameTargetError::ImplicitHeadingAnchor) => {
                return Err(implicit_heading_rename_error());
            }
        }

        let target = match self.workspace.path_rename_target_at(&from, offset) {
            Ok(target) => target,
            Err(PathRenameError::NotRenameable) => return Ok(None),
            Err(PathRenameError::NonDjotPath) => return Err(non_djot_path_rename_error()),
            Err(PathRenameError::TargetNotIndexed) => return Err(unindexed_path_rename_error()),
        };
        let placeholder = entry
            .text
            .get(target.range.clone())
            .unwrap_or_default()
            .to_string();
        Ok(Some(PrepareRenameResponse::RangeWithPlaceholder {
            range: byte_range_to_lsp(&entry.text, &target.range),
            placeholder,
        }))
    }

    fn resolve_rename(
        &mut self,
        uri: &Url,
        position: Position,
        new_name: String,
    ) -> Result<Option<WorkspaceEdit>, ResponseError> {
        let from = match uri.to_file_path() {
            Ok(path) => path,
            Err(()) => return Ok(None),
        };
        let Some(entry) = self.workspace.get(&from) else {
            return Ok(None);
        };
        let offset = position_to_offset(&entry.text, position);
        match self.workspace.rename_target_at(&from, offset) {
            Ok(target) => {
                if !is_valid_anchor_id(&new_name) {
                    return Ok(None);
                }
                return self.resolve_anchor_rename(&target.path, &target.id, new_name);
            }
            Err(RenameTargetError::NotRenameable) => {}
            Err(RenameTargetError::ImplicitHeadingAnchor) => {
                return Err(implicit_heading_rename_error());
            }
        }

        self.resolve_path_rename(&from, offset, new_name)
    }

    fn resolve_anchor_rename(
        &self,
        target_path: &Path,
        target_id: &str,
        new_name: String,
    ) -> Result<Option<WorkspaceEdit>, ResponseError> {
        Ok(anchor_rename_workspace_edit(
            &self.workspace,
            target_path,
            target_id,
            &new_name,
        ))
    }

    fn resolve_path_rename(
        &mut self,
        from: &Path,
        offset: usize,
        new_name: String,
    ) -> Result<Option<WorkspaceEdit>, ResponseError> {
        let target = match self.workspace.path_rename_target_at(from, offset) {
            Ok(target) => target,
            Err(PathRenameError::NotRenameable) => return Ok(None),
            Err(PathRenameError::NonDjotPath) => return Err(non_djot_path_rename_error()),
            Err(PathRenameError::TargetNotIndexed) => return Err(unindexed_path_rename_error()),
        };

        if !self.workspace_edit_capabilities.document_changes {
            return Err(document_changes_capability_error());
        }
        if !self.workspace_edit_capabilities.rename_resource_operation {
            return Err(rename_resource_operation_capability_error());
        }

        let new_path = self.resolve_new_link_path(from, &new_name)?;
        if new_path == target.old_path {
            return Ok(None);
        }
        if self.workspace.contains(&new_path) || new_path.exists() {
            return Err(rename_target_exists_error());
        }

        let edit = path_rename_workspace_edit(&self.workspace, &target.old_path, &new_path)?;

        if let Some(entry) = self.workspace.get(&target.old_path) {
            let text = entry.text.clone();
            self.workspace.insert(new_path.clone(), text);
            self.workspace.remove(&target.old_path);
        }
        self.pending_path_renames.push(PendingPathRename {
            old_path: target.old_path,
            new_path,
            old_removed: false,
            new_seen: false,
        });

        Ok(edit)
    }

    fn confirm_pending_path_rename(&mut self, changed_path: &Path) {
        for rename in &mut self.pending_path_renames {
            if changed_path == rename.old_path {
                rename.old_removed = !rename.old_path.exists();
                if rename.old_removed {
                    self.workspace.remove(&rename.old_path);
                }
                if !rename.new_path.exists() && !self.open_documents.contains(&rename.new_path) {
                    self.workspace.remove(&rename.new_path);
                    rename.new_seen = false;
                }
            } else if changed_path == rename.new_path {
                rename.new_seen = rename.new_path.exists();
                if rename.new_seen {
                    if let Ok(text) = std::fs::read_to_string(&rename.new_path) {
                        self.workspace.insert(rename.new_path.clone(), text);
                    }
                } else if !self.open_documents.contains(&rename.new_path) {
                    self.workspace.remove(&rename.new_path);
                }
                rename.old_removed = !rename.old_path.exists();
            }
        }

        self.pending_path_renames
            .retain(|rename| !(rename.old_removed && rename.new_seen));
    }

    fn resolve_new_link_path(&self, from: &Path, new_name: &str) -> Result<PathBuf, ResponseError> {
        if !is_valid_link_path_rename(new_name) {
            return Err(invalid_rename_path_error());
        }
        let target = resolve_target(
            from,
            &RefTarget::External {
                path: new_name.to_string(),
                id: None,
            },
        )
        .ok_or_else(invalid_rename_path_error)?;
        if !is_djot_file(&target.path) {
            return Err(non_djot_path_rename_error());
        }
        if !self.workspace_roots.is_empty()
            && !self
                .workspace_roots
                .iter()
                .any(|root| target.path.starts_with(root))
        {
            return Err(rename_target_outside_workspace_error());
        }
        Ok(target.path)
    }

    fn resolve_hover(&mut self, uri: &Url, position: Position) -> Option<Hover> {
        let from = uri.to_file_path().ok()?;
        let offset = position_to_offset(&self.workspace.get(&from)?.text, position);

        if let Some(reference) = self.workspace.reference_at(&from, offset) {
            let target = resolve_target(&from, &reference.target)?;
            let source_range = reference.source.clone();

            if !self.workspace.contains(&target.path) {
                if let Ok(text) = std::fs::read_to_string(&target.path) {
                    self.workspace.insert(target.path.clone(), text);
                }
            }

            let source_lsp_range = {
                let entry = self.workspace.get(&from)?;
                byte_range_to_lsp(&entry.text, &source_range)
            };
            let entry = self.workspace.get(&target.path)?;
            let value = match &target.id {
                Some(id) => {
                    let anchor = entry.analysis.index.anchors.get(id)?;
                    anchor_hover_markdown(
                        self.display_path(&target.path),
                        id,
                        &entry.text,
                        &anchor.range,
                    )
                }
                None => file_hover_markdown(self.display_path(&target.path), &entry.text),
            };

            return Some(Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value,
                }),
                range: Some(source_lsp_range),
            });
        }

        if let Some(task) = self.workspace.task_at(&from, offset).cloned() {
            let entry = self.workspace.get(&from)?;
            return Some(Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: task_hover_markdown(TaskHover {
                        title: &task.title,
                        id: task.id.as_deref(),
                        created: task.created.as_deref(),
                        due: task.due.as_deref(),
                        wait: task.wait.as_deref(),
                        done: task.done.as_deref(),
                        canceled: task.canceled.as_deref(),
                        recur: task.recur.as_deref(),
                        prev: task.prev.as_deref(),
                        depends: task
                            .depends
                            .iter()
                            .map(|dependency| dependency.source.clone())
                            .collect(),
                        blockers: self
                            .workspace
                            .open_task_dependencies(&from, &task)
                            .into_iter()
                            .map(|dependency| {
                                format!(
                                    "{}#{}",
                                    relative_link_path(&from, &dependency.target.path)
                                        .unwrap_or_else(|| {
                                            self.display_path(&dependency.target.path)
                                        }),
                                    dependency.target.id
                                )
                            })
                            .collect(),
                    }),
                }),
                range: Some(byte_range_to_lsp(
                    &entry.text,
                    &task
                        .title_range
                        .clone()
                        .unwrap_or_else(|| task.range.clone()),
                )),
            });
        }

        let (id, anchor) = self.workspace.anchor_at(&from, offset)?;
        let entry = self.workspace.get(&from)?;
        Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: anchor_hover_markdown(
                    self.display_path(&from),
                    id,
                    &entry.text,
                    &anchor.range,
                ),
            }),
            range: Some(byte_range_to_lsp(&entry.text, &anchor.range)),
        })
    }

    fn resolve_completion(&self, uri: &Url, position: Position) -> Option<Vec<CompletionItem>> {
        let from = uri.to_file_path().ok()?;
        let entry = self.workspace.get(&from)?;
        let offset = position_to_offset(&entry.text, position);
        let context = link_completion_context(&entry.text, offset)?;

        let items = match context {
            LinkCompletionContext::Label { replace, query } => self
                .workspace_link_targets(&from)
                .into_iter()
                .filter(|target| {
                    fuzzy_match(&query, &target.title) || fuzzy_match(&query, &target.path)
                })
                .map(|target| {
                    completion_item(
                        target.title.clone(),
                        Some(target.path.clone()),
                        format!(
                            "[{}]({})",
                            escape_link_label(&target.title),
                            escape_link_destination(&target.path)
                        ),
                        &entry.text,
                        &replace,
                        CompletionItemKind::FILE,
                    )
                })
                .collect(),
            LinkCompletionContext::Destination { replace, query } => self
                .workspace_link_targets(&from)
                .into_iter()
                .filter(|target| {
                    fuzzy_match(&query, &target.path) || fuzzy_match(&query, &target.title)
                })
                .map(|target| {
                    completion_item(
                        target.path.clone(),
                        Some(target.title.clone()),
                        escape_link_destination(&target.path),
                        &entry.text,
                        &replace,
                        CompletionItemKind::FILE,
                    )
                })
                .collect(),
            LinkCompletionContext::Anchor {
                path,
                replace,
                query,
            } => self
                .anchor_completions(&from, &path)?
                .into_iter()
                .filter(|anchor| fuzzy_match(&query, &anchor.id))
                .map(|anchor| {
                    completion_item(
                        anchor.id.clone(),
                        Some(anchor.path.clone()),
                        escape_link_destination(&anchor.id),
                        &entry.text,
                        &replace,
                        CompletionItemKind::REFERENCE,
                    )
                })
                .collect(),
        };

        Some(items)
    }

    fn resolve_code_actions(&self, params: &CodeActionParams) -> Option<CodeActionResponse> {
        let path = params.text_document.uri.to_file_path().ok()?;
        let entry = self.workspace.get(&path)?;
        let offset = position_to_offset(&entry.text, params.range.start);
        Some(resolve_code_actions_for_document(
            &self.workspace,
            params,
            &path,
            entry,
            offset,
        ))
    }

    fn workspace_link_targets(&self, from: &Path) -> Vec<LinkTargetCompletion> {
        let mut targets: Vec<_> = self
            .workspace
            .documents()
            .map(|(path, entry)| {
                let path =
                    relative_link_path(from, path).unwrap_or_else(|| self.display_path(path));
                let title = document_title(&entry.text).unwrap_or_else(|| path.clone());
                LinkTargetCompletion { title, path }
            })
            .collect();
        targets.sort_by(|a, b| {
            a.title
                .to_lowercase()
                .cmp(&b.title.to_lowercase())
                .then_with(|| a.path.cmp(&b.path))
        });
        targets
    }

    fn anchor_completions(&self, from: &Path, link_path: &str) -> Option<Vec<AnchorCompletion>> {
        let target_path = if link_path.is_empty() {
            from.to_path_buf()
        } else {
            resolve_target(
                from,
                &RefTarget::External {
                    path: link_path.to_string(),
                    id: None,
                },
            )?
            .path
        };

        let entry = self.workspace.get(&target_path)?;
        let display_path = relative_link_path(from, &target_path).unwrap_or_else(|| {
            self.workspace_roots
                .iter()
                .find_map(|root| target_path.strip_prefix(root).ok())
                .unwrap_or(&target_path)
                .display()
                .to_string()
        });
        let mut anchors: Vec<_> = entry
            .analysis
            .index
            .anchors
            .keys()
            .map(|id| AnchorCompletion {
                id: id.clone(),
                path: display_path.clone(),
            })
            .collect();
        anchors.sort_by(|a, b| a.id.to_lowercase().cmp(&b.id.to_lowercase()));
        Some(anchors)
    }
}

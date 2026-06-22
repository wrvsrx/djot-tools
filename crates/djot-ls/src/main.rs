use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::OsString;
use std::ops::{ControlFlow, Range as ByteRange};
use std::path::{Component, Path, PathBuf};

use async_lsp::client_monitor::ClientProcessMonitorLayer;
use async_lsp::concurrency::ConcurrencyLayer;
use async_lsp::panic::CatchUnwindLayer;
use async_lsp::router::Router;
use async_lsp::server::LifecycleLayer;
use async_lsp::tracing::TracingLayer;
use async_lsp::{ClientSocket, ErrorCode, LanguageServer, ResponseError};
use chrono::{DateTime, Datelike, Duration, FixedOffset, Local, SecondsFormat, TimeZone, Timelike};
use djot_core::{
    build_index, heading_outline, metadata_block, parse_repeat_rule, resolve_target, tasks,
    AnalysisDiagnostic, DiagnosticKind, Heading, PathRenameError, RefTarget, RenameTargetError,
    RepeatRule, Workspace,
};
use futures::future::BoxFuture;
use jotdown::{Container, Event, Parser};
use lsp_types::{
    CodeAction, CodeActionKind, CodeActionOptions, CodeActionOrCommand, CodeActionParams,
    CodeActionProviderCapability, CodeActionResponse, CompletionItem, CompletionItemKind,
    CompletionOptions, CompletionParams, CompletionResponse, CompletionTextEdit, Diagnostic,
    DiagnosticRelatedInformation, DiagnosticSeverity, DidChangeConfigurationParams,
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    DidSaveTextDocumentParams, DocumentChangeOperation, DocumentChanges, DocumentSymbol,
    DocumentSymbolParams, DocumentSymbolResponse, GotoDefinitionParams, GotoDefinitionResponse,
    Hover, HoverContents, HoverParams, HoverProviderCapability, InitializeParams, InitializeResult,
    InitializedParams, Location, MarkupContent, MarkupKind, NumberOrString, OneOf,
    OptionalVersionedTextDocumentIdentifier, Position, PrepareRenameResponse, ProgressParams,
    ProgressParamsValue, PublishDiagnosticsParams, Range, ReferenceParams, RenameFile,
    RenameFileOptions, RenameOptions, RenameParams, ResourceOp, ResourceOperationKind,
    ServerCapabilities, SymbolKind, TextDocumentEdit, TextDocumentPositionParams,
    TextDocumentSyncCapability, TextDocumentSyncKind, TextEdit, Url, WorkDoneProgress,
    WorkDoneProgressBegin, WorkDoneProgressEnd, WorkDoneProgressOptions, WorkDoneProgressReport,
    WorkspaceEdit,
};
use tower::ServiceBuilder;
use tracing::Level;

/// Server state. async-lsp's omni-trait hands us `&mut self` on every request and
/// notification, so plain owned state needs no locking.
struct ServerState {
    #[allow(dead_code)]
    client: ClientSocket,
    /// Parsed documents, keyed by file path. Open buffers are inserted on
    /// did_open/did_change; cross-file link targets are loaded from disk lazily.
    workspace: Workspace,
    /// Roots supplied by the LSP client during initialize.
    workspace_roots: Vec<PathBuf>,
    /// Client support for workspace edits that include resource operations.
    #[allow(dead_code)]
    workspace_edit_capabilities: ClientWorkspaceEditCapabilities,
    /// Open buffers that should receive publishDiagnostics updates.
    open_documents: HashSet<PathBuf>,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, Default)]
struct ClientWorkspaceEditCapabilities {
    document_changes: bool,
    rename_resource_operation: bool,
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
                    ..ServerCapabilities::default()
                },
                server_info: None,
            })
        })
    }

    fn initialized(&mut self, _params: InitializedParams) -> Self::NotifyResult {
        self.index_workspace_roots_with_progress();
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
                self.workspace.get(&path).map(|entry| {
                    heading_outline(&entry.text)
                        .iter()
                        .map(|h| to_document_symbol(&entry.text, h))
                        .collect::<Vec<_>>()
                })
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
    fn index_workspace_root(&mut self, root: &Path) -> usize {
        index_djot_files(root, &mut |path, text| {
            self.workspace.insert(path, text);
        })
    }

    fn is_in_workspace(&self, path: &Path) -> bool {
        self.workspace_roots
            .iter()
            .any(|root| path.starts_with(root))
    }

    fn index_workspace_roots_with_progress(&mut self) {
        if self.workspace_roots.is_empty() {
            return;
        }

        self.notify_index_progress(WorkDoneProgress::Begin(WorkDoneProgressBegin {
            title: "Indexing Djot workspace".to_string(),
            cancellable: Some(false),
            message: Some("Scanning .dj/.djot files".to_string()),
            percentage: None,
        }));

        let mut indexed = 0usize;
        for root in self.workspace_roots.clone() {
            indexed += self.index_workspace_root(&root);
        }

        self.notify_index_progress(WorkDoneProgress::Report(WorkDoneProgressReport {
            cancellable: Some(false),
            message: Some(format!("Indexed {indexed} files")),
            percentage: None,
        }));
        self.notify_index_progress(WorkDoneProgress::End(WorkDoneProgressEnd {
            message: Some(format!("Indexed {indexed} Djot files")),
        }));
    }

    fn notify_index_progress(&self, progress: WorkDoneProgress) {
        let _ = self
            .client
            .notify::<lsp_types::notification::Progress>(ProgressParams {
                token: NumberOrString::String("djot-ls-index".to_string()),
                value: ProgressParamsValue::WorkDone(progress),
            });
    }

    fn publish_open_document_diagnostics(&self) {
        for path in &self.open_documents {
            self.publish_diagnostics_for(path);
        }
    }

    fn publish_diagnostics_for(&self, path: &Path) {
        let Some(entry) = self.workspace.get(path) else {
            return;
        };
        let Some(uri) = Url::from_file_path(path).ok() else {
            return;
        };
        let diagnostics = self
            .workspace
            .diagnostics_for(path)
            .into_iter()
            .map(|diagnostic| to_lsp_diagnostic(&entry.text, &uri, diagnostic))
            .collect();

        let _ = self
            .client
            .notify::<lsp_types::notification::PublishDiagnostics>(PublishDiagnosticsParams {
                uri,
                diagnostics,
                version: None,
            });
    }

    fn clear_diagnostics_for(&self, path: &Path) {
        let Some(uri) = Url::from_file_path(path).ok() else {
            return;
        };
        let _ = self
            .client
            .notify::<lsp_types::notification::PublishDiagnostics>(PublishDiagnosticsParams {
                uri,
                diagnostics: Vec::new(),
                version: None,
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
            Some(id) => entry.index.anchors.get(id)?.range.clone(),
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
            let anchor = entry.index.anchors.get(&target_id)?;
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
        let edits = self.workspace.rename_edits(target_path, target_id);
        if edits.is_empty() {
            return Ok(None);
        }

        let mut changes: HashMap<Url, Vec<TextEdit>> = HashMap::new();
        for edit in edits {
            let Some(entry) = self.workspace.get(&edit.path) else {
                return Ok(None);
            };
            let Some(uri) = Url::from_file_path(&edit.path).ok() else {
                return Ok(None);
            };
            changes.entry(uri).or_default().push(TextEdit::new(
                byte_range_to_lsp(&entry.text, &edit.range),
                new_name.clone(),
            ));
        }

        Ok(Some(WorkspaceEdit::new(changes)))
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

        let old_uri = Url::from_file_path(&target.old_path)
            .ok()
            .ok_or_else(invalid_rename_path_error)?;
        let new_uri = Url::from_file_path(&new_path)
            .ok()
            .ok_or_else(invalid_rename_path_error)?;
        let mut operations = vec![DocumentChangeOperation::Op(ResourceOp::Rename(
            RenameFile {
                old_uri,
                new_uri,
                options: Some(RenameFileOptions {
                    overwrite: Some(false),
                    ignore_if_exists: Some(false),
                }),
                annotation_id: None,
            },
        ))];

        let mut edits_by_path: BTreeMap<PathBuf, Vec<TextEdit>> = BTreeMap::new();
        for edit in self
            .workspace
            .path_rename_edits(&target.old_path, &new_path)
        {
            let Some(entry) = self.workspace.get(&edit.source_path) else {
                return Ok(None);
            };
            edits_by_path
                .entry(edit.source_path)
                .or_default()
                .push(TextEdit::new(
                    byte_range_to_lsp(&entry.text, &edit.range),
                    edit.replacement,
                ));
        }

        for (path, edits) in edits_by_path {
            let Some(uri) = Url::from_file_path(&path).ok() else {
                return Ok(None);
            };
            operations.push(DocumentChangeOperation::Edit(TextDocumentEdit {
                text_document: OptionalVersionedTextDocumentIdentifier { uri, version: None },
                edits: edits.into_iter().map(OneOf::Left).collect(),
            }));
        }

        if let Some(entry) = self.workspace.get(&target.old_path) {
            let text = entry.text.clone();
            self.workspace.insert(new_path, text);
            self.workspace.remove(&target.old_path);
        }

        Ok(Some(WorkspaceEdit {
            changes: None,
            document_changes: Some(DocumentChanges::Operations(operations)),
            change_annotations: None,
        }))
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

        if let Some((id, anchor)) = self.workspace.anchor_at(&from, offset) {
            let entry = self.workspace.get(&from)?;
            return Some(Hover {
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
            });
        }

        let (target, source_range) = {
            let reference = self.workspace.reference_at(&from, offset)?;
            (
                resolve_target(&from, &reference.target)?,
                reference.source.clone(),
            )
        };

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
                let anchor = entry.index.anchors.get(id)?;
                anchor_hover_markdown(
                    self.display_path(&target.path),
                    id,
                    &entry.text,
                    &anchor.range,
                )
            }
            None => file_hover_markdown(self.display_path(&target.path), &entry.text),
        };

        Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value,
            }),
            range: Some(source_lsp_range),
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
        let mut actions = Vec::new();

        if requested_code_action_kind_matches(
            params.context.only.as_deref(),
            &CodeActionKind::REFACTOR_REWRITE,
        ) {
            if let Some(insertion) = metadata_insertion(&entry.text, offset, &path) {
                let range = byte_range_to_lsp(&entry.text, &insertion.insert);
                let edit = WorkspaceEdit::new(HashMap::from([(
                    params.text_document.uri.clone(),
                    vec![TextEdit::new(range, insertion.new_text)],
                )]));
                actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                    title: "Add metadata".to_string(),
                    kind: Some(CodeActionKind::REFACTOR_REWRITE),
                    diagnostics: None,
                    edit: Some(edit),
                    command: None,
                    is_preferred: Some(false),
                    disabled: None,
                    data: None,
                }));
            }

            if let Some(conversion) =
                task_list_item_conversion(&entry.text, offset, &created_timestamp())
            {
                let range = byte_range_to_lsp(&entry.text, &conversion.replace);
                let edit = WorkspaceEdit::new(HashMap::from([(
                    params.text_document.uri.clone(),
                    vec![TextEdit::new(range, conversion.replacement)],
                )]));
                actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                    title: "Convert to task div".to_string(),
                    kind: Some(CodeActionKind::REFACTOR_REWRITE),
                    diagnostics: None,
                    edit: Some(edit),
                    command: None,
                    is_preferred: Some(true),
                    disabled: None,
                    data: None,
                }));
            }
        }

        if requested_code_action_kind_matches(
            params.context.only.as_deref(),
            &CodeActionKind::QUICKFIX,
        ) {
            let timestamp = created_timestamp();
            let task_is_blocked = tasks(&entry.text)
                .into_iter()
                .find(|task| {
                    task.done.is_none()
                        && task.canceled.is_none()
                        && task.range.start <= offset
                        && offset <= task.range.end
                })
                .is_some_and(|task| self.workspace.is_task_blocked(&path, &task));
            if !task_is_blocked {
                if let Some(completion) =
                    recurring_task_status_edit(&entry.text, offset, "done", &timestamp)
                        .or_else(|| task_status_edit(&entry.text, offset, "done", &timestamp))
                {
                    let edits = completion
                        .edits
                        .into_iter()
                        .map(|edit| {
                            TextEdit::new(
                                byte_range_to_lsp(&entry.text, &edit.range),
                                edit.new_text,
                            )
                        })
                        .collect();
                    let edit = WorkspaceEdit::new(HashMap::from([(
                        params.text_document.uri.clone(),
                        edits,
                    )]));
                    actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                        title: "Complete task".to_string(),
                        kind: Some(CodeActionKind::QUICKFIX),
                        diagnostics: None,
                        edit: Some(edit),
                        command: None,
                        is_preferred: Some(true),
                        disabled: None,
                        data: None,
                    }));
                }
            }

            if let Some(cancellation) =
                recurring_task_status_edit(&entry.text, offset, "canceled", &timestamp)
                    .or_else(|| task_status_edit(&entry.text, offset, "canceled", &timestamp))
            {
                let edits = cancellation
                    .edits
                    .into_iter()
                    .map(|edit| {
                        TextEdit::new(byte_range_to_lsp(&entry.text, &edit.range), edit.new_text)
                    })
                    .collect();
                let edit =
                    WorkspaceEdit::new(HashMap::from([(params.text_document.uri.clone(), edits)]));
                actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                    title: "Cancel task".to_string(),
                    kind: Some(CodeActionKind::QUICKFIX),
                    diagnostics: None,
                    edit: Some(edit),
                    command: None,
                    is_preferred: Some(false),
                    disabled: None,
                    data: None,
                }));
            }
        }

        Some(actions)
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

    fn display_path(&self, path: &Path) -> String {
        self.workspace_roots
            .iter()
            .find_map(|root| path.strip_prefix(root).ok())
            .unwrap_or(path)
            .display()
            .to_string()
    }
}

#[derive(Debug, Clone)]
struct LinkTargetCompletion {
    title: String,
    path: String,
}

#[derive(Debug, Clone)]
struct AnchorCompletion {
    id: String,
    path: String,
}

struct TaskListItemConversion {
    replace: ByteRange<usize>,
    replacement: String,
}

struct TaskCompletionEdit {
    edits: Vec<TaskTextEdit>,
}

struct TaskTextEdit {
    range: ByteRange<usize>,
    new_text: String,
}

struct MetadataInsertion {
    insert: ByteRange<usize>,
    new_text: String,
}

#[derive(Debug, PartialEq, Eq)]
enum LinkCompletionContext {
    Label {
        replace: ByteRange<usize>,
        query: String,
    },
    Destination {
        replace: ByteRange<usize>,
        query: String,
    },
    Anchor {
        path: String,
        replace: ByteRange<usize>,
        query: String,
    },
}

#[derive(Debug, Clone, Copy)]
enum LinkScanState {
    Text,
    Label { open: usize },
    AfterLabel,
    Destination { start: usize },
}

fn link_completion_context(text: &str, offset: usize) -> Option<LinkCompletionContext> {
    incomplete_link_completion_context(text, offset)
        .or_else(|| closed_link_anchor_completion_context(text, offset))
}

fn incomplete_link_completion_context(text: &str, offset: usize) -> Option<LinkCompletionContext> {
    let str_span = str_event_touching_cursor(text, offset)?;
    let prefix = &text[str_span.start..offset];
    let mut state = LinkScanState::Text;

    for (i, c) in prefix.char_indices() {
        let absolute = str_span.start + i;
        if is_escaped(prefix, i) {
            continue;
        }

        state = match state {
            LinkScanState::Text => {
                if c == '[' {
                    LinkScanState::Label { open: absolute }
                } else {
                    LinkScanState::Text
                }
            }
            LinkScanState::Label { open } => {
                if c == ']' {
                    LinkScanState::AfterLabel
                } else if c == '[' {
                    LinkScanState::Label { open: absolute }
                } else {
                    LinkScanState::Label { open }
                }
            }
            LinkScanState::AfterLabel => {
                if c == '(' {
                    LinkScanState::Destination {
                        start: absolute + c.len_utf8(),
                    }
                } else if c == '[' {
                    LinkScanState::Label { open: absolute }
                } else {
                    LinkScanState::Text
                }
            }
            LinkScanState::Destination { start } => {
                if c == ')' {
                    LinkScanState::Text
                } else {
                    LinkScanState::Destination { start }
                }
            }
        };
    }

    match state {
        LinkScanState::Label { open } => Some(LinkCompletionContext::Label {
            replace: open..label_completion_replace_end(text, offset, str_span.end),
            query: text[open + 1..offset].to_string(),
        }),
        LinkScanState::Destination { start } => {
            let query = &text[start..offset];
            if let Some((path, anchor_query)) = query.split_once('#') {
                Some(LinkCompletionContext::Anchor {
                    path: path.to_string(),
                    replace: start + path.len() + '#'.len_utf8()..offset,
                    query: anchor_query.to_string(),
                })
            } else {
                Some(LinkCompletionContext::Destination {
                    replace: start..offset,
                    query: query.to_string(),
                })
            }
        }
        LinkScanState::Text | LinkScanState::AfterLabel => None,
    }
}

fn closed_link_anchor_completion_context(
    text: &str,
    offset: usize,
) -> Option<LinkCompletionContext> {
    Parser::new(text)
        .into_offset_iter()
        .find_map(|(event, span)| match event {
            Event::End(Container::Link(dst, _)) if span.start <= offset && offset <= span.end => {
                closed_link_completion_from_end_span(text, span, dst.as_ref(), offset)
            }
            _ => None,
        })
}

fn closed_link_completion_from_end_span(
    text: &str,
    span: ByteRange<usize>,
    dst: &str,
    offset: usize,
) -> Option<LinkCompletionContext> {
    let syntax = &text[span.clone()];
    let dst_range = closed_link_destination_range(span.start, syntax, dst)?;
    let dst_start = dst_range.start;
    let dst_end = dst_range.end;

    if let Some(hash_in_dst) = dst.find('#') {
        let fragment_start = dst_start + hash_in_dst + '#'.len_utf8();
        if offset < fragment_start || offset > dst_end {
            return None;
        }

        return Some(LinkCompletionContext::Anchor {
            path: dst[..hash_in_dst].to_string(),
            replace: fragment_start..offset,
            query: text[fragment_start..offset].to_string(),
        });
    }

    if offset < dst_start || offset > dst_end {
        return None;
    }

    Some(LinkCompletionContext::Destination {
        replace: dst_start..offset,
        query: text[dst_start..offset].to_string(),
    })
}

fn closed_link_destination_range(
    span_start: usize,
    syntax: &str,
    dst: &str,
) -> Option<ByteRange<usize>> {
    if dst.is_empty() {
        let open = syntax.find('(')?;
        let close = syntax[open + '('.len_utf8()..].find(')')? + open + '('.len_utf8();
        if close == open + '('.len_utf8() {
            let cursor = span_start + close;
            return Some(cursor..cursor);
        }
    }

    let dst_in_syntax = syntax.find(dst)?;
    let dst_start = span_start + dst_in_syntax;
    Some(dst_start..dst_start + dst.len())
}

fn label_completion_replace_end(text: &str, offset: usize, limit: usize) -> usize {
    if offset < limit && text[offset..].starts_with(']') && !is_escaped(text, offset) {
        offset + ']'.len_utf8()
    } else {
        offset
    }
}

fn task_list_item_conversion(
    text: &str,
    offset: usize,
    created: &str,
) -> Option<TaskListItemConversion> {
    let (line_start, line_end) = line_bounds(text, offset)?;
    let line = text.get(line_start..line_end)?;
    let content = line.strip_suffix('\r').unwrap_or(line);
    let indent_len = content
        .char_indices()
        .find(|(_, c)| *c != ' ' && *c != '\t')
        .map(|(i, _)| i)
        .unwrap_or(content.len());
    let indent = &content[..indent_len];
    let rest = &content[indent_len..];
    let title = rest.strip_prefix("- [ ] ")?.trim();
    if title.is_empty() {
        return None;
    }

    Some(TaskListItemConversion {
        replace: line_start..line_end,
        replacement: format!(
            "{indent}- {{created=\"{created}\"}}\n{indent}  ::: task\n{indent}  {title}\n{indent}  :::"
        ),
    })
}

fn task_status_edit(
    text: &str,
    offset: usize,
    attribute: &str,
    timestamp: &str,
) -> Option<TaskCompletionEdit> {
    let task = tasks(text).into_iter().find(|task| {
        task.done.is_none()
            && task.canceled.is_none()
            && task.range.start <= offset
            && offset <= task.range.end
    })?;
    let opening = task_opening_fence(text, &task.range)?;

    Some(TaskCompletionEdit {
        edits: vec![TaskTextEdit {
            range: opening.attribute_insert.clone(),
            new_text: format!(
                "{}{{{attribute}=\"{timestamp}\"}}\n{}",
                opening.attribute_prefix, opening.fence_prefix
            ),
        }],
    })
}

fn recurring_task_status_edit(
    text: &str,
    offset: usize,
    attribute: &str,
    timestamp: &str,
) -> Option<TaskCompletionEdit> {
    let task = tasks(text).into_iter().find(|task| {
        task.done.is_none()
            && task.canceled.is_none()
            && task.range.start <= offset
            && offset <= task.range.end
    })?;
    let due = DateTime::parse_from_rfc3339(task.due.as_deref()?).ok()?;
    let recur = task.recur.as_deref()?;
    let next_due = next_recur_due(due, recur)?;
    let next_wait = task
        .wait
        .as_deref()
        .and_then(|wait| DateTime::parse_from_rfc3339(wait).ok())
        .and_then(|wait| next_recur_due(wait, recur));
    let opening = task_opening_fence(text, &task.range)?;
    let indent = opening.task_indent.as_str();

    let anchors = build_index(text).anchors;
    let mut reserved = HashSet::new();
    let current_id = match task.id.clone() {
        Some(id) => id,
        None => {
            let id = task_instance_id(&task.title, due, &anchors, &reserved)?;
            reserved.insert(id.clone());
            id
        }
    };
    let next_id = task_instance_id(&task.title, next_due, &anchors, &reserved)?;
    let next_insert = line_bounds(text, task.range.end)?.1;
    let recur = escape_attribute_value(recur);
    let next_due_text = next_due.to_rfc3339_opts(SecondsFormat::Secs, true);
    let next_wait_text = next_wait.map(|wait| wait.to_rfc3339_opts(SecondsFormat::Secs, true));
    let next_wait_attribute = next_wait_text
        .as_deref()
        .map(|wait| format!(" wait=\"{}\"", escape_attribute_value(wait)))
        .unwrap_or_default();
    let current_id_text = escape_attribute_value(&current_id);
    let current_id_attribute = anchor_attribute(&current_id);
    let next_id_attribute = anchor_attribute(&next_id);
    let div = inherited_task_source(text.get(task.range.clone())?, indent);
    let list_item = single_task_list_item_context(text, opening.line_start, task.range.end, indent);

    let mut status_text = String::new();
    let mut attribute_prefix = opening.attribute_prefix.as_str();
    if task.id.is_none() {
        status_text.push_str(&format!("{attribute_prefix}{current_id_attribute}\n"));
        attribute_prefix = opening.continued_attribute_prefix.as_str();
    }
    status_text.push_str(&format!(
        "{attribute_prefix}{{{attribute}=\"{timestamp}\"}}\n{}",
        opening.fence_prefix
    ));

    let next_edit = match list_item {
        Some(context) => TaskTextEdit {
            range: context.insert..context.insert,
            new_text: format!(
                "\n{list_indent}- {next_id_attribute}\n{indent}{{created=\"{timestamp}\" due=\"{next_due_text}\"{next_wait_attribute} recur=\"{recur}\" prev=\"#{current_id_text}\"}}\n{div}",
                list_indent = context.list_indent,
            ),
        },
        None => TaskTextEdit {
            range: next_insert..next_insert,
            new_text: format!(
                "\n\n{indent}{next_id_attribute}\n{indent}{{created=\"{timestamp}\" due=\"{next_due_text}\"{next_wait_attribute} recur=\"{recur}\" prev=\"#{current_id_text}\"}}\n{div}"
            ),
        },
    };

    Some(TaskCompletionEdit {
        edits: vec![
            TaskTextEdit {
                range: opening.attribute_insert,
                new_text: status_text,
            },
            next_edit,
        ],
    })
}

struct ListTaskContext<'a> {
    list_indent: &'a str,
    insert: usize,
}

fn single_task_list_item_context<'a>(
    text: &str,
    task_line_start: usize,
    task_range_end: usize,
    task_indent: &'a str,
) -> Option<ListTaskContext<'a>> {
    let list_indent = task_indent
        .strip_suffix("  ")
        .or_else(|| task_indent.strip_suffix('\t'))?;
    let list_start = containing_list_item_start(text, task_line_start, list_indent, task_indent)?;
    let list_end = list_item_end(text, list_start, list_indent)?;
    let task_end_line_offset = task_range_end.saturating_sub(1);
    if list_end != line_bounds(text, task_end_line_offset).map(|(_, end)| end)? {
        return None;
    }
    if has_indented_content_after(text, list_end, list_indent) {
        return None;
    }
    if count_task_fences(text.get(list_start..list_end)?) != 1 {
        return None;
    }

    Some(ListTaskContext {
        list_indent,
        insert: list_end,
    })
}

fn containing_list_item_start(
    text: &str,
    task_line_start: usize,
    list_indent: &str,
    task_indent: &str,
) -> Option<usize> {
    let (_, current_line_end) = line_bounds(text, task_line_start)?;
    let current_line = text
        .get(task_line_start..current_line_end)?
        .strip_suffix('\r')
        .unwrap_or(text.get(task_line_start..current_line_end)?);
    let current_indent = leading_indent(current_line);
    let current_trimmed = current_line.trim_start();
    if current_indent == list_indent && current_trimmed.starts_with("- ") {
        return Some(task_line_start);
    }

    let mut line_start = previous_line_start(text, task_line_start)?;

    loop {
        let (_, line_end) = line_bounds(text, line_start)?;
        let line = text
            .get(line_start..line_end)?
            .strip_suffix('\r')
            .unwrap_or(text.get(line_start..line_end)?);
        let indent = leading_indent(line);
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            return None;
        }
        if indent == list_indent && trimmed.starts_with("- ") {
            return Some(line_start);
        }
        if indent != task_indent || !trimmed.starts_with('{') {
            return None;
        }
        line_start = previous_line_start(text, line_start)?;
    }
}

fn list_item_end(text: &str, list_start: usize, list_indent: &str) -> Option<usize> {
    let (_, mut line_end) = line_bounds(text, list_start)?;
    let mut next_start = next_line_start(text, line_end)?;

    while next_start < text.len() {
        let (_, next_end) = line_bounds(text, next_start)?;
        let line = text
            .get(next_start..next_end)?
            .strip_suffix('\r')
            .unwrap_or(text.get(next_start..next_end)?);
        let indent = leading_indent(line);
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            break;
        }
        if indent == list_indent && trimmed.starts_with("- ") {
            break;
        }
        if indent.len() <= list_indent.len() {
            break;
        }
        line_end = next_end;
        let Some(start) = next_line_start(text, next_end) else {
            break;
        };
        next_start = start;
    }

    Some(line_end)
}

fn has_indented_content_after(text: &str, line_end: usize, list_indent: &str) -> bool {
    let Some(mut line_start) = next_line_start(text, line_end) else {
        return false;
    };

    while line_start < text.len() {
        let Some((_, next_end)) = line_bounds(text, line_start) else {
            return false;
        };
        let Some(line) = text.get(line_start..next_end) else {
            return false;
        };
        let line = line.strip_suffix('\r').unwrap_or(line);
        let trimmed = line.trim_start();
        if !trimmed.is_empty() {
            let indent = leading_indent(line);
            return indent.len() > list_indent.len();
        }
        let Some(start) = next_line_start(text, next_end) else {
            return false;
        };
        line_start = start;
    }

    false
}

fn count_task_fences(text: &str) -> usize {
    text.lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            trimmed
                .strip_prefix("- ")
                .unwrap_or(trimmed)
                .starts_with("::: task")
        })
        .count()
}

fn previous_line_start(text: &str, line_start: usize) -> Option<usize> {
    if line_start == 0 {
        return None;
    }
    let previous_end = line_start.checked_sub('\n'.len_utf8())?;
    Some(text[..previous_end].rfind('\n').map_or(0, |i| i + 1))
}

fn next_line_start(text: &str, line_end: usize) -> Option<usize> {
    if line_end >= text.len() {
        None
    } else {
        Some(line_end + '\n'.len_utf8())
    }
}

fn ensure_block_indent(block: &str, indent: &str) -> String {
    if indent.is_empty() {
        return block.to_string();
    }

    let mut out = String::new();
    for line in block.split_inclusive('\n') {
        let content = line.trim_end_matches(['\r', '\n']);
        if content.is_empty() || line.starts_with(indent) {
            out.push_str(line);
        } else {
            out.push_str(indent);
            out.push_str(line);
        }
    }
    out
}

fn inherited_task_source(source: &str, indent: &str) -> String {
    filter_recurring_instance_attributes(&ensure_block_indent(source, indent))
}

fn filter_recurring_instance_attributes(source: &str) -> String {
    let mut out = String::new();
    for line in source.split_inclusive('\n') {
        match filter_recurring_attribute_line(line) {
            AttributeLineFilter::Keep(line) => out.push_str(line),
            AttributeLineFilter::Replace(line) => out.push_str(&line),
            AttributeLineFilter::Drop => {}
        }
    }
    out
}

enum AttributeLineFilter<'a> {
    Keep(&'a str),
    Replace(String),
    Drop,
}

fn filter_recurring_attribute_line(line: &str) -> AttributeLineFilter<'_> {
    let line_without_newline = line.trim_end_matches(['\r', '\n']);
    let newline = &line[line_without_newline.len()..];
    let indent = leading_indent(line_without_newline);
    let content = &line_without_newline[indent.len()..];
    let Some(inner) = content.strip_prefix('{').and_then(|s| s.strip_suffix('}')) else {
        return AttributeLineFilter::Keep(line);
    };

    let Some(tokens) = attribute_tokens(inner) else {
        return AttributeLineFilter::Keep(line);
    };
    if tokens.is_empty() {
        return AttributeLineFilter::Keep(line);
    }

    let kept = tokens
        .iter()
        .filter(|token| !is_recurring_instance_attribute(token))
        .collect::<Vec<_>>();
    if kept.len() == tokens.len() {
        return AttributeLineFilter::Keep(line);
    }
    if kept.is_empty() {
        return AttributeLineFilter::Drop;
    }

    let mut replacement = String::new();
    replacement.push_str(indent);
    replacement.push('{');
    for (idx, token) in kept.iter().enumerate() {
        if idx > 0 {
            replacement.push(' ');
        }
        replacement.push_str(token);
    }
    replacement.push('}');
    replacement.push_str(newline);
    AttributeLineFilter::Replace(replacement)
}

fn attribute_tokens(inner: &str) -> Option<Vec<&str>> {
    let mut tokens = Vec::new();
    let mut start = None;
    let mut quote = None;
    let mut escaped = false;

    for (idx, ch) in inner.char_indices() {
        if start.is_none() {
            if ch.is_whitespace() {
                continue;
            }
            start = Some(idx);
        }

        if let Some(quoted) = quote {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == quoted {
                quote = None;
            }
            continue;
        }

        if ch == '"' || ch == '\'' {
            quote = Some(ch);
        } else if ch.is_whitespace() {
            if let Some(token_start) = start.take() {
                tokens.push(inner[token_start..idx].trim());
            }
        }
    }

    if quote.is_some() {
        return None;
    }
    if let Some(token_start) = start {
        tokens.push(inner[token_start..].trim());
    }

    Some(
        tokens
            .into_iter()
            .filter(|token| !token.is_empty())
            .collect(),
    )
}

fn is_recurring_instance_attribute(token: &str) -> bool {
    if token.starts_with('#') {
        return true;
    }
    let key = token.split_once('=').map_or(token, |(key, _)| key);
    matches!(
        key,
        "id" | "created" | "done" | "canceled" | "due" | "wait" | "recur" | "prev"
    )
}

fn next_recur_due(due: DateTime<FixedOffset>, recur: &str) -> Option<DateTime<FixedOffset>> {
    let rule = parse_repeat_rule(recur)?;
    match rule {
        RepeatRule::Days(days) => Some(due + Duration::days(days)),
        RepeatRule::Weeks(weeks) => Some(due + Duration::weeks(weeks)),
        RepeatRule::Months(months) => add_months(due, months),
        RepeatRule::Years(years) => add_months(due, years.checked_mul(12)?),
    }
}

fn add_months(due: DateTime<FixedOffset>, months: i32) -> Option<DateTime<FixedOffset>> {
    let month0 = due.month0() as i32 + months;
    let year = due.year() + month0.div_euclid(12);
    let month0 = month0.rem_euclid(12);
    let month = (month0 + 1) as u32;
    let day = due.day().min(last_day_of_month(year, month)?);
    due.timezone()
        .with_ymd_and_hms(year, month, day, due.hour(), due.minute(), due.second())
        .single()
}

fn last_day_of_month(year: i32, month: u32) -> Option<u32> {
    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    let first_next = chrono::NaiveDate::from_ymd_opt(next_year, next_month, 1)?;
    Some((first_next - Duration::days(1)).day())
}

fn task_instance_id(
    title: &str,
    due: DateTime<FixedOffset>,
    anchors: &HashMap<String, djot_core::Anchor>,
    reserved: &HashSet<String>,
) -> Option<String> {
    let base = djot_heading_id(title)?;
    let date = due.format("%Y-%m-%d");
    let candidate = format!("{base}-{date}");
    Some(unique_anchor_id(candidate, anchors, reserved))
}

fn djot_heading_id(title: &str) -> Option<String> {
    let source = format!("# {}\n", title.trim());
    Parser::new(&source).find_map(|event| match event {
        Event::Start(Container::Heading { id, .. }, _) => Some(id.into_owned()),
        _ => None,
    })
}

fn unique_anchor_id(
    candidate: String,
    anchors: &HashMap<String, djot_core::Anchor>,
    reserved: &HashSet<String>,
) -> String {
    if !anchors.contains_key(&candidate) && !reserved.contains(&candidate) {
        return candidate;
    }
    let mut count = 2;
    loop {
        let id = format!("{candidate}-{count}");
        if !anchors.contains_key(&id) && !reserved.contains(&id) {
            return id;
        }
        count += 1;
    }
}

fn leading_indent(line: &str) -> &str {
    let indent_len = line
        .char_indices()
        .find(|(_, c)| *c != ' ' && *c != '\t')
        .map(|(i, _)| i)
        .unwrap_or(line.len());
    &line[..indent_len]
}

fn escape_attribute_value(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn anchor_attribute(id: &str) -> String {
    if is_shorthand_anchor_id(id) {
        format!("{{#{id}}}")
    } else {
        format!("{{id=\"{}\"}}", escape_attribute_value(id))
    }
}

fn is_shorthand_anchor_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'_' | b'-'))
}

fn metadata_insertion(text: &str, offset: usize, path: &Path) -> Option<MetadataInsertion> {
    if metadata_block(text).is_some() || !text.get(..offset)?.trim().is_empty() {
        return None;
    }

    Some(MetadataInsertion {
        insert: 0..0,
        new_text: format!(
            "{{.metadata}}\n``` toml\ntitle = \"{}\"\ncreated = \"{}\"\n```\n\n",
            escape_toml_string(&default_metadata_title(path)),
            created_timestamp()
        ),
    })
}

fn default_metadata_title(path: &Path) -> String {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.is_empty())
        .unwrap_or("Untitled")
        .to_string()
}

fn escape_toml_string(value: &str) -> String {
    let mut escaped = String::new();
    for c in value.chars() {
        match c {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            c if c.is_control() => {
                escaped.push_str(&format!("\\u{:04X}", c as u32));
            }
            c => escaped.push(c),
        }
    }
    escaped
}

struct TaskOpeningFence {
    line_start: usize,
    attribute_insert: ByteRange<usize>,
    attribute_prefix: String,
    continued_attribute_prefix: String,
    fence_prefix: String,
    task_indent: String,
}

fn task_opening_fence(text: &str, range: &ByteRange<usize>) -> Option<TaskOpeningFence> {
    let mut offset = range.start;
    while offset <= range.end {
        let (line_start, line_end) = line_bounds(text, offset)?;
        let line = text.get(line_start..line_end)?;
        if let Some(opening) = task_opening_fence_from_line(line_start, line) {
            return Some(opening);
        }
        if line_end >= range.end || line_end == text.len() {
            break;
        }
        offset = line_end + '\n'.len_utf8();
    }
    None
}

fn task_opening_fence_from_line(line_start: usize, line: &str) -> Option<TaskOpeningFence> {
    let line = line.strip_suffix('\r').unwrap_or(line);
    let indent = leading_indent(line);
    let rest = &line[indent.len()..];
    if rest.starts_with("::: task") {
        return Some(TaskOpeningFence {
            line_start,
            attribute_insert: line_start..line_start,
            attribute_prefix: indent.to_string(),
            continued_attribute_prefix: indent.to_string(),
            fence_prefix: String::new(),
            task_indent: indent.to_string(),
        });
    }

    let fence = rest.strip_prefix("- ")?;
    if !fence.starts_with("::: task") {
        return None;
    }
    Some(TaskOpeningFence {
        line_start,
        attribute_insert: line_start..line_start + indent.len() + "- ".len(),
        attribute_prefix: format!("{indent}- "),
        continued_attribute_prefix: format!("{indent}  "),
        fence_prefix: format!("{indent}  "),
        task_indent: format!("{indent}  "),
    })
}

fn line_bounds(text: &str, offset: usize) -> Option<(usize, usize)> {
    if offset > text.len() {
        return None;
    }
    let start = text[..offset].rfind('\n').map_or(0, |i| i + 1);
    let end = text[offset..].find('\n').map_or(text.len(), |i| offset + i);
    Some((start, end))
}

fn requested_code_action_kind_matches(
    only: Option<&[CodeActionKind]>,
    action_kind: &CodeActionKind,
) -> bool {
    let Some(only) = only else {
        return true;
    };
    only.iter()
        .any(|requested| code_action_kind_includes(requested, action_kind))
}

fn code_action_kind_includes(requested: &CodeActionKind, action_kind: &CodeActionKind) -> bool {
    let requested = requested.as_str();
    let action_kind = action_kind.as_str();
    action_kind == requested
        || action_kind
            .strip_prefix(requested)
            .is_some_and(|suffix| suffix.starts_with('.'))
}

fn created_timestamp() -> String {
    Local::now().to_rfc3339_opts(SecondsFormat::Secs, false)
}

fn str_event_touching_cursor(text: &str, offset: usize) -> Option<ByteRange<usize>> {
    let mut ignored_depth = 0usize;
    for (event, span) in Parser::new(text).into_offset_iter() {
        match event {
            Event::Start(container, _) => {
                if ignored_depth > 0 || ignores_completion_str(&container) {
                    ignored_depth += 1;
                }
            }
            Event::End(container) => {
                let _ = container;
                if ignored_depth > 0 {
                    ignored_depth -= 1;
                }
            }
            Event::Str(_) if ignored_depth == 0 && span.start <= offset && offset <= span.end => {
                return Some(span);
            }
            _ => {}
        }
    }
    None
}

fn ignores_completion_str(container: &Container<'_>) -> bool {
    matches!(
        container,
        Container::Verbatim
            | Container::CodeBlock { .. }
            | Container::Math { .. }
            | Container::RawInline { .. }
            | Container::RawBlock { .. }
            | Container::Link(_, _)
            | Container::Image(_, _)
    )
}

fn is_escaped(text: &str, byte_index: usize) -> bool {
    let mut backslashes = 0;
    for b in text[..byte_index].bytes().rev() {
        if b == b'\\' {
            backslashes += 1;
        } else {
            break;
        }
    }
    backslashes % 2 == 1
}

fn completion_item(
    label: String,
    detail: Option<String>,
    new_text: String,
    source_text: &str,
    replace: &ByteRange<usize>,
    kind: CompletionItemKind,
) -> CompletionItem {
    CompletionItem {
        label,
        kind: Some(kind),
        detail,
        text_edit: Some(CompletionTextEdit::Edit(TextEdit::new(
            byte_range_to_lsp(source_text, replace),
            new_text,
        ))),
        ..CompletionItem::default()
    }
}

fn is_valid_anchor_id(id: &str) -> bool {
    !id.is_empty() && !id.contains('#') && !id.chars().any(char::is_whitespace)
}

fn is_valid_link_path_rename(path: &str) -> bool {
    !path.is_empty()
        && !path.contains('#')
        && !path.contains("://")
        && !path.starts_with("mailto:")
        && Path::new(path).is_relative()
}

fn implicit_heading_rename_error() -> ResponseError {
    ResponseError::new(
        ErrorCode::INVALID_REQUEST,
        "Renaming implicit heading anchors is not supported yet; add an explicit {#id} anchor or rename the heading text.",
    )
}

fn document_changes_capability_error() -> ResponseError {
    ResponseError::new(
        ErrorCode::INVALID_REQUEST,
        "Renaming link paths requires client support for workspace.workspaceEdit.documentChanges.",
    )
}

fn rename_resource_operation_capability_error() -> ResponseError {
    ResponseError::new(
        ErrorCode::INVALID_REQUEST,
        "Renaming link paths requires client support for the workspace.workspaceEdit.resourceOperations rename operation.",
    )
}

fn invalid_rename_path_error() -> ResponseError {
    ResponseError::new(
        ErrorCode::INVALID_PARAMS,
        "Rename path must be a relative Djot file path without a fragment.",
    )
}

fn non_djot_path_rename_error() -> ResponseError {
    ResponseError::new(
        ErrorCode::INVALID_REQUEST,
        "Only Djot file links can be renamed.",
    )
}

fn unindexed_path_rename_error() -> ResponseError {
    ResponseError::new(
        ErrorCode::INVALID_REQUEST,
        "Cannot rename a link path whose target is not indexed in the workspace.",
    )
}

fn rename_target_exists_error() -> ResponseError {
    ResponseError::new(
        ErrorCode::INVALID_REQUEST,
        "Cannot rename link path because the target path already exists.",
    )
}

fn rename_target_outside_workspace_error() -> ResponseError {
    ResponseError::new(
        ErrorCode::INVALID_REQUEST,
        "Cannot rename link path outside the workspace.",
    )
}

fn to_lsp_diagnostic(text: &str, uri: &Url, diagnostic: AnalysisDiagnostic) -> Diagnostic {
    let (code, message, related_information, severity) = match diagnostic.kind {
        DiagnosticKind::UnresolvedAnchor { id } => (
            "unresolved-anchor",
            format!("Unresolved anchor `{}`", id),
            None,
            DiagnosticSeverity::WARNING,
        ),
        DiagnosticKind::UnresolvedPath { path } => (
            "unresolved-path",
            format!("Unresolved Djot path `{}`", path),
            None,
            DiagnosticSeverity::WARNING,
        ),
        DiagnosticKind::DuplicateAnchor { id, first_range } => (
            "duplicate-anchor",
            format!("Duplicate anchor `{}`", id),
            Some(vec![DiagnosticRelatedInformation {
                location: Location::new(uri.clone(), byte_range_to_lsp(text, &first_range)),
                message: "First definition is here.".to_string(),
            }]),
            DiagnosticSeverity::WARNING,
        ),
        DiagnosticKind::MissingTaskDueForRecur => (
            "missing-task-due-for-recur",
            "Recurring tasks with `recur` need a valid RFC 3339 `due` datetime.".to_string(),
            None,
            DiagnosticSeverity::WARNING,
        ),
        DiagnosticKind::InvalidTaskRecur { recur } => (
            "invalid-task-recur",
            format!(
                "Unsupported task `recur` value `{}`. Use an ISO 8601 duration like `P1D`, `P1W`, `P1M`, or `P1Y`.",
                recur
            ),
            None,
            DiagnosticSeverity::WARNING,
        ),
        DiagnosticKind::ConflictingTaskClosedState => (
            "conflicting-task-closed-state",
            "Task cannot have both `done` and `canceled`.".to_string(),
            None,
            DiagnosticSeverity::WARNING,
        ),
        DiagnosticKind::InvalidTaskPrevTarget { id } => (
            "invalid-task-prev-target",
            format!("Task `prev` target `{}` must be a task.", id),
            None,
            DiagnosticSeverity::WARNING,
        ),
        DiagnosticKind::InvalidTaskDependencyTarget { target } => (
            "invalid-task-dependency-target",
            format!("Task dependency target `{}` must be a task.", target),
            None,
            DiagnosticSeverity::WARNING,
        ),
        DiagnosticKind::TaskSelfDependency { target } => (
            "task-self-dependency",
            format!("Task cannot depend on itself via `{}`.", target),
            None,
            DiagnosticSeverity::WARNING,
        ),
        DiagnosticKind::TaskDependencyCycle { id } => (
            "task-dependency-cycle",
            format!("Task dependency cycle includes `{}`.", id),
            None,
            DiagnosticSeverity::WARNING,
        ),
        DiagnosticKind::TaskBlocked { count } => (
            "task-blocked",
            match count {
                1 => "Blocked by 1 open dependency.".to_string(),
                _ => format!("Blocked by {count} open dependencies."),
            },
            None,
            DiagnosticSeverity::HINT,
        ),
    };

    Diagnostic {
        range: byte_range_to_lsp(text, &diagnostic.range),
        severity: Some(severity),
        code: Some(NumberOrString::String(code.to_string())),
        code_description: None,
        source: Some("djot-ls".to_string()),
        message,
        related_information,
        tags: None,
        data: None,
    }
}

fn document_title(text: &str) -> Option<String> {
    let metadata = metadata_block(text)?;
    let value: toml::Value = toml::from_str(&metadata).ok()?;
    value
        .get("title")
        .and_then(|title| title.as_str())
        .map(str::to_string)
}

fn relative_link_path(from: &Path, target: &Path) -> Option<String> {
    let base = from.parent()?;
    Some(relative_path(base, target)?.display().to_string())
}

fn relative_path(base: &Path, target: &Path) -> Option<PathBuf> {
    let base_components = lexical_components(base)?;
    let target_components = lexical_components(target)?;

    if base_components.first() != target_components.first() {
        return None;
    }

    let common_len = base_components
        .iter()
        .zip(target_components.iter())
        .take_while(|(base, target)| base == target)
        .count();

    let mut out = PathBuf::new();
    for _ in common_len..base_components.len() {
        out.push("..");
    }
    for component in &target_components[common_len..] {
        out.push(component);
    }

    Some(if out.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        out
    })
}

fn lexical_components(path: &Path) -> Option<Vec<OsString>> {
    let mut out = Vec::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop()?;
            }
            Component::Normal(part) => out.push(part.to_os_string()),
            Component::RootDir => out.push(OsString::from(std::path::MAIN_SEPARATOR.to_string())),
            Component::Prefix(prefix) => out.push(prefix.as_os_str().to_os_string()),
        }
    }
    Some(out)
}

fn fuzzy_match(query: &str, candidate: &str) -> bool {
    if query.is_empty() {
        return true;
    }

    let mut chars = query.chars().flat_map(char::to_lowercase);
    let Some(mut needle) = chars.next() else {
        return true;
    };

    for c in candidate.chars().flat_map(char::to_lowercase) {
        if c == needle {
            if let Some(next) = chars.next() {
                needle = next;
            } else {
                return true;
            }
        }
    }
    false
}

fn escape_link_label(value: &str) -> String {
    value.replace('\\', "\\\\").replace(']', "\\]")
}

fn escape_link_destination(value: &str) -> String {
    value.replace('\\', "\\\\").replace(')', "\\)")
}

fn anchor_hover_markdown(
    display_path: String,
    id: &str,
    text: &str,
    range: &std::ops::Range<usize>,
) -> String {
    let kind = if text[range.clone()].trim_start().starts_with('#') {
        "Heading"
    } else {
        "Anchor"
    };
    let line = offset_to_position(text, range.start).line + 1;
    let preview = preview_from_offset(text, range.start, 5);
    format!(
        "**{kind}** `{}`\n\n`{}:{line}`\n\n```djot\n{}\n```",
        escape_markdown_code(id),
        escape_markdown_code(&display_path),
        preview
    )
}

fn file_hover_markdown(display_path: String, text: &str) -> String {
    let (line, offset) = first_preview_offset(text);
    let preview = preview_from_offset(text, offset, 5);
    if preview.is_empty() {
        format!(
            "**File**\n\n`{}:{line}`",
            escape_markdown_code(&display_path)
        )
    } else {
        format!(
            "**File**\n\n`{}:{line}`\n\n```djot\n{}\n```",
            escape_markdown_code(&display_path),
            preview
        )
    }
}

fn first_preview_offset(text: &str) -> (usize, usize) {
    text.lines()
        .scan(0usize, |offset, line| {
            let current = *offset;
            *offset += line.len() + 1;
            Some((current, line))
        })
        .enumerate()
        .find(|(_, (_, line))| !line.trim().is_empty())
        .map(|(line, (offset, _))| (line + 1, offset))
        .unwrap_or((1, 0))
}

fn preview_from_offset(text: &str, offset: usize, max_lines: usize) -> String {
    let start = text[..offset].rfind('\n').map_or(0, |i| i + 1);
    text[start..]
        .lines()
        .take(max_lines)
        .collect::<Vec<_>>()
        .join("\n")
        .trim_end()
        .to_string()
}

fn escape_markdown_code(value: &str) -> String {
    value.replace('`', "\\`")
}

/// Convert a core [`Heading`] (byte offsets) into an LSP `DocumentSymbol`.
fn to_document_symbol(text: &str, heading: &Heading) -> DocumentSymbol {
    let children: Vec<_> = heading
        .children
        .iter()
        .map(|child| to_document_symbol(text, child))
        .collect();
    #[allow(deprecated)]
    DocumentSymbol {
        name: if heading.name.is_empty() {
            format!("H{}", heading.level)
        } else {
            heading.name.clone()
        },
        detail: Some(format!("H{}", heading.level)),
        kind: SymbolKind::STRING,
        tags: None,
        deprecated: None,
        range: byte_range_to_lsp(text, &heading.range),
        selection_range: byte_range_to_lsp(text, &heading.selection_range),
        children: if children.is_empty() {
            None
        } else {
            Some(children)
        },
    }
}

/// Convert a byte range into an LSP `Range`.
fn byte_range_to_lsp(text: &str, range: &std::ops::Range<usize>) -> Range {
    Range {
        start: offset_to_position(text, range.start),
        end: offset_to_position(text, range.end),
    }
}

/// Convert an LSP `Position` (line + UTF-16 column) into a byte offset.
fn position_to_offset(text: &str, pos: Position) -> usize {
    let mut line = 0u32;
    let mut character = 0u32;
    for (i, c) in text.char_indices() {
        if line == pos.line && character == pos.character {
            return i;
        }
        if c == '\n' {
            if line == pos.line {
                return i; // position is past the line's end: clamp to line end
            }
            line += 1;
            character = 0;
        } else {
            character += c.len_utf16() as u32;
        }
    }
    text.len()
}

/// Convert a byte offset into an LSP `Position` (line + UTF-16 column).
fn offset_to_position(text: &str, offset: usize) -> Position {
    let mut line = 0u32;
    let mut character = 0u32;
    for (i, c) in text.char_indices() {
        if i >= offset {
            break;
        }
        if c == '\n' {
            line += 1;
            character = 0;
        } else {
            character += c.len_utf16() as u32;
        }
    }
    Position { line, character }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let (server, _) = async_lsp::MainLoop::new_server(|client| {
        ServiceBuilder::new()
            .layer(TracingLayer::default())
            .layer(LifecycleLayer::default())
            .layer(CatchUnwindLayer::default())
            .layer(ConcurrencyLayer::default())
            .layer(ClientProcessMonitorLayer::new(client.clone()))
            .service(Router::from_language_server(ServerState {
                client,
                workspace: Workspace::new(),
                workspace_roots: Vec::new(),
                workspace_edit_capabilities: ClientWorkspaceEditCapabilities::default(),
                open_documents: HashSet::new(),
            }))
    });

    tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .with_ansi(false)
        .with_writer(std::io::stderr)
        .init();

    // Prefer truly asynchronous piped stdin/stdout without blocking tasks.
    let stdin = async_lsp::stdio::PipeStdin::lock_tokio().unwrap();
    let stdout = async_lsp::stdio::PipeStdout::lock_tokio().unwrap();

    server.run_buffered(stdin, stdout).await.unwrap();
}

fn workspace_roots(params: &InitializeParams) -> Vec<PathBuf> {
    if let Some(folders) = &params.workspace_folders {
        folders
            .iter()
            .filter_map(|folder| folder.uri.to_file_path().ok())
            .collect()
    } else {
        #[allow(deprecated)]
        params
            .root_uri
            .as_ref()
            .and_then(|uri| uri.to_file_path().ok())
            .into_iter()
            .collect()
    }
}

fn client_workspace_edit_capabilities(
    params: &InitializeParams,
) -> ClientWorkspaceEditCapabilities {
    let Some(workspace_edit) = params
        .capabilities
        .workspace
        .as_ref()
        .and_then(|workspace| workspace.workspace_edit.as_ref())
    else {
        return ClientWorkspaceEditCapabilities::default();
    };

    ClientWorkspaceEditCapabilities {
        document_changes: workspace_edit.document_changes == Some(true),
        rename_resource_operation: workspace_edit
            .resource_operations
            .as_ref()
            .is_some_and(|operations| operations.contains(&ResourceOperationKind::Rename)),
    }
}

fn index_djot_files(root: &Path, insert: &mut impl FnMut(PathBuf, String)) -> usize {
    let Ok(entries) = std::fs::read_dir(root) else {
        return 0;
    };

    let mut indexed = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };

        if file_type.is_dir() {
            indexed += index_djot_files(&path, insert);
        } else if file_type.is_file() && is_djot_file(&path) {
            if let Ok(text) = std::fs::read_to_string(&path) {
                insert(path, text);
                indexed += 1;
            }
        }
    }
    indexed
}

fn is_djot_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext == "dj" || ext == "djot")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recur_due_supports_iso_week_duration() {
        let due = DateTime::parse_from_rfc3339("2026-06-21T17:00:00+08:00").unwrap();
        let next = next_recur_due(due, "P1W").unwrap();

        assert_eq!(next.to_rfc3339(), "2026-06-28T17:00:00+08:00");
    }

    #[test]
    fn recur_due_adds_calendar_months() {
        let due = DateTime::parse_from_rfc3339("2026-01-31T17:00:00+08:00").unwrap();
        let next = next_recur_due(due, "P1M").unwrap();

        assert_eq!(next.to_rfc3339(), "2026-02-28T17:00:00+08:00");
    }

    #[test]
    fn recur_due_adds_calendar_years() {
        let due = DateTime::parse_from_rfc3339("2024-02-29T17:00:00+08:00").unwrap();
        let next = next_recur_due(due, "P1Y").unwrap();

        assert_eq!(next.to_rfc3339(), "2025-02-28T17:00:00+08:00");
    }

    #[test]
    fn recur_due_rejects_composite_and_time_durations() {
        let due = DateTime::parse_from_rfc3339("2026-06-21T17:00:00+08:00").unwrap();

        assert!(next_recur_due(due, "P1M1D").is_none());
        assert!(next_recur_due(due, "PT1H").is_none());
        assert!(next_recur_due(due, "weekly").is_none());
    }

    #[test]
    fn anchor_attribute_uses_shorthand_only_for_ascii_name_ids() {
        assert_eq!(
            anchor_attribute("daily-review-2026-06-22"),
            "{#daily-review-2026-06-22}"
        );
        assert_eq!(
            anchor_attribute("学习-anki-2026-06-22"),
            "{id=\"学习-anki-2026-06-22\"}"
        );
        assert_eq!(
            anchor_attribute("quote\"backslash\\"),
            "{id=\"quote\\\"backslash\\\\\"}"
        );
    }

    #[test]
    fn recurring_attribute_filter_drops_instance_attribute_lines() {
        let source = "  {#task created=\"2026-06-21T00:00:00Z\" due=\"2026-06-22T00:00:00Z\" wait=\"2026-06-21T20:00:00Z\" recur=\"P1D\" done=\"2026-06-21T12:00:00Z\" canceled=\"2026-06-21T13:00:00Z\" prev=\"#old\"}\n  ::: task\n  Title\n  :::\n";

        assert_eq!(
            filter_recurring_instance_attributes(source),
            "  ::: task\n  Title\n  :::\n"
        );
    }

    #[test]
    fn recurring_attribute_filter_keeps_unknown_attribute_lines_verbatim() {
        let source = "  {project=\"anki\" priority=\"high\" .work}\n  ::: task\n  Title\n  :::\n";

        assert_eq!(filter_recurring_instance_attributes(source), source);
    }

    #[test]
    fn recurring_attribute_filter_rebuilds_mixed_attribute_lines() {
        let source = "  {project=\"anki cards\" recur=\"P1D\" priority=\"high\" #old}\n";

        assert_eq!(
            filter_recurring_instance_attributes(source),
            "  {project=\"anki cards\" priority=\"high\"}\n"
        );
    }

    #[test]
    fn recurring_attribute_filter_handles_quoted_spaces_and_escapes() {
        let source =
            "  {note=\"keep \\\"quoted\\\" value\" due=\"2026-06-22T00:00:00Z\" tag='two words'}\n";

        assert_eq!(
            filter_recurring_instance_attributes(source),
            "  {note=\"keep \\\"quoted\\\" value\" tag='two words'}\n"
        );
    }
}

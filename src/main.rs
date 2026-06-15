use std::collections::HashMap;
use std::ops::ControlFlow;

use async_lsp::client_monitor::ClientProcessMonitorLayer;
use async_lsp::concurrency::ConcurrencyLayer;
use async_lsp::panic::CatchUnwindLayer;
use async_lsp::router::Router;
use async_lsp::server::LifecycleLayer;
use async_lsp::tracing::TracingLayer;
use async_lsp::{ClientSocket, LanguageServer, ResponseError};
use futures::future::BoxFuture;
use jotdown::{Container, Event, Parser};
use lsp_types::{
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    DocumentSymbol, DocumentSymbolParams, DocumentSymbolResponse, InitializeParams,
    InitializeResult, OneOf, Position, Range, ServerCapabilities, SymbolKind,
    TextDocumentSyncCapability, TextDocumentSyncKind, Url,
};
use tower::ServiceBuilder;
use tracing::Level;

/// Server state. async-lsp's omni-trait hands us `&mut self` on every request and
/// notification, so plain owned state needs no locking.
struct ServerState {
    #[allow(dead_code)]
    client: ClientSocket,
    /// Full text of every open document, keyed by URI.
    documents: HashMap<Url, String>,
}

impl LanguageServer for ServerState {
    type Error = ResponseError;
    type NotifyResult = ControlFlow<async_lsp::Result<()>>;

    fn initialize(
        &mut self,
        _params: InitializeParams,
    ) -> BoxFuture<'static, Result<InitializeResult, Self::Error>> {
        Box::pin(async move {
            Ok(InitializeResult {
                capabilities: ServerCapabilities {
                    // Full-document sync keeps things simple for now.
                    text_document_sync: Some(TextDocumentSyncCapability::Kind(
                        TextDocumentSyncKind::FULL,
                    )),
                    document_symbol_provider: Some(OneOf::Left(true)),
                    ..ServerCapabilities::default()
                },
                server_info: None,
            })
        })
    }

    fn did_open(&mut self, params: DidOpenTextDocumentParams) -> Self::NotifyResult {
        let doc = params.text_document;
        self.documents.insert(doc.uri, doc.text);
        ControlFlow::Continue(())
    }

    fn did_change(&mut self, params: DidChangeTextDocumentParams) -> Self::NotifyResult {
        // FULL sync: the last change contains the entire document.
        if let Some(change) = params.content_changes.into_iter().last() {
            self.documents.insert(params.text_document.uri, change.text);
        }
        ControlFlow::Continue(())
    }

    fn did_close(&mut self, params: DidCloseTextDocumentParams) -> Self::NotifyResult {
        self.documents.remove(&params.text_document.uri);
        ControlFlow::Continue(())
    }

    fn document_symbol(
        &mut self,
        params: DocumentSymbolParams,
    ) -> BoxFuture<'static, Result<Option<DocumentSymbolResponse>, Self::Error>> {
        let symbols = self
            .documents
            .get(&params.text_document.uri)
            .map(|text| heading_symbols(text));
        Box::pin(async move { Ok(symbols.map(DocumentSymbolResponse::Nested)) })
    }
}

/// Extract one flat `DocumentSymbol` per heading in the document.
fn heading_symbols(text: &str) -> Vec<DocumentSymbol> {
    let mut symbols = Vec::new();
    // Frame for the heading we are currently inside: (start byte, accumulated name).
    let mut current: Option<(usize, String)> = None;

    for (event, span) in Parser::new(text).into_offset_iter() {
        match event {
            Event::Start(Container::Heading { .. }, _) => {
                current = Some((span.start, String::new()));
            }
            Event::Str(s) => {
                if let Some((_, name)) = current.as_mut() {
                    name.push_str(&s);
                }
            }
            Event::End(Container::Heading { level, .. }) => {
                if let Some((start, name)) = current.take() {
                    let range = Range {
                        start: offset_to_position(text, start),
                        end: offset_to_position(text, span.end),
                    };
                    #[allow(deprecated)]
                    symbols.push(DocumentSymbol {
                        name: if name.is_empty() {
                            format!("H{level}")
                        } else {
                            name
                        },
                        detail: Some(format!("H{level}")),
                        kind: SymbolKind::STRING,
                        tags: None,
                        deprecated: None,
                        range,
                        selection_range: range,
                        children: None,
                    });
                }
            }
            _ => {}
        }
    }

    symbols
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
                documents: HashMap::new(),
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

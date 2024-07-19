use dashmap::DashMap;
use ropey::Rope;
use tokio;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};

mod adapter;
mod ast;
mod document_symbol;
mod utils;

#[derive(Debug)]
struct Backend {
    client: Client,
    doc_and_ast_map: DashMap<String, (Rope, Vec<ast::Node>)>,
}

impl Backend {
    async fn on_change(&self, params: TextDocumentItem) {
        let rope = ropey::Rope::from_str(&params.text);

        let mut events = jotdown::Parser::new(&params.text).into_offset_iter();
        let nodes = {
            let mut res = Vec::new();
            while let Some(offset_e) = events.next() {
                res.push(ast::Node::new(&offset_e, &mut events));
            }
            res
        };
        self.doc_and_ast_map
            .insert(params.uri.to_string(), (rope, nodes));
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        self.client
            .log_message(MessageType::INFO, "file opened!")
            .await;
        self.on_change(TextDocumentItem {
            uri: params.text_document.uri,
            language_id: params.text_document.language_id,
            text: params.text_document.text,
            version: params.text_document.version,
        })
        .await
    }

    async fn did_change(&self, mut params: DidChangeTextDocumentParams) {
        self.on_change(TextDocumentItem {
            uri: params.text_document.uri,
            language_id: String::from("djot"),
            text: std::mem::take(&mut params.content_changes[0].text),
            version: params.text_document.version,
        })
        .await
    }

    async fn initialize(&self, _: InitializeParams) -> Result<InitializeResult> {
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                document_symbol_provider: Some(OneOf::Left(true)),
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                ..Default::default()
            },
            ..Default::default()
        })
    }

    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> Result<Option<DocumentSymbolResponse>> {
        self.client
            .log_message(MessageType::INFO, format!("{:?}", self.doc_and_ast_map))
            .await;

        let tns = self
            .doc_and_ast_map
            .get(&params.text_document.uri.to_string())
            .unwrap();
        let text = &tns.0;
        let nodes = &tns.1;
        let symbols: Vec<DocumentSymbol> = nodes
            .into_iter()
            .filter_map(|child| document_symbol::find_document_heading(text, child))
            .collect();
        Ok(Some(DocumentSymbolResponse::Nested(symbols)))
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "server initialized!")
            .await;
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}

#[tokio::main]
async fn main() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(|client| Backend {
        client,
        doc_and_ast_map: DashMap::new(),
    });
    Server::new(stdin, stdout, socket).serve(service).await;
}

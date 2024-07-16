use dashmap::DashMap;
use ropey::Rope;
use tokio;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};
use tree_sitter::{Parser, Tree};

mod document_symbol;
mod utils;

#[derive(Debug)]
struct Backend {
    client: Client,
    ast_map: DashMap<String, Tree>,
    document_map: DashMap<String, Rope>,
}

impl Backend {
    async fn on_change(&self, params: TextDocumentItem) {
        let rope = ropey::Rope::from_str(&params.text);
        self.document_map
            .insert(params.uri.to_string(), rope.clone());
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_djot::language())
            .expect("Error loading djot grammer");
        let tree = parser.parse(rope.to_string(), None).unwrap();
        self.ast_map.insert(params.uri.to_string(), tree);
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
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                completion_provider: Some(CompletionOptions::default()),
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
            .log_message(MessageType::INFO, format!("{:?}", self.ast_map))
            .await;

        let tree = self
            .ast_map
            .get(&params.text_document.uri.to_string())
            .unwrap();
        let text = self
            .document_map
            .get(&params.text_document.uri.to_string())
            .unwrap();
        let mut cursor = tree.root_node().walk();
        let symbols: Vec<DocumentSymbol> = tree
            .root_node()
            .children(&mut cursor)
            .filter_map(|child| document_symbol::find_document_heading(child, &text))
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

    async fn completion(&self, _: CompletionParams) -> Result<Option<CompletionResponse>> {
        Ok(Some(CompletionResponse::Array(vec![
            CompletionItem::new_simple("Hello".to_string(), "Some detail".to_string()),
            CompletionItem::new_simple("Bye".to_string(), "More detail".to_string()),
        ])))
    }

    async fn hover(&self, _: HoverParams) -> Result<Option<Hover>> {
        Ok(Some(Hover {
            contents: HoverContents::Scalar(MarkedString::String("You're hovering!".to_string())),
            range: None,
        }))
    }
}

#[tokio::main]
async fn main() {
    // let mut parser = Parser::new();
    // parser
    //     .set_language(&tree_sitter_djot::language())
    //     .expect("Error loading djot grammer");
    // let source_code = std::fs::read_to_string("a.dj").unwrap();
    // // "# Heading\n\nsomethind\n\n## Heading ne\n\n114514\n\n# Heading 2\n\n114514\n";
    // let rope = Rope::from_str(&source_code);
    // let tree = parser.parse(source_code, None).unwrap();
    // println!("{}", tree.root_node().to_sexp());
    // let mut cursor = tree.root_node().walk();
    // let hs: Vec<DocumentSymbol> = tree
    //     .root_node()
    //     .children(&mut cursor)
    //     .filter_map(|child| find_document_heading(child, &rope))
    //     .collect();
    // println!("{:?}", hs);
    // std::process::exit(0);
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(|client| Backend {
        client,
        ast_map: DashMap::new(),
        document_map: DashMap::new(),
    });
    Server::new(stdin, stdout, socket).serve(service).await;
}

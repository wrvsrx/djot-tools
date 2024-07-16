use super::utils;
use tower_lsp::lsp_types::{DocumentSymbol, SymbolKind};

fn render_heading(text: &str) -> String {
    text.lines()
        .map(|s| {
            let mut i = 0;
            for c in s.chars() {
                if c == '#' || c == ' ' {
                    i += 1;
                } else {
                    break;
                }
            }
            &s[i..]
        })
        .collect::<Vec<&str>>()
        .join(" ")
}

pub fn find_document_heading(
    node: tree_sitter::Node,
    text: &ropey::Rope,
) -> Option<DocumentSymbol> {
    if node.kind() == "section" {
        let first_child = node.child(0).unwrap();

        // check if first_child.kind()'s prefix is "heading"
        assert!(first_child.kind().starts_with("heading"));
        assert_eq!(first_child.child(0).unwrap().kind(), "marker");

        let range = first_child.range();
        let heading_str = text
            .slice(text.byte_to_char(range.start_byte)..(text.byte_to_char(range.end_byte) - 1))
            .to_string();
        let heading_str = render_heading(&heading_str);

        let mut cursor = node.walk();
        let b: Vec<DocumentSymbol> = match node.child(1) {
            Some(c) => c
                .children(&mut cursor)
                .filter_map(|child| find_document_heading(child, &text))
                .collect(),
            None => vec![],
        };
        Some(
            #[allow(deprecated)]
            DocumentSymbol {
                name: heading_str,
                detail: None,
                kind: SymbolKind::NAMESPACE,
                tags: None,
                deprecated: None,
                range: utils::treesitter_range_to_lsp_range(node.range()),
                selection_range: utils::treesitter_range_to_lsp_range(first_child.range()),
                children: if b.len() > 0 { Some(b) } else { None },
            },
        )
    } else {
        None
    }
}

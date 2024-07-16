use std::borrow::Borrow;
use tower_lsp::lsp_types::{DocumentSymbol, SymbolKind};
use super::utils;

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
        let heading_events: Vec<jotdown::Event> = jotdown::Parser::new(&heading_str).collect();

        assert!(heading_events.len() >= 4);
        assert!(matches!(heading_events[0], jotdown::Event::Start { .. }));
        assert!(matches!(heading_events[1], jotdown::Event::Start { .. }));
        assert!(matches!(
            heading_events[heading_events.len() - 1],
            jotdown::Event::End { .. }
        ));
        assert!(matches!(
            heading_events[heading_events.len() - 2],
            jotdown::Event::End { .. }
        ));

        let heading_str = heading_events[2..(heading_events.len() - 2)]
            .iter()
            .filter_map(|e| -> Option<&str> {
                match e {
                    jotdown::Event::Str(s) => Some(s.borrow()),
                    jotdown::Event::Softbreak => Some(" "),
                    _ => None,
                }
            })
            .collect();

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

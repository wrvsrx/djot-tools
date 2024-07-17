use super::utils;
use tower_lsp::lsp_types::{DocumentSymbol, SymbolKind};

fn render_heading(text: &str) -> String {
    // FIXME: consider the case
    // ## Heading
    // ##.a
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

#[cfg(test)]
mod tests {
    use super::*;
    use tower_lsp::lsp_types::*;

    #[test]
    fn document_symbol() {
        let source_code = "# Heading

something

## nest heading

content

# Heading 2
    continue

114514

# Heading 3
#    continue
";
        let text = ropey::Rope::from_str(&source_code);
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_djot::language())
            .expect("Error loading djot grammer");
        let tree = parser.parse(&source_code, None).unwrap();
        let mut cursor = tree.root_node().walk();
        let symbols = tree
            .root_node()
            .children(&mut cursor)
            .filter_map(|child| find_document_heading(child, &text))
            .collect::<Vec<_>>();
        println!("{:?}", tree.root_node().to_sexp());
        println!("{:?}", symbols);
        let a = [
            #[allow(deprecated)]
            DocumentSymbol {
                name: "Heading".to_string(),
                detail: None,
                kind: SymbolKind::NAMESPACE,
                tags: None,
                deprecated: None,
                range: Range {
                    start: Position {
                        line: 0,
                        character: 0,
                    },
                    end: Position {
                        line: 8,
                        character: 0,
                    },
                },
                selection_range: Range {
                    start: Position {
                        line: 0,
                        character: 0,
                    },
                    end: Position {
                        line: 1,
                        character: 0,
                    },
                },
                children: Some(vec![DocumentSymbol {
                    name: "nest heading".to_string(),
                    detail: None,
                    kind: SymbolKind::NAMESPACE,
                    tags: None,
                    deprecated: None,
                    range: Range {
                        start: Position {
                            line: 4,
                            character: 0,
                        },
                        end: Position {
                            line: 8,
                            character: 0,
                        },
                    },
                    selection_range: Range {
                        start: Position {
                            line: 4,
                            character: 0,
                        },
                        end: Position {
                            line: 5,
                            character: 0,
                        },
                    },
                    children: None,
                }]),
            },
            #[allow(deprecated)]
            DocumentSymbol {
                name: "Heading 2 continue".to_string(),
                detail: None,
                kind: SymbolKind::NAMESPACE,
                tags: None,
                deprecated: None,
                range: Range {
                    start: Position {
                        line: 8,
                        character: 0,
                    },
                    end: Position {
                        line: 13,
                        character: 0,
                    },
                },
                selection_range: Range {
                    start: Position {
                        line: 8,
                        character: 0,
                    },
                    end: Position {
                        line: 10,
                        character: 0,
                    },
                },
                children: None,
            },
            #[allow(deprecated)]
            DocumentSymbol {
                name: "Heading 3 continue".to_string(),
                detail: None,
                kind: SymbolKind::NAMESPACE,
                tags: None,
                deprecated: None,
                range: Range {
                    start: Position {
                        line: 13,
                        character: 0,
                    },
                    end: Position {
                        line: 15,
                        character: 0,
                    },
                },
                selection_range: Range {
                    start: Position {
                        line: 13,
                        character: 0,
                    },
                    end: Position {
                        line: 15,
                        character: 0,
                    },
                },
                children: None,
            },
        ];
        assert_eq!(symbols, a);
    }
}

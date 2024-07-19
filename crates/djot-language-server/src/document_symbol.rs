use crate::utils::byte_range_to_lsp_range;

use super::adapter::*;
use super::ast::*;
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
    text: &ropey::Rope,
    node: &super::ast::Node,
) -> Option<DocumentSymbol> {
    match node {
        Node::Unified(c, _, ns, total_range) => match c {
            Container::Section { .. } => {
                let first_child = ns.first().unwrap();
                let heading_range = match first_child {
                    Node::Unified(header_container, _, _, r) => {
                        assert!(matches!(header_container, Container::Heading { .. }));
                        r
                    }
                    Node::Leaf(_, _) => unreachable!(),
                };
                let heading_str = text
                    .slice(
                        text.char_to_byte(heading_range.start)
                            ..text.char_to_byte(heading_range.end),
                    )
                    .to_string();
                let heading_str = render_heading(&heading_str);

                let other_headings = ns
                    .iter()
                    .filter_map(|n| find_document_heading(text, n))
                    .collect::<Vec<_>>();
                Some(
                    #[allow(deprecated)]
                    DocumentSymbol {
                        name: heading_str,
                        detail: None,
                        kind: SymbolKind::NAMESPACE,
                        tags: None,
                        deprecated: None,
                        range: byte_range_to_lsp_range(total_range, text),
                        selection_range: byte_range_to_lsp_range(heading_range, text),
                        children: if other_headings.len() > 0 {
                            Some(other_headings)
                        } else {
                            None
                        },
                    },
                )
            }
            _ => None,
        },
        _ => None,
    }
}

//! `djot-export`: convert a djot document to a [pandoc] JSON AST on stdout, so
//! it can be piped into pandoc (`djot-export doc.dj | pandoc -f json -o doc.pdf`).
//!
//! This is where djot-ls's conventions become document semantics for export:
//! the `{.metadata}` toml block is lifted out of the body (and, eventually,
//! folded into pandoc `Meta`) rather than rendered as a literal code block.
//!
//! The converter currently covers a common subset of djot; unhandled containers
//! are transparently flattened so output is always valid pandoc JSON.
//!
//! [pandoc]: https://pandoc.org

use std::io::Read;
use std::process::ExitCode;

use jotdown::{Container, Event, ListKind, Parser};
use serde_json::{json, Value};

/// pandoc-types API version this output targets (matches pandoc 3.7).
const API_VERSION: [u32; 4] = [1, 23, 1, 1];

fn main() -> ExitCode {
    let input = match std::env::args().nth(1) {
        Some(path) => match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) => {
                eprintln!("djot-export: cannot read {path}: {err}");
                return ExitCode::FAILURE;
            }
        },
        None => {
            let mut buf = String::new();
            if let Err(err) = std::io::stdin().read_to_string(&mut buf) {
                eprintln!("djot-export: cannot read stdin: {err}");
                return ExitCode::FAILURE;
            }
            buf
        }
    };

    let ast = to_pandoc_json(&input);
    println!("{}", ast);
    ExitCode::SUCCESS
}

/// Convert djot `text` into a pandoc JSON AST document.
fn to_pandoc_json(text: &str) -> Value {
    json!({
        "pandoc-api-version": API_VERSION,
        "meta": build_meta(text),
        "blocks": convert_blocks(text),
    })
}

/// Fold the leading `{.metadata}` toml block into a pandoc `Meta` map. The block
/// stays out of the rendered body (see `convert_blocks`); this is where its
/// information is preserved instead of dropped.
fn build_meta(text: &str) -> Value {
    let table = djot_core::metadata_block(text)
        .and_then(|toml_src| toml::from_str::<toml::Table>(&toml_src).ok());
    match table {
        Some(table) => Value::Object(
            table
                .iter()
                .map(|(key, value)| (key.clone(), toml_to_meta(value)))
                .collect(),
        ),
        None => json!({}),
    }
}

/// Convert a toml value into a pandoc `MetaValue` JSON node.
fn toml_to_meta(value: &toml::Value) -> Value {
    match value {
        toml::Value::String(s) => json!({ "t": "MetaString", "c": s }),
        toml::Value::Boolean(b) => json!({ "t": "MetaBool", "c": b }),
        toml::Value::Integer(n) => json!({ "t": "MetaString", "c": n.to_string() }),
        toml::Value::Float(n) => json!({ "t": "MetaString", "c": n.to_string() }),
        toml::Value::Datetime(d) => json!({ "t": "MetaString", "c": d.to_string() }),
        toml::Value::Array(items) => json!({
            "t": "MetaList",
            "c": items.iter().map(toml_to_meta).collect::<Vec<_>>(),
        }),
        toml::Value::Table(table) => json!({
            "t": "MetaMap",
            "c": table.iter().map(|(k, v)| (k.clone(), toml_to_meta(v))).collect::<serde_json::Map<_, _>>(),
        }),
    }
}

/// What a finished frame contributes to its parent.
enum Built {
    /// A single pandoc node.
    Node(Value),
    /// Splice the children straight into the parent (unhandled containers).
    Splice(Vec<Value>),
    /// Drop entirely (the metadata block).
    Drop,
}

/// The kind of djot container a frame represents, with the data needed to build
/// its pandoc node on close.
enum Kind {
    Root,
    Section {
        id: String,
    },
    Heading {
        level: u32,
    },
    Para,
    BlockQuote,
    List {
        ordered: bool,
    },
    ListItem,
    Emph,
    Strong,
    Link {
        dst: String,
    },
    /// Inline code / fenced code: text is accumulated rather than child nodes.
    Verbatim,
    CodeBlock {
        lang: String,
        metadata: bool,
    },
    /// Produces no output and discards its children (e.g. link reference
    /// definitions, which pandoc resolves rather than rendering).
    Drop,
    Other,
}

/// An in-progress container while walking the event stream.
struct Frame {
    kind: Kind,
    children: Vec<Value>,
    /// Raw text, for verbatim/code containers.
    text: String,
}

impl Frame {
    fn new(kind: Kind) -> Self {
        Frame {
            kind,
            children: Vec::new(),
            text: String::new(),
        }
    }

    /// Whether plain `Str` events should accumulate as raw text (code) rather
    /// than become `Str` inline nodes.
    fn is_verbatim(&self) -> bool {
        matches!(self.kind, Kind::Verbatim | Kind::CodeBlock { .. })
    }

    fn build(self) -> Built {
        let attr_empty = json!(["", [], []]);
        match self.kind {
            // Root never reaches build(); handled in the loop.
            Kind::Root => Built::Splice(self.children),
            Kind::Section { id } => Built::Node(json!({
                "t": "Div",
                "c": [[id, ["section"], []], self.children],
            })),
            Kind::Heading { level } => Built::Node(json!({
                "t": "Header",
                "c": [level, attr_empty, self.children],
            })),
            Kind::Para => Built::Node(json!({ "t": "Para", "c": self.children })),
            Kind::BlockQuote => Built::Node(json!({ "t": "BlockQuote", "c": self.children })),
            Kind::List { ordered } => {
                if ordered {
                    Built::Node(json!({
                        "t": "OrderedList",
                        "c": [[1, {"t": "Decimal"}, {"t": "Period"}], self.children],
                    }))
                } else {
                    Built::Node(json!({ "t": "BulletList", "c": self.children }))
                }
            }
            // A list item is a raw [Block]; tighten a lone Para to Plain.
            Kind::ListItem => {
                let blocks = match self.children.as_slice() {
                    [only] if only.get("t").and_then(Value::as_str) == Some("Para") => {
                        vec![json!({ "t": "Plain", "c": only["c"].clone() })]
                    }
                    _ => self.children,
                };
                Built::Node(Value::Array(blocks))
            }
            Kind::Emph => Built::Node(json!({ "t": "Emph", "c": self.children })),
            Kind::Strong => Built::Node(json!({ "t": "Strong", "c": self.children })),
            Kind::Link { dst } => Built::Node(json!({
                "t": "Link",
                "c": [attr_empty, self.children, [dst, ""]],
            })),
            Kind::Verbatim => Built::Node(json!({ "t": "Code", "c": [attr_empty, self.text] })),
            Kind::CodeBlock { lang, metadata } => {
                if metadata {
                    // The transformation: lift metadata out of the rendered body.
                    Built::Drop
                } else {
                    let classes = if lang.is_empty() { vec![] } else { vec![lang] };
                    Built::Node(json!({
                        "t": "CodeBlock",
                        "c": [["", classes, []], self.text],
                    }))
                }
            }
            Kind::Drop => Built::Drop,
            Kind::Other => Built::Splice(self.children),
        }
    }
}

fn convert_blocks(text: &str) -> Vec<Value> {
    let mut stack: Vec<Frame> = Vec::new();
    let mut roots: Vec<Value> = Vec::new();

    for (event, _span) in Parser::new(text).into_offset_iter() {
        match event {
            Event::Start(container, attrs) => {
                let kind = match &container {
                    Container::Document => Kind::Root,
                    Container::Section { id } => Kind::Section { id: id.to_string() },
                    Container::Heading { level, .. } => Kind::Heading {
                        level: *level as u32,
                    },
                    Container::Paragraph => Kind::Para,
                    Container::Blockquote => Kind::BlockQuote,
                    Container::List { kind, .. } => Kind::List {
                        ordered: matches!(kind, ListKind::Ordered { .. }),
                    },
                    Container::ListItem => Kind::ListItem,
                    Container::Emphasis => Kind::Emph,
                    Container::Strong => Kind::Strong,
                    Container::Link(dst, _) => Kind::Link {
                        dst: dst.to_string(),
                    },
                    Container::LinkDefinition { .. } => Kind::Drop,
                    Container::Verbatim => Kind::Verbatim,
                    Container::CodeBlock { language } => Kind::CodeBlock {
                        lang: language.to_string(),
                        metadata: djot_core::has_class(&attrs, djot_core::METADATA_CLASS),
                    },
                    _ => Kind::Other,
                };
                stack.push(Frame::new(kind));
            }
            Event::End(_) => {
                let frame = stack.pop().expect("unbalanced End event");
                if matches!(frame.kind, Kind::Root) {
                    roots = frame.children;
                    continue;
                }
                let built = frame.build();
                let parent = stack.last_mut().expect("node outside document");
                match built {
                    Built::Node(node) => parent.children.push(node),
                    Built::Splice(nodes) => parent.children.extend(nodes),
                    Built::Drop => {}
                }
            }
            Event::Str(s) => {
                if let Some(top) = stack.last_mut() {
                    if top.is_verbatim() {
                        top.text.push_str(&s);
                    } else {
                        top.children.push(json!({ "t": "Str", "c": s.as_ref() }));
                    }
                }
            }
            Event::Softbreak => push_inline(&mut stack, json!({ "t": "SoftBreak" })),
            Event::Hardbreak => push_inline(&mut stack, json!({ "t": "LineBreak" })),
            // Smart punctuation: emit the resolved character as text.
            Event::EnDash => push_inline(&mut stack, str_node("\u{2013}")),
            Event::EmDash => push_inline(&mut stack, str_node("\u{2014}")),
            Event::Ellipsis => push_inline(&mut stack, str_node("\u{2026}")),
            Event::NonBreakingSpace => push_inline(&mut stack, str_node("\u{a0}")),
            Event::ThematicBreak(_) => {
                if let Some(top) = stack.last_mut() {
                    top.children.push(json!({ "t": "HorizontalRule" }));
                }
            }
            // Blanklines, symbols, footnotes, etc. are ignored for now.
            _ => {}
        }
    }

    roots
}

fn str_node(s: &str) -> Value {
    json!({ "t": "Str", "c": s })
}

fn push_inline(stack: &mut [Frame], node: Value) {
    if let Some(top) = stack.last_mut() {
        if !top.is_verbatim() {
            top.children.push(node);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emits_valid_top_level_shape() {
        let ast = to_pandoc_json("# Hi\n");
        assert_eq!(ast["pandoc-api-version"], json!([1, 23, 1, 1]));
        assert!(ast["blocks"].is_array());
    }

    #[test]
    fn metadata_is_folded_into_meta_not_dropped() {
        let ast =
            to_pandoc_json("{.metadata}\n``` toml\ntitle = \"X\"\ndraft = true\n```\n\n# H\n");
        assert_eq!(ast["meta"]["title"], json!({ "t": "MetaString", "c": "X" }));
        assert_eq!(ast["meta"]["draft"], json!({ "t": "MetaBool", "c": true }));
        // The block is folded into meta, so the body holds only the "# H" section.
        assert_eq!(ast["blocks"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn heading_becomes_section_div_with_header() {
        let blocks = convert_blocks("# Title\n");
        assert_eq!(blocks[0]["t"], "Div");
        let inner = &blocks[0]["c"][1];
        assert_eq!(inner[0]["t"], "Header");
        assert_eq!(inner[0]["c"][0], 1);
    }

    #[test]
    fn smart_punctuation_becomes_text() {
        let blocks = convert_blocks("a -- b ... c\n");
        let text: String = blocks[0]["c"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|n| n["c"].as_str())
            .collect();
        assert!(text.contains('\u{2013}'), "en dash"); // --
        assert!(text.contains('\u{2026}'), "ellipsis"); // ...
    }

    #[test]
    fn link_reference_definitions_do_not_leak() {
        // The `[ref]: url` definition must not emit a stray Str at block level.
        let blocks = convert_blocks("see [x][ref]\n\n[ref]: https://e.com\n");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0]["t"], "Para");
    }

    #[test]
    fn metadata_block_is_dropped_from_body() {
        let blocks = convert_blocks("{.metadata}\n``` toml\ntitle = \"x\"\n```\n\n# H\n");
        // Only the section Div for "# H" survives; the metadata block is gone.
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0]["t"], "Div");
        // A non-metadata code block is kept.
        let kept = convert_blocks("``` toml\ntitle = \"x\"\n```\n");
        assert_eq!(kept[0]["t"], "CodeBlock");
    }
}

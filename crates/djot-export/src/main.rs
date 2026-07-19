//! `djot-export`: convert a djot document to a [pandoc] JSON AST on stdout, so
//! it can be piped into pandoc (`djot-export doc.dj | pandoc -f json -o doc.pdf`).
//!
//! Pandoc's native djot reader owns the syntax conversion. This binary applies
//! `djot-tools` export semantics on top of the resulting Pandoc AST:
//!
//! - the first `.metadata` definition list is folded into Pandoc metadata and
//!   removed from the rendered body;
//! - every `[X]{.cite}` span is rewritten into a Pandoc `Cite` node, where `X`
//!   is treated exactly as the body of a pandoc-markdown citation bracket
//!   (`[X]`). The parsing is delegated back to pandoc so the supported forms
//!   (`[@k]`, `[-@k]`, `[@k, p. 3]`, `[see @k]`, `[@a; @b]`) stay identical to
//!   pandoc-markdown. A downstream `pandoc --citeproc` then resolves them.
//!
//! [pandoc]: https://pandoc.org

use std::io::{Read, Write};
use std::process::{Command, ExitCode, Stdio};

use pandoc_types::definition::{Attr, Block, Inline, MetaValue, Pandoc};
use serde_json::Value;

/// Span class that marks a citation, e.g. `[@smith2004]{.cite}`. Export-only.
const CITE_CLASS: &str = "cite";

fn main() -> ExitCode {
    let input = match read_input() {
        Ok(input) => input,
        Err(err) => {
            eprintln!("djot-export: {err}");
            return ExitCode::FAILURE;
        }
    };

    match to_pandoc_json(&input) {
        Ok(json) => {
            println!("{json}");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("djot-export: {err}");
            ExitCode::FAILURE
        }
    }
}

fn read_input() -> Result<String, String> {
    match std::env::args().nth(1) {
        Some(path) => {
            std::fs::read_to_string(&path).map_err(|err| format!("cannot read {path}: {err}"))
        }
        None => {
            let mut buf = String::new();
            std::io::stdin()
                .read_to_string(&mut buf)
                .map_err(|err| format!("cannot read stdin: {err}"))?;
            Ok(buf)
        }
    }
}

/// Convert djot `text` into a Pandoc JSON AST document.
fn to_pandoc_json(text: &str) -> Result<String, String> {
    let json = run_pandoc(&["-f", "djot", "-t", "json"], text)?;
    let mut value: Value =
        serde_json::from_str(&json).map_err(|err| format!("cannot parse pandoc JSON: {err}"))?;

    convert_cite_spans_in(&mut value)?;

    let mut document: Pandoc =
        serde_json::from_value(value).map_err(|err| format!("cannot parse pandoc JSON: {err}"))?;
    fold_metadata_definition_list(&mut document);
    serde_json::to_string(&document).map_err(|err| format!("cannot write pandoc JSON: {err}"))
}

/// Run `pandoc` with `args`, feeding `input` on stdin, and return its stdout.
fn run_pandoc(args: &[&str], input: &str) -> Result<String, String> {
    let mut child = Command::new("pandoc")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("cannot run pandoc: {err}"))?;

    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "cannot open pandoc stdin".to_string())?;
    stdin
        .write_all(input.as_bytes())
        .map_err(|err| format!("cannot write to pandoc: {err}"))?;
    drop(stdin);

    let output = child
        .wait_with_output()
        .map_err(|err| format!("cannot wait for pandoc: {err}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let message = stderr.trim();
        return Err(if message.is_empty() {
            format!("pandoc exited with {}", output.status)
        } else {
            format!("pandoc exited with {}: {message}", output.status)
        });
    }

    String::from_utf8(output.stdout).map_err(|err| format!("pandoc wrote non-UTF-8 JSON: {err}"))
}

/// Rewrite every `[X]{.cite}` span anywhere in `value` (body or metadata) into a
/// pandoc `Cite` node, by delegating the parsing of each `X` to pandoc.
fn convert_cite_spans_in(value: &mut Value) -> Result<(), String> {
    let mut texts = Vec::new();
    collect_cite_texts(value, &mut texts);
    if !texts.is_empty() {
        let cites = parse_citations_via_pandoc(&texts)?;
        let mut idx = 0;
        replace_cite_spans(value, &cites, &mut idx);
    }
    Ok(())
}

/// If `value` is a `[X]{.cite}` span, return its inline-content `Value` (`X`).
fn cite_span_content(value: &Value) -> Option<&Value> {
    let object = value.as_object()?;
    if object.get("t")?.as_str()? != "Span" {
        return None;
    }
    let content = object.get("c")?.as_array()?;
    let classes = content.first()?.as_array()?.get(1)?.as_array()?;
    if classes
        .iter()
        .any(|class| class.as_str() == Some(CITE_CLASS))
    {
        content.get(1)
    } else {
        None
    }
}

/// Collect the citation body text of every `.cite` span, in document order.
fn collect_cite_texts(value: &Value, out: &mut Vec<String>) {
    if let Some(content) = cite_span_content(value) {
        let mut text = String::new();
        inline_text(content, &mut text);
        out.push(text.trim().to_string());
        return;
    }
    match value {
        Value::Array(items) => items.iter().for_each(|item| collect_cite_texts(item, out)),
        Value::Object(map) => map.values().for_each(|item| collect_cite_texts(item, out)),
        _ => {}
    }
}

/// Flatten inline `Value`s to their plain text, joining words with spaces.
fn inline_text(value: &Value, out: &mut String) {
    match value {
        Value::Array(items) => items.iter().for_each(|item| inline_text(item, out)),
        Value::Object(map) => match map.get("t").and_then(Value::as_str) {
            Some("Str") => {
                if let Some(text) = map.get("c").and_then(Value::as_str) {
                    out.push_str(text);
                }
            }
            Some("Space" | "SoftBreak" | "LineBreak") => out.push(' '),
            _ => {
                if let Some(child) = map.get("c") {
                    inline_text(child, out);
                }
            }
        },
        _ => {}
    }
}

/// Replace each `.cite` span with the matching parsed `Cite` node, in order.
/// A `None` entry (body was not a valid citation) leaves the span unchanged.
fn replace_cite_spans(value: &mut Value, cites: &[Option<Value>], idx: &mut usize) {
    if cite_span_content(value).is_some() {
        if let Some(Some(cite)) = cites.get(*idx) {
            *value = cite.clone();
        }
        *idx += 1;
        return;
    }
    match value {
        Value::Array(items) => items
            .iter_mut()
            .for_each(|item| replace_cite_spans(item, cites, idx)),
        Value::Object(map) => map
            .values_mut()
            .for_each(|item| replace_cite_spans(item, cites, idx)),
        _ => {}
    }
}

/// Find the first `Cite` inline anywhere inside a block `Value`.
fn extract_cite_from_block(block: &Value) -> Option<Value> {
    fn find(value: &Value) -> Option<Value> {
        if let Value::Object(map) = value {
            if map.get("t").and_then(Value::as_str) == Some("Cite") {
                return Some(value.clone());
            }
        }
        match value {
            Value::Array(items) => items.iter().find_map(find),
            Value::Object(map) => map.values().find_map(find),
            _ => None,
        }
    }
    find(block)
}

/// Parse each citation body `X` by handing `[X]` back to pandoc's markdown
/// reader, returning one `Cite` `Value` per input (or `None` if `X` is not a
/// citation). Order matches `texts`.
fn parse_citations_via_pandoc(texts: &[String]) -> Result<Vec<Option<Value>>, String> {
    let markdown = texts
        .iter()
        .map(|text| format!("[{}]", text.replace('\n', " ")))
        .collect::<Vec<_>>()
        .join("\n\n");
    let json = run_pandoc(&["-f", "markdown", "-t", "json"], &markdown)?;
    let document: Value = serde_json::from_str(&json)
        .map_err(|err| format!("cannot parse pandoc citation JSON: {err}"))?;
    let blocks = document
        .get("blocks")
        .and_then(Value::as_array)
        .ok_or_else(|| "pandoc citation output has no blocks".to_string())?;
    if blocks.len() != texts.len() {
        return Err(format!(
            "expected {} citation blocks from pandoc, got {}",
            texts.len(),
            blocks.len()
        ));
    }
    let cites: Vec<Option<Value>> = blocks.iter().map(extract_cite_from_block).collect();
    for (text, cite) in texts.iter().zip(&cites) {
        if cite.is_none() {
            eprintln!("djot-export: warning: .cite span is not a valid citation: [{text}]");
        }
    }
    Ok(cites)
}

/// Fold the first `.metadata` definition list into `document.meta`.
///
/// Pandoc represents attributes on a Djot definition list by wrapping the list
/// in a `Div`. Definition values are already parsed Djot blocks, so rich inline
/// and block content move directly into metadata. Bullet lists map recursively
/// to `MetaList`, preserving structured fields such as `math_macros` triples.
fn fold_metadata_definition_list(document: &mut Pandoc) {
    let mut found = None;
    document.blocks.retain(|block| {
        if found.is_none() {
            if let Block::Div(attr, blocks) = block {
                if has_class(attr, djot_core::METADATA_CLASS)
                    && matches!(blocks.as_slice(), [Block::DefinitionList(_)])
                {
                    found = blocks.first().cloned();
                    return false;
                }
            }
        }
        true
    });

    let Some(Block::DefinitionList(fields)) = found else {
        return;
    };
    for (term, definitions) in fields {
        let Some(key) = metadata_key(&term) else {
            continue;
        };
        let value = match definitions.as_slice() {
            [blocks] => blocks_to_meta(blocks.clone()),
            definitions => {
                MetaValue::MetaList(definitions.iter().cloned().map(blocks_to_meta).collect())
            }
        };
        document.meta.entry(key).or_insert(value);
    }
}

fn metadata_key(term: &[Inline]) -> Option<String> {
    let mut key = String::new();
    for inline in term {
        match inline {
            Inline::Str(text) => key.push_str(text),
            Inline::Space | Inline::SoftBreak | Inline::LineBreak => key.push(' '),
            _ => return None,
        }
    }
    let key = key.trim();
    (!key.is_empty()).then(|| key.to_string())
}

fn blocks_to_meta(blocks: Vec<Block>) -> MetaValue {
    match <[Block; 1]>::try_from(blocks) {
        Ok([Block::BulletList(items)]) => {
            MetaValue::MetaList(items.into_iter().map(blocks_to_meta).collect())
        }
        Ok([Block::Para(inlines) | Block::Plain(inlines)])
            if matches!(inlines.as_slice(), [Inline::Code(_, _)]) =>
        {
            let [Inline::Code(_, text)] = inlines.as_slice() else {
                unreachable!()
            };
            MetaValue::MetaString(text.clone())
        }
        Ok([Block::Para(inlines) | Block::Plain(inlines)]) => MetaValue::MetaInlines(inlines),
        Ok([block]) => MetaValue::MetaBlocks(vec![block]),
        Err(blocks) if blocks.is_empty() => MetaValue::MetaString(String::new()),
        Err(blocks) => MetaValue::MetaBlocks(blocks),
    }
}

fn has_class(attr: &Attr, class: &str) -> bool {
    attr.classes.iter().any(|candidate| candidate == class)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn inlines(text: &str) -> MetaValue {
        MetaValue::MetaInlines(vec![Inline::Str(text.to_string())])
    }

    fn plain(text: &str) -> Vec<Block> {
        vec![Block::Plain(vec![Inline::Str(text.to_string())])]
    }

    fn field(name: &str, blocks: Vec<Block>) -> (Vec<Inline>, Vec<Vec<Block>>) {
        (vec![Inline::Str(name.to_string())], vec![blocks])
    }

    fn metadata_definition_list(fields: Vec<(Vec<Inline>, Vec<Vec<Block>>)>) -> Pandoc {
        Pandoc {
            meta: HashMap::new(),
            blocks: vec![
                Block::Div(
                    Attr {
                        identifier: String::new(),
                        classes: vec!["metadata".to_string()],
                        attributes: Vec::new(),
                    },
                    vec![Block::DefinitionList(fields)],
                ),
                Block::Header(1, Attr::default(), vec![Inline::Str("Heading".to_string())]),
            ],
        }
    }

    #[test]
    fn metadata_is_folded_into_meta_and_removed_from_body() {
        let mut document = metadata_definition_list(vec![field("title", plain("X"))]);

        fold_metadata_definition_list(&mut document);

        assert_eq!(document.meta.get("title"), Some(&inlines("X")));
        assert!(matches!(document.blocks.as_slice(), [Block::Header(..)]));
    }

    #[test]
    fn bullet_lists_become_nested_meta_lists_and_verbatim_becomes_string() {
        let tuple = |name: &str, expansion: &str, nargs: &str| {
            vec![Block::BulletList(vec![
                vec![Block::Plain(vec![Inline::Code(
                    Attr::default(),
                    name.to_string(),
                )])],
                vec![Block::Plain(vec![Inline::Code(
                    Attr::default(),
                    expansion.to_string(),
                )])],
                plain(nargs),
            ])]
        };
        let macros = vec![Block::BulletList(vec![
            tuple("norm", "\\lVert #1 \\rVert", "1"),
            tuple("R", "\\mathbb{R}", "0"),
        ])];
        let mut document = metadata_definition_list(vec![
            field(
                "created",
                vec![Block::Plain(vec![Inline::Code(
                    Attr::default(),
                    "2026-06-22T09:00:00+08:00".to_string(),
                )])],
            ),
            field("math_macros", macros),
        ]);

        fold_metadata_definition_list(&mut document);

        assert_eq!(
            document.meta.get("created"),
            Some(&MetaValue::MetaString(
                "2026-06-22T09:00:00+08:00".to_string()
            ))
        );
        let Some(MetaValue::MetaList(macros)) = document.meta.get("math_macros") else {
            panic!("math_macros was not a list")
        };
        assert_eq!(macros.len(), 2);
        assert!(matches!(macros[0], MetaValue::MetaList(ref tuple) if tuple.len() == 3));
    }

    #[test]
    fn blocks_reduce_to_inlines_blocks_or_string() {
        let para = Block::Para(vec![Inline::Str("hi".to_string())]);
        assert_eq!(
            blocks_to_meta(vec![para.clone()]),
            MetaValue::MetaInlines(vec![Inline::Str("hi".to_string())])
        );
        assert!(matches!(
            blocks_to_meta(vec![para.clone(), para]),
            MetaValue::MetaBlocks(_)
        ));
        assert_eq!(blocks_to_meta(vec![]), MetaValue::MetaString(String::new()));
    }

    #[test]
    fn metadata_class_on_non_definition_list_is_kept() {
        let mut document = Pandoc {
            meta: HashMap::new(),
            blocks: vec![Block::Div(
                Attr {
                    identifier: String::new(),
                    classes: vec!["metadata".to_string()],
                    attributes: Vec::new(),
                },
                vec![Block::Para(vec![Inline::Str("not metadata".to_string())])],
            )],
        };

        fold_metadata_definition_list(&mut document);

        assert!(document.meta.is_empty());
        assert!(matches!(document.blocks.as_slice(), [Block::Div(..)]));
    }

    #[test]
    fn non_metadata_code_block_is_kept() {
        let mut document = Pandoc {
            meta: HashMap::new(),
            blocks: vec![Block::CodeBlock(
                Attr {
                    identifier: String::new(),
                    classes: vec!["toml".to_string()],
                    attributes: Vec::new(),
                },
                "title = \"X\"\n".to_string(),
            )],
        };

        fold_metadata_definition_list(&mut document);

        assert!(document.meta.is_empty());
        assert!(matches!(document.blocks.as_slice(), [Block::CodeBlock(..)]));
    }

    use serde_json::json;

    /// A `[X]{.cite}` span `Value` whose inline content is a single `Str`.
    fn cite_span(text: &str) -> Value {
        json!({"t": "Span", "c": [["", ["cite"], []], [{"t": "Str", "c": text}]]})
    }

    #[test]
    fn collect_finds_nested_cite_text_in_order() {
        // Two cite spans, the first nested inside an Emph, plus a plain span.
        let document = json!({
            "blocks": [{"t": "Para", "c": [
                {"t": "Emph", "c": [cite_span("@smith2004")]},
                {"t": "Str", "c": "and"},
                cite_span("@doe2010"),
            ]}]
        });

        let mut texts = Vec::new();
        collect_cite_texts(&document, &mut texts);

        assert_eq!(
            texts,
            vec!["@smith2004".to_string(), "@doe2010".to_string()]
        );
    }

    #[test]
    fn span_without_cite_class_is_not_a_citation() {
        let span = json!({"t": "Span", "c": [["", ["aside"], []], [{"t": "Str", "c": "x"}]]});
        assert!(cite_span_content(&span).is_none());
    }

    #[test]
    fn replace_swaps_cites_and_leaves_invalid_spans() {
        let mut document = json!({
            "blocks": [{"t": "Para", "c": [
                cite_span("@smith2004"),
                {"t": "Str", "c": "between"},
                cite_span("not a cite"),
            ]}]
        });
        let cite = json!({"t": "Cite", "c": [[], [{"t": "Str", "c": "(Smith 2004)"}]]});
        let cites = vec![Some(cite.clone()), None];

        let mut idx = 0;
        replace_cite_spans(&mut document, &cites, &mut idx);

        let inlines = &document["blocks"][0]["c"];
        assert_eq!(idx, 2);
        assert_eq!(inlines[0], cite); // first span became the Cite
        assert_eq!(inlines[1], json!({"t": "Str", "c": "between"})); // untouched
        assert_eq!(inlines[2], cite_span("not a cite")); // None left as-is
    }

    #[test]
    fn extract_cite_pulls_cite_out_of_a_paragraph() {
        let cite = json!({"t": "Cite", "c": [[], [{"t": "Str", "c": "(Smith 2004)"}]]});
        let para = json!({"t": "Para", "c": [cite.clone()]});
        assert_eq!(extract_cite_from_block(&para), Some(cite));

        let plain = json!({"t": "Para", "c": [{"t": "Str", "c": "[foo]"}]});
        assert_eq!(extract_cite_from_block(&plain), None);
    }
}

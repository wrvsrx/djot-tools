//! `djot-export`: convert a djot document to a [pandoc] JSON AST on stdout, so
//! it can be piped into pandoc (`djot-export doc.dj | pandoc -f json -o doc.pdf`).
//!
//! Pandoc's native djot reader owns the syntax conversion. This binary applies
//! `djot-tools` export semantics on top of the resulting Pandoc AST: the first
//! `{.metadata}` TOML code block is folded into Pandoc metadata and removed
//! from the rendered body.
//!
//! [pandoc]: https://pandoc.org

use std::collections::HashMap;
use std::io::{Read, Write};
use std::process::{Command, ExitCode, Stdio};

use pandoc_types::definition::{Attr, Block, MetaValue, Pandoc};

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
    let json = pandoc_json_from_djot(text)?;
    let mut document: Pandoc =
        serde_json::from_str(&json).map_err(|err| format!("cannot parse pandoc JSON: {err}"))?;
    fold_metadata_block(&mut document);
    serde_json::to_string(&document).map_err(|err| format!("cannot write pandoc JSON: {err}"))
}

fn pandoc_json_from_djot(text: &str) -> Result<String, String> {
    let mut child = Command::new("pandoc")
        .args(["-f", "djot", "-t", "json"])
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
        .write_all(text.as_bytes())
        .map_err(|err| format!("cannot write djot to pandoc: {err}"))?;
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

fn fold_metadata_block(document: &mut Pandoc) {
    let mut found = None;
    document.blocks.retain(|block| {
        if found.is_none() {
            if let Block::CodeBlock(attr, text) = block {
                if has_class(attr, djot_core::METADATA_CLASS) {
                    found = Some(text.clone());
                    return false;
                }
            }
        }
        true
    });

    let Some(metadata) = found else {
        return;
    };
    let Ok(table) = toml::from_str::<toml::Table>(&metadata) else {
        return;
    };
    for (key, value) in table {
        document.meta.insert(key, toml_to_meta(value));
    }
}

fn has_class(attr: &Attr, class: &str) -> bool {
    attr.classes.iter().any(|candidate| candidate == class)
}

fn toml_to_meta(value: toml::Value) -> MetaValue {
    match value {
        toml::Value::String(s) => MetaValue::MetaString(s),
        toml::Value::Boolean(b) => MetaValue::MetaBool(b),
        toml::Value::Integer(n) => MetaValue::MetaString(n.to_string()),
        toml::Value::Float(n) => MetaValue::MetaString(n.to_string()),
        toml::Value::Datetime(d) => MetaValue::MetaString(d.to_string()),
        toml::Value::Array(items) => {
            MetaValue::MetaList(items.into_iter().map(toml_to_meta).collect())
        }
        toml::Value::Table(table) => MetaValue::MetaMap(
            table
                .into_iter()
                .map(|(key, value)| (key, toml_to_meta(value)))
                .collect::<HashMap<_, _>>(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pandoc_types::definition::Inline;

    #[test]
    fn metadata_is_folded_into_meta_and_removed_from_body() {
        let mut document = Pandoc {
            meta: HashMap::new(),
            blocks: vec![
                Block::CodeBlock(
                    Attr {
                        identifier: String::new(),
                        classes: vec!["metadata".to_string(), "toml".to_string()],
                        attributes: Vec::new(),
                    },
                    "title = \"X\"\ndraft = true\n".to_string(),
                ),
                Block::Header(1, Attr::default(), vec![Inline::Str("Heading".to_string())]),
            ],
        };

        fold_metadata_block(&mut document);

        assert_eq!(
            document.meta.get("title"),
            Some(&MetaValue::MetaString("X".to_string()))
        );
        assert_eq!(document.meta.get("draft"), Some(&MetaValue::MetaBool(true)));
        assert!(matches!(document.blocks.as_slice(), [Block::Header(..)]));
    }

    #[test]
    fn invalid_metadata_is_removed_without_failing() {
        let mut document = Pandoc {
            meta: HashMap::new(),
            blocks: vec![Block::CodeBlock(
                Attr {
                    identifier: String::new(),
                    classes: vec!["metadata".to_string()],
                    attributes: Vec::new(),
                },
                "not = = toml\n".to_string(),
            )],
        };

        fold_metadata_block(&mut document);

        assert!(document.meta.is_empty());
        assert!(document.blocks.is_empty());
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

        fold_metadata_block(&mut document);

        assert!(document.meta.is_empty());
        assert!(matches!(document.blocks.as_slice(), [Block::CodeBlock(..)]));
    }
}

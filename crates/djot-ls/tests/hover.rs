//! End-to-end tests for `textDocument/hover`.

mod support;

use lsp_types::Url;
use serde_json::{json, Value};

use support::run_session;

#[test]
fn hover_shows_link_target_heading() {
    let dir = std::env::temp_dir().join("djot-ls-hover-heading-test");
    std::fs::create_dir_all(&dir).unwrap();
    let a = dir.join("a.dj");
    let b = dir.join("b.dj");
    let doc_a = "# A\n\nsee [topic](b.dj#Topic)\n";
    std::fs::write(&a, doc_a).unwrap();
    std::fs::write(&b, "# Intro\n\n## Topic\n\nbody\nmore body\n").unwrap();

    let root_uri = Url::from_directory_path(&dir).unwrap().to_string();
    let a_uri = Url::from_file_path(&a).unwrap().to_string();
    let link_col = doc_a.lines().nth(2).unwrap().find("b.dj").unwrap() as i64;
    let msgs = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{},"processId":null,"rootUri":root_uri}}),
        json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
        json!({"jsonrpc":"2.0","id":2,"method":"textDocument/hover",
               "params":{"textDocument":{"uri":a_uri},"position":{"line":2,"character":link_col}}}),
        json!({"jsonrpc":"2.0","id":99,"method":"shutdown","params":null}),
        json!({"jsonrpc":"2.0","method":"exit","params":null}),
    ];

    let responses = run_session(&msgs);
    let value = hover_value(&responses, 2);

    assert!(value.contains("**Heading** `Topic`"));
    assert!(value.contains("`b.dj:3`"));
    assert!(value.contains("## Topic"));
    assert!(value.contains("body\nmore body"));
}

#[test]
fn hover_shows_link_target_file() {
    let dir = std::env::temp_dir().join("djot-ls-hover-file-test");
    std::fs::create_dir_all(&dir).unwrap();
    let a = dir.join("a.dj");
    let b = dir.join("b.dj");
    let doc_a = "# A\n\nsee [file](b.dj)\n";
    std::fs::write(&a, doc_a).unwrap();
    std::fs::write(&b, "\n# Target File\n\nbody\nmore body\n").unwrap();

    let root_uri = Url::from_directory_path(&dir).unwrap().to_string();
    let a_uri = Url::from_file_path(&a).unwrap().to_string();
    let link_col = doc_a.lines().nth(2).unwrap().find("b.dj").unwrap() as i64;
    let msgs = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{},"processId":null,"rootUri":root_uri}}),
        json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
        json!({"jsonrpc":"2.0","id":2,"method":"textDocument/hover",
               "params":{"textDocument":{"uri":a_uri},"position":{"line":2,"character":link_col}}}),
        json!({"jsonrpc":"2.0","id":99,"method":"shutdown","params":null}),
        json!({"jsonrpc":"2.0","method":"exit","params":null}),
    ];

    let responses = run_session(&msgs);
    let value = hover_value(&responses, 2);

    assert!(value.contains("**File**"));
    assert!(value.contains("`b.dj:2`"));
    assert!(value.contains("# Target File"));
    assert!(value.contains("body\nmore body"));
}

#[test]
fn hover_shows_explicit_anchor_target() {
    let doc =
        "# A\n\nsee [note](#important-note)\n\n{#important-note}\nImportant text.\nMore text.\n";
    let msgs = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{},"processId":null,"rootUri":null}}),
        json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///a.dj","languageId":"djot","version":1,"text":doc}}}),
        json!({"jsonrpc":"2.0","id":2,"method":"textDocument/hover",
               "params":{"textDocument":{"uri":"file:///a.dj"},"position":{"line":2,"character":12}}}),
        json!({"jsonrpc":"2.0","id":99,"method":"shutdown","params":null}),
        json!({"jsonrpc":"2.0","method":"exit","params":null}),
    ];

    let responses = run_session(&msgs);
    let value = hover_value(&responses, 2);

    assert!(value.contains("**Anchor** `important-note`"));
    assert!(value.contains("`/a.dj:5`"));
    assert!(value.contains("{#important-note}"));
    assert!(value.contains("Important text.\nMore text."));
}

fn hover_value(responses: &[Value], id: i64) -> String {
    responses
        .iter()
        .find(|m| m["id"] == json!(id))
        .unwrap_or_else(|| panic!("no hover response for id {id}"))["result"]["contents"]["value"]
        .as_str()
        .expect("hover value is not a string")
        .to_string()
}

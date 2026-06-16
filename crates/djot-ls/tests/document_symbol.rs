//! End-to-end tests for `textDocument/documentSymbol`.

mod support;

use serde_json::{json, Value};

use support::run_session;

#[test]
fn document_symbol_returns_headings() {
    // Title > Section A > Sub, plus a sibling top-level heading.
    let doc = "# Title\n\nsome text\n\n## Section A\n\nmore\n\n### Sub\n\n# Second\n";
    let msgs = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{},"processId":null,"rootUri":null}}),
        json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///t.dj","languageId":"djot","version":1,"text":doc}}}),
        json!({"jsonrpc":"2.0","id":2,"method":"textDocument/documentSymbol","params":{"textDocument":{"uri":"file:///t.dj"}}}),
        json!({"jsonrpc":"2.0","id":3,"method":"shutdown","params":null}),
        json!({"jsonrpc":"2.0","method":"exit","params":null}),
    ];

    let responses = run_session(&msgs);
    let roots = responses
        .iter()
        .find(|m| m["id"] == json!(2))
        .expect("no documentSymbol response")["result"]
        .as_array()
        .expect("result is not an array")
        .clone();

    let name = |s: &Value| s["name"].as_str().unwrap().to_string();
    let detail = |s: &Value| s["detail"].as_str().unwrap().to_string();
    let children = |s: &Value| s["children"].as_array().cloned().unwrap_or_default();

    // Two top-level headings.
    assert_eq!(
        roots.iter().map(name).collect::<Vec<_>>(),
        ["Title", "Second"]
    );

    // Title (H1) > Section A (H2) > Sub (H3).
    let title = &roots[0];
    assert_eq!(detail(title), "H1");
    let lvl2 = children(title);
    assert_eq!(lvl2.iter().map(name).collect::<Vec<_>>(), ["Section A"]);
    assert_eq!(detail(&lvl2[0]), "H2");
    let lvl3 = children(&lvl2[0]);
    assert_eq!(lvl3.iter().map(name).collect::<Vec<_>>(), ["Sub"]);
    assert_eq!(detail(&lvl3[0]), "H3");

    // A parent's range must enclose its child's range (line-wise here).
    let title_end = title["range"]["end"]["line"].as_u64().unwrap();
    let sub_end = lvl3[0]["range"]["end"]["line"].as_u64().unwrap();
    assert!(
        title_end >= sub_end,
        "parent range must contain child range"
    );

    // The "Second" sibling is a leaf.
    assert!(children(&roots[1]).is_empty());
}

//! End-to-end tests for `textDocument/publishDiagnostics`.

mod support;

use lsp_types::Url;
use serde_json::{json, Value};

use support::run_session;

#[test]
fn diagnostics_report_unresolved_links() {
    let dir = std::env::temp_dir().join("djot-ls-diagnostics-unresolved-test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let a = dir.join("a.dj");
    let b = dir.join("b.dj");
    let doc_a = "# A\n\n[missing anchor](#Missing) [missing file](missing.dj) [missing cross anchor](b.dj#Nope) [ok](b.dj#Topic) [plain](AGENTS.md) [dir](crates/djot-core) [license](LICENSE) [url](https://example.com)\n";
    std::fs::write(&a, doc_a).unwrap();
    std::fs::write(&b, "# Topic\n").unwrap();

    let root_uri = Url::from_directory_path(&dir).unwrap().to_string();
    let a_uri = Url::from_file_path(&a).unwrap().to_string();
    let msgs = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{},"processId":null,"rootUri":root_uri}}),
        json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":a_uri,"languageId":"djot","version":1,"text":doc_a}}}),
        json!({"jsonrpc":"2.0","id":99,"method":"shutdown","params":null}),
        json!({"jsonrpc":"2.0","method":"exit","params":null}),
    ];

    let responses = run_session(&msgs);
    let diagnostics = last_diagnostics(&responses);
    let messages = diagnostics
        .iter()
        .map(|diagnostic| diagnostic["message"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();

    assert_eq!(messages.len(), 3);
    assert!(messages.contains(&"Unresolved anchor `Missing`".to_string()));
    assert!(messages.contains(&"Unresolved Djot path `missing.dj`".to_string()));
    assert!(messages.contains(&"Unresolved anchor `Nope`".to_string()));
}

#[test]
fn diagnostics_clear_after_links_are_fixed() {
    let doc_bad = "# A\n\n[bad](#Missing)\n";
    let doc_good = "# A\n\n[good](#A)\n";
    let msgs = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{},"processId":null,"rootUri":null}}),
        json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///a.dj","languageId":"djot","version":1,"text":doc_bad}}}),
        json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":"file:///a.dj","version":2},"contentChanges":[{"text":doc_good}]}}),
        json!({"jsonrpc":"2.0","id":99,"method":"shutdown","params":null}),
        json!({"jsonrpc":"2.0","method":"exit","params":null}),
    ];

    let responses = run_session(&msgs);
    let publish_counts = responses
        .iter()
        .filter(|m| m["method"] == json!("textDocument/publishDiagnostics"))
        .map(|m| m["params"]["diagnostics"].as_array().unwrap().len())
        .collect::<Vec<_>>();

    assert_eq!(publish_counts, [1, 0]);
}

#[test]
fn diagnostics_refresh_when_target_document_changes() {
    let doc_a = "# A\n\n[to b](b.dj#Missing)\n";
    let doc_b_bad = "# B\n";
    let doc_b_good = "# Missing\n";
    let msgs = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{},"processId":null,"rootUri":null}}),
        json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///a.dj","languageId":"djot","version":1,"text":doc_a}}}),
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///b.dj","languageId":"djot","version":1,"text":doc_b_bad}}}),
        json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":"file:///b.dj","version":2},"contentChanges":[{"text":doc_b_good}]}}),
        json!({"jsonrpc":"2.0","id":99,"method":"shutdown","params":null}),
        json!({"jsonrpc":"2.0","method":"exit","params":null}),
    ];

    let responses = run_session(&msgs);
    let a_publish_counts = responses
        .iter()
        .filter(|m| {
            m["method"] == json!("textDocument/publishDiagnostics")
                && m["params"]["uri"] == json!("file:///a.dj")
        })
        .map(|m| m["params"]["diagnostics"].as_array().unwrap().len())
        .collect::<Vec<_>>();

    assert_eq!(a_publish_counts, [1, 1, 0]);
}

fn last_diagnostics(responses: &[Value]) -> Vec<Value> {
    responses
        .iter()
        .rev()
        .find(|m| m["method"] == json!("textDocument/publishDiagnostics"))
        .expect("no publishDiagnostics notification")["params"]["diagnostics"]
        .as_array()
        .expect("diagnostics is not an array")
        .clone()
}

//! End-to-end tests for LSP notifications.

mod support;

use serde_json::json;

use support::run_session;

/// Regression: editors send `textDocument/didSave` on save. async-lsp's
/// omni-trait breaks the main loop on any unhandled notification, so an
/// unhandled `didSave` used to crash the server. The server must survive it and
/// keep answering requests.
#[test]
fn did_save_does_not_crash_the_server() {
    let doc = "# Title\n";
    let msgs = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{},"processId":null,"rootUri":null}}),
        json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///t.dj","languageId":"djot","version":1,"text":doc}}}),
        json!({"jsonrpc":"2.0","method":"textDocument/didSave","params":{"textDocument":{"uri":"file:///t.dj"}}}),
        // If didSave crashed the loop, this request would get no response.
        json!({"jsonrpc":"2.0","id":2,"method":"textDocument/documentSymbol","params":{"textDocument":{"uri":"file:///t.dj"}}}),
        json!({"jsonrpc":"2.0","id":3,"method":"shutdown","params":null}),
        json!({"jsonrpc":"2.0","method":"exit","params":null}),
    ];

    let responses = run_session(&msgs);
    let answered = responses
        .iter()
        .any(|m| m["id"] == json!(2) && m.get("result").is_some());
    assert!(
        answered,
        "server did not answer documentSymbol after didSave"
    );
}

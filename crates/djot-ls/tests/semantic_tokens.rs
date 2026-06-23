//! End-to-end tests for `textDocument/semanticTokens/full`.

mod support;

use serde_json::json;

use support::{response_result, run_session};

#[test]
fn initialize_advertises_task_semantic_tokens() {
    let msgs = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{},"processId":null,"rootUri":null}}),
        json!({"jsonrpc":"2.0","id":2,"method":"shutdown","params":null}),
        json!({"jsonrpc":"2.0","method":"exit","params":null}),
    ];

    let responses = run_session(&msgs);
    let provider = &response_result(&responses, 1)["capabilities"]["semanticTokensProvider"];

    assert_eq!(provider["legend"]["tokenTypes"], json!(["task"]));
    assert_eq!(provider["legend"]["tokenModifiers"], json!(["completed"]));
    assert_eq!(provider["full"], json!(true));
}

#[test]
fn semantic_tokens_marks_closed_task_titles() {
    let doc = concat!(
        "::: task\n",
        "Open.\n",
        ":::\n",
        "\n",
        "{done=\"2026-06-24T09:00:00Z\"}\n",
        "::: task\n",
        "Done task.\n",
        ":::\n",
        "\n",
        "{canceled=\"2026-06-24T09:00:00Z\"}\n",
        "::: task\n",
        "Canceled.\n",
        ":::\n",
        "\n",
        "- [x] Native done.\n",
    );
    let msgs = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{},"processId":null,"rootUri":null}}),
        json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///tasks.dj","languageId":"djot","version":1,"text":doc}}}),
        json!({"jsonrpc":"2.0","id":2,"method":"textDocument/semanticTokens/full","params":{"textDocument":{"uri":"file:///tasks.dj"}}}),
        json!({"jsonrpc":"2.0","id":3,"method":"shutdown","params":null}),
        json!({"jsonrpc":"2.0","method":"exit","params":null}),
    ];

    let responses = run_session(&msgs);
    let data = &response_result(&responses, 2)["data"];

    assert_eq!(
        data,
        &json!([
            6, 0, 10, 0, 1, // Done task.
            5, 0, 9, 0, 1, // Canceled.
            3, 6, 12, 0, 1, // Native done.
        ])
    );
}

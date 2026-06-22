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
fn diagnostics_report_invalid_recurring_task_metadata() {
    let doc = "{recur=\"P1W\"}\n::: task\nMissing due.\n:::\n\n{due=\"2026-06-21T09:00:00+08:00\" recur=\"P1M1D\"}\n::: task\nInvalid recur.\n:::\n\n{due=\"2026-06-21T09:00:00+08:00\" recur=\"P1W\"}\n::: task\nValid recur.\n:::\n";
    let msgs = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{},"processId":null,"rootUri":null}}),
        json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///tasks.dj","languageId":"djot","version":1,"text":doc}}}),
        json!({"jsonrpc":"2.0","id":99,"method":"shutdown","params":null}),
        json!({"jsonrpc":"2.0","method":"exit","params":null}),
    ];

    let responses = run_session(&msgs);
    let diagnostics = last_diagnostics(&responses);
    let messages = diagnostics
        .iter()
        .map(|diagnostic| diagnostic["message"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    let codes = diagnostics
        .iter()
        .map(|diagnostic| diagnostic["code"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();

    assert_eq!(diagnostics.len(), 2);
    assert!(codes.contains(&"missing-task-due-for-recur".to_string()));
    assert!(codes.contains(&"invalid-task-recur".to_string()));
    assert!(messages.contains(
        &"Recurring tasks with `recur` need a valid RFC 3339 `due` datetime.".to_string()
    ));
    assert!(messages.contains(&"Unsupported task `recur` value `P1M1D`. Use an ISO 8601 duration like `P1D`, `P1W`, `P1M`, or `P1Y`.".to_string()));
}

#[test]
fn diagnostics_report_duplicate_anchors() {
    let doc = "{#task}\n::: task\nFirst task.\n:::\n\n{#task}\n::: task\nSecond task.\n:::\n";
    let msgs = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{},"processId":null,"rootUri":null}}),
        json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///tasks.dj","languageId":"djot","version":1,"text":doc}}}),
        json!({"jsonrpc":"2.0","id":99,"method":"shutdown","params":null}),
        json!({"jsonrpc":"2.0","method":"exit","params":null}),
    ];

    let responses = run_session(&msgs);
    let diagnostics = last_diagnostics(&responses);

    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0]["code"], json!("duplicate-anchor"));
    assert_eq!(diagnostics[0]["message"], json!("Duplicate anchor `task`"));
    assert_eq!(
        diagnostics[0]["range"],
        json!({"start":{"line":5,"character":2},"end":{"line":5,"character":6}})
    );
    assert_eq!(
        diagnostics[0]["relatedInformation"],
        json!([{
            "location": {
                "uri": "file:///tasks.dj",
                "range": {"start":{"line":0,"character":2},"end":{"line":0,"character":6}}
            },
            "message": "First definition is here."
        }])
    );
}

#[test]
fn diagnostics_report_unresolved_task_prev_anchor() {
    let doc = "{prev=\"#missing-task\"}\n::: task\nFollow-up task.\n:::\n";
    let msgs = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{},"processId":null,"rootUri":null}}),
        json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///tasks.dj","languageId":"djot","version":1,"text":doc}}}),
        json!({"jsonrpc":"2.0","id":99,"method":"shutdown","params":null}),
        json!({"jsonrpc":"2.0","method":"exit","params":null}),
    ];

    let responses = run_session(&msgs);
    let diagnostics = last_diagnostics(&responses);

    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0]["code"], json!("unresolved-anchor"));
    assert_eq!(
        diagnostics[0]["message"],
        json!("Unresolved anchor `missing-task`")
    );
    assert_eq!(
        diagnostics[0]["range"],
        json!({"start":{"line":0,"character":7},"end":{"line":0,"character":20}})
    );
}

#[test]
fn diagnostics_report_blocked_task_as_hint() {
    let doc =
        "{#draft}\n::: task\nDraft.\n:::\n\n{#review depends=\"#draft\"}\n::: task\nReview.\n:::\n";
    let msgs = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{},"processId":null,"rootUri":null}}),
        json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///tasks.dj","languageId":"djot","version":1,"text":doc}}}),
        json!({"jsonrpc":"2.0","id":99,"method":"shutdown","params":null}),
        json!({"jsonrpc":"2.0","method":"exit","params":null}),
    ];

    let responses = run_session(&msgs);
    let diagnostics = last_diagnostics(&responses);
    let blocked = diagnostics
        .iter()
        .find(|diagnostic| diagnostic["code"] == json!("task-blocked"))
        .expect("missing blocked diagnostic");

    assert_eq!(blocked["severity"], json!(4));
    assert_eq!(blocked["message"], json!("Blocked by 1 open dependency."));
}

#[test]
fn diagnostics_report_task_prev_target_that_is_not_task() {
    let doc = "{#note}\nPlain anchor.\n\n{prev=\"#note\"}\n::: task\nFollow-up task.\n:::\n";
    let msgs = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{},"processId":null,"rootUri":null}}),
        json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///tasks.dj","languageId":"djot","version":1,"text":doc}}}),
        json!({"jsonrpc":"2.0","id":99,"method":"shutdown","params":null}),
        json!({"jsonrpc":"2.0","method":"exit","params":null}),
    ];

    let responses = run_session(&msgs);
    let diagnostics = last_diagnostics(&responses);

    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0]["code"], json!("invalid-task-prev-target"));
    assert_eq!(
        diagnostics[0]["message"],
        json!("Task `prev` target `note` must be a task.")
    );
    assert_eq!(
        diagnostics[0]["range"],
        json!({"start":{"line":3,"character":7},"end":{"line":3,"character":12}})
    );
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

//! End-to-end tests for `textDocument/codeAction`.

mod support;

use serde_json::json;

use support::run_session;

#[test]
fn code_action_converts_native_task_list_item_to_task_div() {
    let doc = "# Tasks\n\n- [ ] Write parser.\n";
    let msgs = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{},"processId":null,"rootUri":null}}),
        json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///tasks.dj","languageId":"djot","version":1,"text":doc}}}),
        json!({"jsonrpc":"2.0","id":2,"method":"textDocument/codeAction",
        "params":{
            "textDocument":{"uri":"file:///tasks.dj"},
            "range":{"start":{"line":2,"character":3},"end":{"line":2,"character":3}},
            "context":{"diagnostics":[]}
        }}),
        json!({"jsonrpc":"2.0","id":3,"method":"shutdown","params":null}),
        json!({"jsonrpc":"2.0","method":"exit","params":null}),
    ];

    let responses = run_session(&msgs);
    let actions = responses
        .iter()
        .find(|m| m["id"] == json!(2))
        .expect("no codeAction response")["result"]
        .as_array()
        .expect("result is not an array");
    assert_eq!(actions.len(), 1);

    let action = &actions[0];
    assert_eq!(action["title"], json!("Convert to task div"));
    assert_eq!(action["kind"], json!("refactor.rewrite"));

    let edit = &action["edit"]["changes"]["file:///tasks.dj"][0];
    assert_eq!(
        edit["range"],
        json!({"start":{"line":2,"character":0},"end":{"line":2,"character":19}})
    );

    let replacement = edit["newText"].as_str().expect("newText is not a string");
    assert!(replacement.starts_with("- {created=\""));
    assert!(replacement.contains("\"}\n  ::: task\n  Write parser.\n  :::"));
    assert_created_timestamp_shape(replacement);
}

fn assert_created_timestamp_shape(replacement: &str) {
    let timestamp = replacement
        .strip_prefix("- {created=\"")
        .and_then(|rest| rest.split_once("\"}"))
        .map(|(timestamp, _)| timestamp)
        .expect("missing created timestamp");

    assert_eq!(timestamp.len(), "2026-06-20T12:34:56Z".len());
    assert_eq!(&timestamp[4..5], "-");
    assert_eq!(&timestamp[7..8], "-");
    assert_eq!(&timestamp[10..11], "T");
    assert_eq!(&timestamp[13..14], ":");
    assert_eq!(&timestamp[16..17], ":");
    assert_eq!(&timestamp[19..20], "Z");
    assert!(timestamp[..4].chars().all(|c| c.is_ascii_digit()));
    assert!(timestamp[5..7].chars().all(|c| c.is_ascii_digit()));
    assert!(timestamp[8..10].chars().all(|c| c.is_ascii_digit()));
    assert!(timestamp[11..13].chars().all(|c| c.is_ascii_digit()));
    assert!(timestamp[14..16].chars().all(|c| c.is_ascii_digit()));
    assert!(timestamp[17..19].chars().all(|c| c.is_ascii_digit()));
}

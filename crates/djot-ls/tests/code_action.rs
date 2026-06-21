//! End-to-end tests for `textDocument/codeAction`.

mod support;

use serde_json::json;

use support::run_session;

#[test]
fn code_action_adds_metadata_at_start() {
    let doc = "\n\n# Heading\n";
    let msgs = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{},"processId":null,"rootUri":null}}),
        json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///notes/my-note.dj","languageId":"djot","version":1,"text":doc}}}),
        json!({"jsonrpc":"2.0","id":2,"method":"textDocument/codeAction",
        "params":{
            "textDocument":{"uri":"file:///notes/my-note.dj"},
            "range":{"start":{"line":1,"character":0},"end":{"line":1,"character":0}},
            "context":{"diagnostics":[],"only":["refactor.rewrite"]}
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
    assert_eq!(action["title"], json!("Add metadata"));
    assert_eq!(action["kind"], json!("refactor.rewrite"));

    let edit = &action["edit"]["changes"]["file:///notes/my-note.dj"][0];
    assert_eq!(
        edit["range"],
        json!({"start":{"line":0,"character":0},"end":{"line":0,"character":0}})
    );
    let new_text = edit["newText"].as_str().expect("newText is not a string");
    assert!(new_text.starts_with("{.metadata}\n``` toml\ntitle = \"my-note\"\ncreated = \""));
    assert!(new_text.ends_with("\"\n```\n\n"));
    assert_timestamp_shape_with_closing_quote(
        new_text,
        "{.metadata}\n``` toml\ntitle = \"my-note\"\ncreated = \"",
    );
}

#[test]
fn code_action_does_not_add_metadata_after_existing_block() {
    let doc = "# Heading\n\nBody\n";
    let msgs = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{},"processId":null,"rootUri":null}}),
        json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///notes/my-note.dj","languageId":"djot","version":1,"text":doc}}}),
        json!({"jsonrpc":"2.0","id":2,"method":"textDocument/codeAction",
        "params":{
            "textDocument":{"uri":"file:///notes/my-note.dj"},
            "range":{"start":{"line":2,"character":0},"end":{"line":2,"character":0}},
            "context":{"diagnostics":[],"only":["refactor.rewrite"]}
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
    assert!(actions.is_empty());
}

#[test]
fn code_action_does_not_add_metadata_when_metadata_exists() {
    let doc = "{.metadata}\n``` toml\ntitle = \"Existing\"\n```\n\n# Heading\n";
    let msgs = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{},"processId":null,"rootUri":null}}),
        json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///notes/my-note.dj","languageId":"djot","version":1,"text":doc}}}),
        json!({"jsonrpc":"2.0","id":2,"method":"textDocument/codeAction",
        "params":{
            "textDocument":{"uri":"file:///notes/my-note.dj"},
            "range":{"start":{"line":0,"character":0},"end":{"line":0,"character":0}},
            "context":{"diagnostics":[],"only":["refactor.rewrite"]}
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
    assert!(actions.is_empty());
}

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
    assert_timestamp_shape(replacement, "- {created=\"");
}

#[test]
fn code_action_marks_task_div_done() {
    let doc = "# Tasks\n\n::: task\nWrite parser.\n:::\n";
    let msgs = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{},"processId":null,"rootUri":null}}),
        json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///tasks.dj","languageId":"djot","version":1,"text":doc}}}),
        json!({"jsonrpc":"2.0","id":2,"method":"textDocument/codeAction",
        "params":{
            "textDocument":{"uri":"file:///tasks.dj"},
            "range":{"start":{"line":3,"character":2},"end":{"line":3,"character":2}},
            "context":{"diagnostics":[],"only":["quickfix"]}
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
    assert_eq!(action["title"], json!("Mark task done"));
    assert_eq!(action["kind"], json!("quickfix"));

    let edit = &action["edit"]["changes"]["file:///tasks.dj"][0];
    assert_eq!(
        edit["range"],
        json!({"start":{"line":2,"character":0},"end":{"line":2,"character":0}})
    );

    let inserted = edit["newText"].as_str().expect("newText is not a string");
    assert!(inserted.starts_with("{done=\""));
    assert!(inserted.ends_with("\"}\n"));
    assert_timestamp_shape(inserted, "{done=\"");
}

#[test]
fn code_action_marks_list_shaped_task_done() {
    let doc =
        "# Tasks\n\n- {created=\"2026-06-20T09:30:00Z\"}\n  ::: task\n  Write parser.\n  :::\n";
    let msgs = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{},"processId":null,"rootUri":null}}),
        json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///tasks.dj","languageId":"djot","version":1,"text":doc}}}),
        json!({"jsonrpc":"2.0","id":2,"method":"textDocument/codeAction",
        "params":{
            "textDocument":{"uri":"file:///tasks.dj"},
            "range":{"start":{"line":4,"character":4},"end":{"line":4,"character":4}},
            "context":{"diagnostics":[],"only":["quickfix"]}
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
    assert_eq!(action["title"], json!("Mark task done"));

    let edit = &action["edit"]["changes"]["file:///tasks.dj"][0];
    assert_eq!(
        edit["range"],
        json!({"start":{"line":3,"character":0},"end":{"line":3,"character":0}})
    );

    let inserted = edit["newText"].as_str().expect("newText is not a string");
    assert!(inserted.starts_with("  {done=\""));
    assert!(inserted.ends_with("\"}\n"));
    assert_timestamp_shape(inserted, "  {done=\"");
}

#[test]
fn code_action_marks_recurring_task_done_and_creates_next_instance() {
    let doc = "# Tasks\n\n{due=\"2026-06-21T17:00:00+08:00\" recur=\"P1W\"}\n::: task\nWeekly review.\n:::\n";
    let msgs = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{},"processId":null,"rootUri":null}}),
        json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///tasks.dj","languageId":"djot","version":1,"text":doc}}}),
        json!({"jsonrpc":"2.0","id":2,"method":"textDocument/codeAction",
        "params":{
            "textDocument":{"uri":"file:///tasks.dj"},
            "range":{"start":{"line":4,"character":2},"end":{"line":4,"character":2}},
            "context":{"diagnostics":[],"only":["quickfix"]}
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
    assert_eq!(action["title"], json!("Mark task done"));
    assert_eq!(action["kind"], json!("quickfix"));

    let edits = action["edit"]["changes"]["file:///tasks.dj"]
        .as_array()
        .expect("changes is not an array");
    assert_eq!(edits.len(), 2);
    assert_eq!(
        edits[0]["range"],
        json!({"start":{"line":3,"character":0},"end":{"line":3,"character":0}})
    );
    let done_insert = edits[0]["newText"]
        .as_str()
        .expect("newText is not a string");
    assert!(done_insert.starts_with("{#Weekly-review-2026-06-21}\n{done=\""));
    assert_timestamp_shape(
        done_insert
            .strip_prefix("{#Weekly-review-2026-06-21}\n")
            .unwrap(),
        "{done=\"",
    );

    assert_eq!(
        edits[1]["range"],
        json!({"start":{"line":6,"character":0},"end":{"line":6,"character":0}})
    );
    let next_insert = edits[1]["newText"]
        .as_str()
        .expect("newText is not a string");
    assert!(next_insert.contains("{#Weekly-review-2026-06-28}\n"));
    assert!(next_insert.contains("{created=\"20"));
    assert!(next_insert.contains(
        " due=\"2026-06-28T17:00:00+08:00\" recur=\"P1W\" prev=\"#Weekly-review-2026-06-21\"}"
    ));
    assert!(next_insert.contains("::: task\nWeekly review.\n:::\n"));
}

#[test]
fn code_action_marks_indented_recurring_task_done_and_creates_next_instance() {
    let doc = "# Tasks\n\n- {created=\"2026-06-20T09:30:00Z\"}\n  {due=\"2026-06-21T17:00:00+08:00\" recur=\"P1D\"}\n  {#daily-review}\n  ::: task\n  Daily review.\n  :::\n";
    let msgs = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{},"processId":null,"rootUri":null}}),
        json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///tasks.dj","languageId":"djot","version":1,"text":doc}}}),
        json!({"jsonrpc":"2.0","id":2,"method":"textDocument/codeAction",
        "params":{
            "textDocument":{"uri":"file:///tasks.dj"},
            "range":{"start":{"line":6,"character":4},"end":{"line":6,"character":4}},
            "context":{"diagnostics":[],"only":["quickfix"]}
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

    let edits = actions[0]["edit"]["changes"]["file:///tasks.dj"]
        .as_array()
        .expect("changes is not an array");
    assert_eq!(edits.len(), 2);
    assert_eq!(
        edits[0]["range"],
        json!({"start":{"line":5,"character":0},"end":{"line":5,"character":0}})
    );
    let done_insert = edits[0]["newText"]
        .as_str()
        .expect("newText is not a string");
    assert!(done_insert.starts_with("  {done=\""));
    assert_timestamp_shape(done_insert, "  {done=\"");

    assert_eq!(
        edits[1]["range"],
        json!({"start":{"line":8,"character":0},"end":{"line":8,"character":0}})
    );
    let next_insert = edits[1]["newText"]
        .as_str()
        .expect("newText is not a string");
    assert!(next_insert.contains("\n\n  {#Daily-review-2026-06-22}\n"));
    assert!(next_insert.contains("  {created=\"20"));
    assert!(next_insert
        .contains(" due=\"2026-06-22T17:00:00+08:00\" recur=\"P1D\" prev=\"#daily-review\"}"));
    assert!(next_insert.contains("  ::: task\n  Daily review.\n  :::\n"));
}

fn assert_timestamp_shape(replacement: &str, prefix: &str) {
    let timestamp = replacement
        .strip_prefix(prefix)
        .and_then(|rest| rest.split_once("\"}"))
        .map(|(timestamp, _)| timestamp)
        .expect("missing timestamp");

    assert!(
        timestamp.len() == "2026-06-20T12:34:56Z".len()
            || timestamp.len() == "2026-06-20T12:34:56+08:00".len()
    );
    assert_eq!(&timestamp[4..5], "-");
    assert_eq!(&timestamp[7..8], "-");
    assert_eq!(&timestamp[10..11], "T");
    assert_eq!(&timestamp[13..14], ":");
    assert_eq!(&timestamp[16..17], ":");
    assert_timezone_shape(timestamp);
    assert!(timestamp[..4].chars().all(|c| c.is_ascii_digit()));
    assert!(timestamp[5..7].chars().all(|c| c.is_ascii_digit()));
    assert!(timestamp[8..10].chars().all(|c| c.is_ascii_digit()));
    assert!(timestamp[11..13].chars().all(|c| c.is_ascii_digit()));
    assert!(timestamp[14..16].chars().all(|c| c.is_ascii_digit()));
    assert!(timestamp[17..19].chars().all(|c| c.is_ascii_digit()));
}

fn assert_timestamp_shape_with_closing_quote(replacement: &str, prefix: &str) {
    let timestamp = replacement
        .strip_prefix(prefix)
        .and_then(|rest| rest.split_once('"'))
        .map(|(timestamp, _)| timestamp)
        .expect("missing timestamp");

    assert!(
        timestamp.len() == "2026-06-20T12:34:56Z".len()
            || timestamp.len() == "2026-06-20T12:34:56+08:00".len()
    );
    assert_eq!(&timestamp[4..5], "-");
    assert_eq!(&timestamp[7..8], "-");
    assert_eq!(&timestamp[10..11], "T");
    assert_eq!(&timestamp[13..14], ":");
    assert_eq!(&timestamp[16..17], ":");
    assert_timezone_shape(timestamp);
    assert!(timestamp[..4].chars().all(|c| c.is_ascii_digit()));
    assert!(timestamp[5..7].chars().all(|c| c.is_ascii_digit()));
    assert!(timestamp[8..10].chars().all(|c| c.is_ascii_digit()));
    assert!(timestamp[11..13].chars().all(|c| c.is_ascii_digit()));
    assert!(timestamp[14..16].chars().all(|c| c.is_ascii_digit()));
    assert!(timestamp[17..19].chars().all(|c| c.is_ascii_digit()));
}

fn assert_timezone_shape(timestamp: &str) {
    match timestamp.len() {
        20 => assert_eq!(&timestamp[19..20], "Z"),
        25 => {
            assert!(&timestamp[19..20] == "+" || &timestamp[19..20] == "-");
            assert!(timestamp[20..22].chars().all(|c| c.is_ascii_digit()));
            assert_eq!(&timestamp[22..23], ":");
            assert!(timestamp[23..25].chars().all(|c| c.is_ascii_digit()));
        }
        _ => panic!("unexpected timestamp length: {timestamp}"),
    }
}

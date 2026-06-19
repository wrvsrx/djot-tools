//! End-to-end tests for `textDocument/completion`.

mod support;

use lsp_types::Url;
use serde_json::{json, Value};

use support::run_session;

#[test]
fn completion_after_open_bracket_inserts_title_link() {
    let dir = std::env::temp_dir().join("djot-ls-completion-label-test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let a = dir.join("a.dj");
    let usage = dir.join("usage.dj");
    let doc_a = "# A\n\n[Us";
    std::fs::write(&a, doc_a).unwrap();
    std::fs::write(
        &usage,
        "{.metadata}\n``` toml\ntitle = \"Usage Guide\"\n```\n\n# Usage\n",
    )
    .unwrap();

    let root_uri = Url::from_directory_path(&dir).unwrap().to_string();
    let a_uri = Url::from_file_path(&a).unwrap().to_string();
    let msgs = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{},"processId":null,"rootUri":root_uri}}),
        json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
        json!({"jsonrpc":"2.0","id":2,"method":"textDocument/completion",
               "params":{"textDocument":{"uri":a_uri},"position":{"line":2,"character":3}}}),
        json!({"jsonrpc":"2.0","id":99,"method":"shutdown","params":null}),
        json!({"jsonrpc":"2.0","method":"exit","params":null}),
    ];

    let responses = run_session(&msgs);
    let item = completion_item(&responses, 2, "Usage Guide");

    assert_eq!(item["detail"], json!("usage.dj"));
    assert_eq!(
        item["textEdit"]["newText"],
        json!("[Usage Guide](usage.dj)")
    );
    assert_eq!(
        item["textEdit"]["range"]["start"],
        json!({"line":2,"character":0})
    );
    assert_eq!(
        item["textEdit"]["range"]["end"],
        json!({"line":2,"character":3})
    );
}

#[test]
fn completion_replaces_closing_bracket_after_label_cursor() {
    let dir = std::env::temp_dir().join("djot-ls-completion-label-bracket-test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let a = dir.join("a.dj");
    let usage = dir.join("usage.dj");
    let doc_a = "# A\n\n[Us]";
    std::fs::write(&a, doc_a).unwrap();
    std::fs::write(
        &usage,
        "{.metadata}\n``` toml\ntitle = \"Usage Guide\"\n```\n\n# Usage\n",
    )
    .unwrap();

    let root_uri = Url::from_directory_path(&dir).unwrap().to_string();
    let a_uri = Url::from_file_path(&a).unwrap().to_string();
    let msgs = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{},"processId":null,"rootUri":root_uri}}),
        json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
        json!({"jsonrpc":"2.0","id":2,"method":"textDocument/completion",
               "params":{"textDocument":{"uri":a_uri},"position":{"line":2,"character":3}}}),
        json!({"jsonrpc":"2.0","id":99,"method":"shutdown","params":null}),
        json!({"jsonrpc":"2.0","method":"exit","params":null}),
    ];

    let responses = run_session(&msgs);
    let item = completion_item(&responses, 2, "Usage Guide");

    assert_eq!(
        item["textEdit"]["newText"],
        json!("[Usage Guide](usage.dj)")
    );
    assert_eq!(
        item["textEdit"]["range"]["start"],
        json!({"line":2,"character":0})
    );
    assert_eq!(
        item["textEdit"]["range"]["end"],
        json!({"line":2,"character":4})
    );
}

#[test]
fn completion_inside_link_destination_inserts_path() {
    let dir = std::env::temp_dir().join("djot-ls-completion-path-test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let a = dir.join("a.dj");
    let usage = dir.join("usage.dj");
    let doc_a = "# A\n\n[read](us";
    std::fs::write(&a, doc_a).unwrap();
    std::fs::write(
        &usage,
        "{.metadata}\n``` toml\ntitle = \"Usage Guide\"\n```\n\n# Usage\n",
    )
    .unwrap();

    let root_uri = Url::from_directory_path(&dir).unwrap().to_string();
    let a_uri = Url::from_file_path(&a).unwrap().to_string();
    let msgs = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{},"processId":null,"rootUri":root_uri}}),
        json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
        json!({"jsonrpc":"2.0","id":2,"method":"textDocument/completion",
               "params":{"textDocument":{"uri":a_uri},"position":{"line":2,"character":9}}}),
        json!({"jsonrpc":"2.0","id":99,"method":"shutdown","params":null}),
        json!({"jsonrpc":"2.0","method":"exit","params":null}),
    ];

    let responses = run_session(&msgs);
    let item = completion_item(&responses, 2, "usage.dj");

    assert_eq!(item["detail"], json!("Usage Guide"));
    assert_eq!(item["textEdit"]["newText"], json!("usage.dj"));
    assert_eq!(
        item["textEdit"]["range"]["start"],
        json!({"line":2,"character":7})
    );
    assert_eq!(
        item["textEdit"]["range"]["end"],
        json!({"line":2,"character":9})
    );
}

#[test]
fn completion_from_subdirectory_inserts_relative_path() {
    let dir = std::env::temp_dir().join("djot-ls-completion-relative-path-test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("a")).unwrap();
    std::fs::create_dir_all(dir.join("b")).unwrap();
    let source = dir.join("b").join("b.dj");
    let target = dir.join("a").join("a.dj");
    let doc_source = "# B\n\n[Target";
    std::fs::write(&source, doc_source).unwrap();
    std::fs::write(
        &target,
        "{.metadata}\n``` toml\ntitle = \"Target A\"\n```\n\n# A\n",
    )
    .unwrap();

    let root_uri = Url::from_directory_path(&dir).unwrap().to_string();
    let source_uri = Url::from_file_path(&source).unwrap().to_string();
    let msgs = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{},"processId":null,"rootUri":root_uri}}),
        json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
        json!({"jsonrpc":"2.0","id":2,"method":"textDocument/completion",
               "params":{"textDocument":{"uri":source_uri},"position":{"line":2,"character":7}}}),
        json!({"jsonrpc":"2.0","id":99,"method":"shutdown","params":null}),
        json!({"jsonrpc":"2.0","method":"exit","params":null}),
    ];

    let responses = run_session(&msgs);
    let item = completion_item(&responses, 2, "Target A");

    assert_eq!(item["detail"], json!("../a/a.dj"));
    assert_eq!(item["textEdit"]["newText"], json!("[Target A](../a/a.dj)"));
}

#[test]
fn completion_inside_closed_empty_destination_inserts_path() {
    let dir = std::env::temp_dir().join("djot-ls-completion-closed-empty-path-test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let a = dir.join("a.dj");
    let usage = dir.join("usage.dj");
    let doc_a = "# A\n\n[read]()";
    std::fs::write(&a, doc_a).unwrap();
    std::fs::write(
        &usage,
        "{.metadata}\n``` toml\ntitle = \"Usage Guide\"\n```\n\n# Usage\n",
    )
    .unwrap();

    let root_uri = Url::from_directory_path(&dir).unwrap().to_string();
    let a_uri = Url::from_file_path(&a).unwrap().to_string();
    let msgs = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{},"processId":null,"rootUri":root_uri}}),
        json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
        json!({"jsonrpc":"2.0","id":2,"method":"textDocument/completion",
               "params":{"textDocument":{"uri":a_uri},"position":{"line":2,"character":7}}}),
        json!({"jsonrpc":"2.0","id":99,"method":"shutdown","params":null}),
        json!({"jsonrpc":"2.0","method":"exit","params":null}),
    ];

    let responses = run_session(&msgs);
    let item = completion_item(&responses, 2, "usage.dj");

    assert_eq!(item["detail"], json!("Usage Guide"));
    assert_eq!(item["textEdit"]["newText"], json!("usage.dj"));
    assert_eq!(
        item["textEdit"]["range"]["start"],
        json!({"line":2,"character":7})
    );
    assert_eq!(
        item["textEdit"]["range"]["end"],
        json!({"line":2,"character":7})
    );
}

#[test]
fn completion_inside_internal_anchor_inserts_anchor_id() {
    let dir = std::env::temp_dir().join("djot-ls-completion-internal-anchor-test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let a = dir.join("a.dj");
    let doc_a = "# A\n\n[read](#Us\n\n## Usage Guide\n";
    std::fs::write(&a, doc_a).unwrap();

    let root_uri = Url::from_directory_path(&dir).unwrap().to_string();
    let a_uri = Url::from_file_path(&a).unwrap().to_string();
    let msgs = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{},"processId":null,"rootUri":root_uri}}),
        json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
        json!({"jsonrpc":"2.0","id":2,"method":"textDocument/completion",
               "params":{"textDocument":{"uri":a_uri},"position":{"line":2,"character":10}}}),
        json!({"jsonrpc":"2.0","id":99,"method":"shutdown","params":null}),
        json!({"jsonrpc":"2.0","method":"exit","params":null}),
    ];

    let responses = run_session(&msgs);
    let item = completion_item(&responses, 2, "Usage-Guide");

    assert_eq!(item["detail"], json!("a.dj"));
    assert_eq!(item["textEdit"]["newText"], json!("Usage-Guide"));
    assert_eq!(
        item["textEdit"]["range"]["start"],
        json!({"line":2,"character":8})
    );
    assert_eq!(
        item["textEdit"]["range"]["end"],
        json!({"line":2,"character":10})
    );
}

#[test]
fn completion_inside_external_anchor_inserts_anchor_id() {
    let dir = std::env::temp_dir().join("djot-ls-completion-external-anchor-test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let a = dir.join("a.dj");
    let usage = dir.join("usage.dj");
    let doc_a = "# A\n\n[read](usage.dj#Us";
    std::fs::write(&a, doc_a).unwrap();
    std::fs::write(&usage, "# Intro\n\n## Usage Guide\n").unwrap();

    let root_uri = Url::from_directory_path(&dir).unwrap().to_string();
    let a_uri = Url::from_file_path(&a).unwrap().to_string();
    let msgs = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{},"processId":null,"rootUri":root_uri}}),
        json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
        json!({"jsonrpc":"2.0","id":2,"method":"textDocument/completion",
               "params":{"textDocument":{"uri":a_uri},"position":{"line":2,"character":18}}}),
        json!({"jsonrpc":"2.0","id":99,"method":"shutdown","params":null}),
        json!({"jsonrpc":"2.0","method":"exit","params":null}),
    ];

    let responses = run_session(&msgs);
    let item = completion_item(&responses, 2, "Usage-Guide");

    assert_eq!(item["detail"], json!("usage.dj"));
    assert_eq!(item["textEdit"]["newText"], json!("Usage-Guide"));
    assert_eq!(
        item["textEdit"]["range"]["start"],
        json!({"line":2,"character":16})
    );
    assert_eq!(
        item["textEdit"]["range"]["end"],
        json!({"line":2,"character":18})
    );
}

#[test]
fn completion_inside_closed_internal_anchor_inserts_anchor_id() {
    let dir = std::env::temp_dir().join("djot-ls-completion-closed-internal-anchor-test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let a = dir.join("a.dj");
    let doc_a = "# A\n\n[read](#)\n\n## Usage Guide\n";
    std::fs::write(&a, doc_a).unwrap();

    let root_uri = Url::from_directory_path(&dir).unwrap().to_string();
    let a_uri = Url::from_file_path(&a).unwrap().to_string();
    let msgs = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{},"processId":null,"rootUri":root_uri}}),
        json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
        json!({"jsonrpc":"2.0","id":2,"method":"textDocument/completion",
               "params":{"textDocument":{"uri":a_uri},"position":{"line":2,"character":8}}}),
        json!({"jsonrpc":"2.0","id":99,"method":"shutdown","params":null}),
        json!({"jsonrpc":"2.0","method":"exit","params":null}),
    ];

    let responses = run_session(&msgs);
    let item = completion_item(&responses, 2, "Usage-Guide");

    assert_eq!(item["detail"], json!("a.dj"));
    assert_eq!(item["textEdit"]["newText"], json!("Usage-Guide"));
    assert_eq!(
        item["textEdit"]["range"]["start"],
        json!({"line":2,"character":8})
    );
    assert_eq!(
        item["textEdit"]["range"]["end"],
        json!({"line":2,"character":8})
    );
}

#[test]
fn completion_inside_closed_external_anchor_inserts_anchor_id() {
    let dir = std::env::temp_dir().join("djot-ls-completion-closed-external-anchor-test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let a = dir.join("a.dj");
    let usage = dir.join("usage.dj");
    let doc_a = "# A\n\n[read](usage.dj#)";
    std::fs::write(&a, doc_a).unwrap();
    std::fs::write(&usage, "# Intro\n\n## Usage Guide\n").unwrap();

    let root_uri = Url::from_directory_path(&dir).unwrap().to_string();
    let a_uri = Url::from_file_path(&a).unwrap().to_string();
    let msgs = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{},"processId":null,"rootUri":root_uri}}),
        json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
        json!({"jsonrpc":"2.0","id":2,"method":"textDocument/completion",
               "params":{"textDocument":{"uri":a_uri},"position":{"line":2,"character":16}}}),
        json!({"jsonrpc":"2.0","id":99,"method":"shutdown","params":null}),
        json!({"jsonrpc":"2.0","method":"exit","params":null}),
    ];

    let responses = run_session(&msgs);
    let item = completion_item(&responses, 2, "Usage-Guide");

    assert_eq!(item["detail"], json!("usage.dj"));
    assert_eq!(item["textEdit"]["newText"], json!("Usage-Guide"));
    assert_eq!(
        item["textEdit"]["range"]["start"],
        json!({"line":2,"character":16})
    );
    assert_eq!(
        item["textEdit"]["range"]["end"],
        json!({"line":2,"character":16})
    );
}

#[test]
fn completion_does_not_run_inside_inline_code() {
    let dir = std::env::temp_dir().join("djot-ls-completion-code-test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let a = dir.join("a.dj");
    let usage = dir.join("usage.dj");
    let doc_a = "# A\n\n`[Us`";
    std::fs::write(&a, doc_a).unwrap();
    std::fs::write(
        &usage,
        "{.metadata}\n``` toml\ntitle = \"Usage Guide\"\n```\n\n# Usage\n",
    )
    .unwrap();

    let root_uri = Url::from_directory_path(&dir).unwrap().to_string();
    let a_uri = Url::from_file_path(&a).unwrap().to_string();
    let msgs = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{},"processId":null,"rootUri":root_uri}}),
        json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
        json!({"jsonrpc":"2.0","id":2,"method":"textDocument/completion",
               "params":{"textDocument":{"uri":a_uri},"position":{"line":2,"character":4}}}),
        json!({"jsonrpc":"2.0","id":99,"method":"shutdown","params":null}),
        json!({"jsonrpc":"2.0","method":"exit","params":null}),
    ];

    let responses = run_session(&msgs);
    let response = responses
        .iter()
        .find(|m| m["id"] == json!(2))
        .expect("no completion response for id 2");
    assert!(response["result"].is_null());
}

fn completion_item<'a>(responses: &'a [Value], id: i64, label: &str) -> &'a Value {
    let items = responses
        .iter()
        .find(|m| m["id"] == json!(id))
        .unwrap_or_else(|| panic!("no completion response for id {id}"))["result"]
        .as_array()
        .expect("completion result is not an array");
    items
        .iter()
        .find(|item| item["label"] == json!(label))
        .unwrap_or_else(|| panic!("no completion item with label {label:?}"))
}

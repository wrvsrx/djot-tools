//! End-to-end tests for `textDocument/rename`.

mod support;

use lsp_types::Url;
use serde_json::{json, Value};

use support::run_session;

#[test]
fn rename_anchor_updates_declaration_and_workspace_references() {
    let dir = std::env::temp_dir().join("djot-ls-rename-anchor-test");
    std::fs::create_dir_all(&dir).unwrap();
    let a = dir.join("a.dj");
    let b = dir.join("b.dj");
    let doc_a = "# A\n\nsee [topic](b.dj#topic)\n";
    let doc_b = "{#topic}\nTopic\n\n[local](#topic)\n";
    std::fs::write(&a, doc_a).unwrap();
    std::fs::write(&b, doc_b).unwrap();

    let root_uri = Url::from_directory_path(&dir).unwrap().to_string();
    let a_uri = Url::from_file_path(&a).unwrap().to_string();
    let topic_col = (doc_a.lines().nth(2).unwrap().find("#topic").unwrap() + 1) as i64;
    let position = json!({"line":2,"character":topic_col});
    let text_document = json!({"uri":a_uri});

    let msgs = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{},"processId":null,"rootUri":root_uri}}),
        json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
        json!({"jsonrpc":"2.0","id":2,"method":"textDocument/prepareRename",
        "params":{"textDocument":text_document.clone(),"position":position.clone()}}),
        json!({"jsonrpc":"2.0","id":3,"method":"textDocument/rename",
        "params":{"textDocument":text_document,"position":position,"newName":"Renamed"}}),
        json!({"jsonrpc":"2.0","id":99,"method":"shutdown","params":null}),
        json!({"jsonrpc":"2.0","method":"exit","params":null}),
    ];

    let responses = run_session(&msgs);
    let prepare = response_result(&responses, 2);
    assert_eq!(prepare["placeholder"], json!("topic"));
    assert_eq!(
        prepare["range"]["start"],
        json!({"line":2,"character":topic_col})
    );
    assert_eq!(
        prepare["range"]["end"],
        json!({"line":2,"character":topic_col + "topic".len() as i64})
    );

    assert_eq!(
        sorted_edits(response_result(&responses, 3)),
        vec![
            (
                "a.dj".to_string(),
                2,
                topic_col as u64,
                "Renamed".to_string()
            ),
            ("b.dj".to_string(), 0, 2, "Renamed".to_string()),
            ("b.dj".to_string(), 3, 9, "Renamed".to_string()),
        ]
    );
}

#[test]
fn rename_link_path_renames_file_and_updates_workspace_links() {
    let dir = std::env::temp_dir().join("djot-ls-rename-link-path-test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    let a = dir.join("a.dj");
    let b = dir.join("b.dj");
    let c = dir.join("sub").join("c.dj");
    let renamed = dir.join("renamed.dj");
    let doc_a = "# A\n\nsee [topic](b.dj#topic)\n";
    let doc_c = "# C\n\nsee [topic](../b.dj)\n";
    std::fs::write(&a, doc_a).unwrap();
    std::fs::write(&b, "{#topic}\nTopic\n").unwrap();
    std::fs::write(&c, doc_c).unwrap();

    let root_uri = Url::from_directory_path(&dir).unwrap().to_string();
    let a_uri = Url::from_file_path(&a).unwrap().to_string();
    let b_uri = Url::from_file_path(&b).unwrap().to_string();
    let renamed_uri = Url::from_file_path(&renamed).unwrap().to_string();
    let path_col = doc_a.lines().nth(2).unwrap().find("b.dj").unwrap() as i64;
    let position = json!({"line":2,"character":path_col});
    let text_document = json!({"uri":a_uri});

    let msgs = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
            "capabilities":{"workspace":{"workspaceEdit":{"documentChanges":true,"resourceOperations":["rename"]}}},
            "processId":null,
            "rootUri":root_uri
        }}),
        json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
        json!({"jsonrpc":"2.0","id":2,"method":"textDocument/prepareRename",
        "params":{"textDocument":text_document.clone(),"position":position.clone()}}),
        json!({"jsonrpc":"2.0","id":3,"method":"textDocument/rename",
        "params":{"textDocument":text_document,"position":position,"newName":"renamed.dj"}}),
        json!({"jsonrpc":"2.0","id":99,"method":"shutdown","params":null}),
        json!({"jsonrpc":"2.0","method":"exit","params":null}),
    ];

    let responses = run_session(&msgs);
    let prepare = response_result(&responses, 2);
    assert_eq!(prepare["placeholder"], json!("b.dj"));
    assert_eq!(
        prepare["range"]["start"],
        json!({"line":2,"character":path_col})
    );
    assert_eq!(
        prepare["range"]["end"],
        json!({"line":2,"character":path_col + "b.dj".len() as i64})
    );

    let result = response_result(&responses, 3);
    let changes = result["documentChanges"]
        .as_array()
        .expect("documentChanges is not an array");
    assert_eq!(changes[0]["kind"], json!("rename"));
    assert_eq!(changes[0]["oldUri"], json!(b_uri));
    assert_eq!(changes[0]["newUri"], json!(renamed_uri));
    assert_eq!(
        sorted_document_change_edits(result),
        vec![
            (
                "a.dj".to_string(),
                2,
                path_col as u64,
                "renamed.dj".to_string()
            ),
            (
                "c.dj".to_string(),
                2,
                doc_c.lines().nth(2).unwrap().find("../b.dj").unwrap() as u64,
                "../renamed.dj".to_string(),
            ),
        ]
    );
}

#[test]
fn rename_rejects_implicit_heading_anchor() {
    let dir = std::env::temp_dir().join("djot-ls-rename-implicit-heading-test");
    std::fs::create_dir_all(&dir).unwrap();
    let a = dir.join("a.dj");
    let b = dir.join("b.dj");
    let doc_a = "# A\n\nsee [topic](b.dj#Topic)\n";
    let doc_b = "# Topic\n";
    std::fs::write(&a, doc_a).unwrap();
    std::fs::write(&b, doc_b).unwrap();

    let root_uri = Url::from_directory_path(&dir).unwrap().to_string();
    let a_uri = Url::from_file_path(&a).unwrap().to_string();
    let topic_col = doc_a.lines().nth(2).unwrap().find("Topic").unwrap() as i64;
    let position = json!({"line":2,"character":topic_col});
    let text_document = json!({"uri":a_uri});

    let msgs = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{},"processId":null,"rootUri":root_uri}}),
        json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
        json!({"jsonrpc":"2.0","id":2,"method":"textDocument/prepareRename",
        "params":{"textDocument":text_document.clone(),"position":position.clone()}}),
        json!({"jsonrpc":"2.0","id":3,"method":"textDocument/rename",
        "params":{"textDocument":text_document,"position":position,"newName":"Renamed"}}),
        json!({"jsonrpc":"2.0","id":99,"method":"shutdown","params":null}),
        json!({"jsonrpc":"2.0","method":"exit","params":null}),
    ];

    let responses = run_session(&msgs);
    assert_eq!(
        response_error_message(&responses, 2),
        "Renaming implicit heading anchors is not supported yet; add an explicit {#id} anchor and rename that instead."
    );
    assert_eq!(
        response_error_message(&responses, 3),
        "Renaming implicit heading anchors is not supported yet; add an explicit {#id} anchor and rename that instead."
    );
}

fn response_result(responses: &[Value], id: i64) -> &Value {
    &responses
        .iter()
        .find(|message| message["id"] == json!(id))
        .unwrap_or_else(|| panic!("no response for id {id}"))["result"]
}

fn response_error_message(responses: &[Value], id: i64) -> &str {
    responses
        .iter()
        .find(|message| message["id"] == json!(id))
        .unwrap_or_else(|| panic!("no response for id {id}"))["error"]["message"]
        .as_str()
        .expect("error message is not a string")
}

fn sorted_edits(edit: &Value) -> Vec<(String, u64, u64, String)> {
    let changes = edit["changes"]
        .as_object()
        .expect("changes is not an object");
    let mut out = Vec::new();
    for (uri, edits) in changes {
        let filename = uri.rsplit('/').next().unwrap().to_string();
        for edit in edits.as_array().expect("edits is not an array") {
            out.push((
                filename.clone(),
                edit["range"]["start"]["line"].as_u64().unwrap(),
                edit["range"]["start"]["character"].as_u64().unwrap(),
                edit["newText"].as_str().unwrap().to_string(),
            ));
        }
    }
    out.sort_unstable();
    out
}

fn sorted_document_change_edits(edit: &Value) -> Vec<(String, u64, u64, String)> {
    let changes = edit["documentChanges"]
        .as_array()
        .expect("documentChanges is not an array");
    let mut out = Vec::new();
    for change in changes {
        let Some(text_document) = change.get("textDocument") else {
            continue;
        };
        let uri = text_document["uri"].as_str().unwrap();
        let filename = uri.rsplit('/').next().unwrap().to_string();
        for edit in change["edits"].as_array().expect("edits is not an array") {
            out.push((
                filename.clone(),
                edit["range"]["start"]["line"].as_u64().unwrap(),
                edit["range"]["start"]["character"].as_u64().unwrap(),
                edit["newText"].as_str().unwrap().to_string(),
            ));
        }
    }
    out.sort_unstable();
    out
}

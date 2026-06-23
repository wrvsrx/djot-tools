//! End-to-end tests for `textDocument/rename`.

mod support;

use serde_json::{json, Value};

use support::{
    diagnostics_for, dir_uri, file_uri, response_error_message, response_result, run_session,
    run_session_with_pause, temp_dir,
};

#[test]
fn rename_anchor_updates_declaration_and_workspace_references() {
    let dir = temp_dir("djot-ls-rename-anchor-test");
    let a = dir.join("a.dj");
    let b = dir.join("b.dj");
    let doc_a = "# A\n\nsee [topic](b.dj#topic)\n";
    let doc_b = "{#topic}\nTopic\n\n[local](#topic)\n";
    std::fs::write(&a, doc_a).unwrap();
    std::fs::write(&b, doc_b).unwrap();

    let root_uri = dir_uri(&dir);
    let a_uri = file_uri(&a);
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
fn rename_reference_updates_declaration_and_all_references() {
    let dir = temp_dir("djot-ls-rename-reference-all-test");
    let doc = dir.join("doc.dj");
    let text = "{#xxx}\nAnchor\n\n[first](#xxx)\n\n[second](#xxx)\n";
    std::fs::write(&doc, text).unwrap();

    let root_uri = dir_uri(&dir);
    let doc_uri = file_uri(&doc);
    let id_col = (text.lines().nth(3).unwrap().find("#xxx").unwrap() + 1) as i64;
    let second_id_col = (text.lines().nth(5).unwrap().find("#xxx").unwrap() + 1) as i64;
    let position = json!({"line":3,"character":id_col});
    let text_document = json!({"uri":doc_uri});

    let msgs = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{},"processId":null,"rootUri":root_uri}}),
        json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
        json!({"jsonrpc":"2.0","id":2,"method":"textDocument/prepareRename",
        "params":{"textDocument":text_document.clone(),"position":position.clone()}}),
        json!({"jsonrpc":"2.0","id":3,"method":"textDocument/rename",
        "params":{"textDocument":text_document,"position":position,"newName":"yyy"}}),
        json!({"jsonrpc":"2.0","id":99,"method":"shutdown","params":null}),
        json!({"jsonrpc":"2.0","method":"exit","params":null}),
    ];

    let responses = run_session(&msgs);
    let prepare = response_result(&responses, 2);
    assert_eq!(prepare["placeholder"], json!("xxx"));
    assert_eq!(
        sorted_edits(response_result(&responses, 3)),
        vec![
            ("doc.dj".to_string(), 0, 2, "yyy".to_string()),
            ("doc.dj".to_string(), 3, id_col as u64, "yyy".to_string()),
            (
                "doc.dj".to_string(),
                5,
                second_id_col as u64,
                "yyy".to_string()
            ),
        ]
    );
}

#[test]
fn rename_link_path_renames_file_and_updates_workspace_links() {
    let dir = temp_dir("djot-ls-rename-link-path-test");
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

    let root_uri = dir_uri(&dir);
    let a_uri = file_uri(&a);
    let b_uri = file_uri(&b);
    let renamed_uri = file_uri(&renamed);
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
fn rename_link_path_handles_spaces_and_nested_relative_links() {
    let dir = temp_dir("djot-ls-rename-link-path-spaces-test");
    std::fs::create_dir_all(dir.join("nested")).unwrap();
    std::fs::create_dir_all(dir.join("archive")).unwrap();
    let index = dir.join("index.dj");
    let project = dir.join("Project Plan.dj");
    let nested = dir.join("nested").join("notes.dj");
    let renamed = dir.join("archive").join("Project Plan.dj");
    let doc_index = "# Index\n\n[review](Project Plan.dj#review)\n";
    let doc_nested = "# Nested\n\n[review](../Project Plan.dj#review)\n";
    std::fs::write(&index, doc_index).unwrap();
    std::fs::write(&project, "{#review}\nReview\n").unwrap();
    std::fs::write(&nested, doc_nested).unwrap();

    let root_uri = dir_uri(&dir);
    let index_uri = file_uri(&index);
    let project_uri = file_uri(&project);
    let renamed_uri = file_uri(&renamed);
    let path_col = doc_index
        .lines()
        .nth(2)
        .unwrap()
        .find("Project Plan.dj")
        .unwrap() as i64;
    let position = json!({"line":2,"character":path_col});
    let text_document = json!({"uri":index_uri});

    let msgs = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
            "capabilities":{"workspace":{"workspaceEdit":{"documentChanges":true,"resourceOperations":["rename"]}}},
            "processId":null,
            "rootUri":root_uri
        }}),
        json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
        json!({"jsonrpc":"2.0","id":2,"method":"textDocument/rename",
        "params":{"textDocument":text_document,"position":position,"newName":"archive/Project Plan.dj"}}),
        json!({"jsonrpc":"2.0","id":99,"method":"shutdown","params":null}),
        json!({"jsonrpc":"2.0","method":"exit","params":null}),
    ];

    let responses = run_session(&msgs);
    let result = response_result(&responses, 2);
    let changes = result["documentChanges"]
        .as_array()
        .expect("documentChanges is not an array");
    assert_eq!(changes[0]["kind"], json!("rename"));
    assert_eq!(changes[0]["oldUri"], json!(project_uri));
    assert_eq!(changes[0]["newUri"], json!(renamed_uri));
    assert_eq!(
        sorted_document_change_edits(result),
        vec![
            (
                "index.dj".to_string(),
                2,
                path_col as u64,
                "archive/Project Plan.dj".to_string()
            ),
            (
                "notes.dj".to_string(),
                2,
                doc_nested
                    .lines()
                    .nth(2)
                    .unwrap()
                    .find("../Project Plan.dj")
                    .unwrap() as u64,
                "../archive/Project Plan.dj".to_string(),
            ),
        ]
    );
}

#[test]
fn rename_link_path_requires_document_changes_capability() {
    let responses = run_path_rename_with_workspace_edit_capabilities(json!({
        "resourceOperations": ["rename"]
    }));

    assert_eq!(
        response_error_message(&responses, 3),
        "Renaming link paths requires client support for workspace.workspaceEdit.documentChanges."
    );
}

#[test]
fn rename_link_path_requires_rename_resource_operation_capability() {
    let responses = run_path_rename_with_workspace_edit_capabilities(json!({
        "documentChanges": true,
        "resourceOperations": ["create", "delete"]
    }));

    assert_eq!(
        response_error_message(&responses, 3),
        "Renaming link paths requires client support for the workspace.workspaceEdit.resourceOperations rename operation."
    );
}

#[test]
fn rename_link_path_keeps_diagnostics_clean_after_client_applies_edit() {
    let dir = temp_dir("djot-ls-rename-link-path-diagnostics-test");
    let links = dir.join("links.dj");
    let outline = dir.join("outline.dj");
    let doc_before = "# Links\n\n[Appendix](outline.dj#appendix)\n";
    let doc_after = "# Links\n\n[Appendix](outlinx.dj#appendix)\n";
    std::fs::write(&links, doc_before).unwrap();
    std::fs::write(&outline, "{#appendix}\n# Appendix\n").unwrap();

    let root_uri = dir_uri(&dir);
    let links_uri = file_uri(&links);
    let path_col = doc_before
        .lines()
        .nth(2)
        .unwrap()
        .find("outline.dj")
        .unwrap() as i64;
    let msgs = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
            "capabilities":{"workspace":{"workspaceEdit":{"documentChanges":true,"resourceOperations":["rename"]}}},
            "processId":null,
            "rootUri":root_uri
        }}),
        json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":links_uri,"languageId":"djot","version":1,"text":doc_before}}}),
        json!({"jsonrpc":"2.0","id":2,"method":"textDocument/rename",
        "params":{"textDocument":{"uri":links_uri},"position":{"line":2,"character":path_col},"newName":"outlinx.dj"}}),
        json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":links_uri,"version":2},"contentChanges":[{"text":doc_after}]}}),
        json!({"jsonrpc":"2.0","id":99,"method":"shutdown","params":null}),
        json!({"jsonrpc":"2.0","method":"exit","params":null}),
    ];

    let responses = run_session(&msgs);
    let diagnostics = diagnostics_for(&responses, &links_uri);
    assert_eq!(diagnostics.last().unwrap().len(), 0);
}

#[test]
fn rename_link_path_file_watch_delete_clears_missing_optimistic_target() {
    let dir = temp_dir("djot-ls-rename-link-path-watch-delete-test");
    let links = dir.join("links.dj");
    let outline = dir.join("outline.dj");
    let renamed = dir.join("outlinx.dj");
    let doc_before = "# Links\n\n[Appendix](outline.dj#appendix)\n";
    let doc_after = "# Links\n\n[Appendix](outlinx.dj#appendix)\n";
    std::fs::write(&links, doc_before).unwrap();
    std::fs::write(&outline, "{#appendix}\n# Appendix\n").unwrap();

    let root_uri = dir_uri(&dir);
    let links_uri = file_uri(&links);
    let outline_uri = file_uri(&outline);
    let path_col = doc_before
        .lines()
        .nth(2)
        .unwrap()
        .find("outline.dj")
        .unwrap() as i64;
    let first_msgs = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
            "capabilities":{"workspace":{"workspaceEdit":{"documentChanges":true,"resourceOperations":["rename"]}}},
            "processId":null,
            "rootUri":root_uri
        }}),
        json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":links_uri,"languageId":"djot","version":1,"text":doc_before}}}),
        json!({"jsonrpc":"2.0","id":2,"method":"textDocument/rename",
        "params":{"textDocument":{"uri":links_uri},"position":{"line":2,"character":path_col},"newName":"outlinx.dj"}}),
    ];
    let second_msgs = [
        json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":links_uri,"version":2},"contentChanges":[{"text":doc_after}]}}),
        json!({"jsonrpc":"2.0","method":"workspace/didChangeWatchedFiles","params":{"changes":[{"uri":outline_uri,"type":3}]}}),
        json!({"jsonrpc":"2.0","id":3,"method":"textDocument/definition",
        "params":{"textDocument":{"uri":links_uri},"position":{"line":2,"character":path_col}}}),
        json!({"jsonrpc":"2.0","id":99,"method":"shutdown","params":null}),
        json!({"jsonrpc":"2.0","method":"exit","params":null}),
    ];

    let remove_outline = std::thread::spawn({
        let outline = outline.clone();
        move || {
            std::thread::sleep(std::time::Duration::from_millis(30));
            std::fs::remove_file(outline).unwrap();
        }
    });
    let responses = run_session_with_pause(
        &first_msgs,
        &second_msgs,
        std::time::Duration::from_millis(100),
    );
    remove_outline.join().unwrap();

    assert!(!renamed.exists());
    assert_eq!(response_result(&responses, 3), &Value::Null);
}

#[test]
fn rename_rejects_implicit_heading_anchor() {
    let dir = temp_dir("djot-ls-rename-implicit-heading-test");
    let a = dir.join("a.dj");
    let b = dir.join("b.dj");
    let doc_a = "# A\n\nsee [topic](b.dj#Topic)\n";
    let doc_b = "# Topic\n";
    std::fs::write(&a, doc_a).unwrap();
    std::fs::write(&b, doc_b).unwrap();

    let root_uri = dir_uri(&dir);
    let a_uri = file_uri(&a);
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
        "Renaming implicit heading anchors is not supported yet; add an explicit {#id} anchor or rename the heading text."
    );
    assert_eq!(
        response_error_message(&responses, 3),
        "Renaming implicit heading anchors is not supported yet; add an explicit {#id} anchor or rename the heading text."
    );
}

fn run_path_rename_with_workspace_edit_capabilities(workspace_edit: Value) -> Vec<Value> {
    let suffix = workspace_edit
        .as_object()
        .map(|object| {
            let mut keys = object.keys().cloned().collect::<Vec<_>>();
            keys.sort();
            keys.join("-")
        })
        .unwrap_or_else(|| "none".to_string());
    let dir = temp_dir(&format!(
        "djot-ls-rename-link-path-capability-test-{}-{}",
        std::process::id(),
        suffix
    ));
    let a = dir.join("a.dj");
    let b = dir.join("b.dj");
    let doc_a = "# A\n\nsee [topic](b.dj#topic)\n";
    std::fs::write(&a, doc_a).unwrap();
    std::fs::write(&b, "{#topic}\nTopic\n").unwrap();

    let root_uri = dir_uri(&dir);
    let a_uri = file_uri(&a);
    let path_col = doc_a.lines().nth(2).unwrap().find("b.dj").unwrap() as i64;
    let position = json!({"line":2,"character":path_col});
    let text_document = json!({"uri":a_uri});
    let msgs = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
            "capabilities":{"workspace":{"workspaceEdit":workspace_edit}},
            "processId":null,
            "rootUri":root_uri
        }}),
        json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
        json!({"jsonrpc":"2.0","id":3,"method":"textDocument/rename",
        "params":{"textDocument":text_document,"position":position,"newName":"renamed.dj"}}),
        json!({"jsonrpc":"2.0","id":99,"method":"shutdown","params":null}),
        json!({"jsonrpc":"2.0","method":"exit","params":null}),
    ];

    run_session(&msgs)
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

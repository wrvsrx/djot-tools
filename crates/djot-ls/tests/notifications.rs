//! End-to-end tests for LSP notifications.

mod support;

use serde_json::json;

use support::{dir_uri, file_uri, response_result, run_session, run_session_with_pause, temp_dir};

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

#[test]
fn initialized_reports_workspace_index_progress() {
    let dir = temp_dir("djot-ls-progress-test");
    std::fs::write(dir.join("a.dj"), "# A\n").unwrap();
    std::fs::write(dir.join("b.djot"), "# B\n").unwrap();
    let root_uri = dir_uri(&dir);

    let msgs = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{},"processId":null,"rootUri":root_uri}}),
        json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
        json!({"jsonrpc":"2.0","id":99,"method":"shutdown","params":null}),
        json!({"jsonrpc":"2.0","method":"exit","params":null}),
    ];

    let responses = run_session(&msgs);
    let progress_kinds = responses
        .iter()
        .filter(|m| m["method"] == json!("$/progress"))
        .map(|m| m["params"]["value"]["kind"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();

    assert_eq!(progress_kinds, ["begin", "report", "end"]);
}

#[test]
fn initialized_does_not_register_file_watchers_without_client_capability() {
    let dir = temp_dir("djot-ls-no-watch-registration-test");
    let root_uri = dir_uri(&dir);

    let msgs = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{},"processId":null,"rootUri":root_uri}}),
        json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
        json!({"jsonrpc":"2.0","id":99,"method":"shutdown","params":null}),
        json!({"jsonrpc":"2.0","method":"exit","params":null}),
    ];

    let responses = run_session(&msgs);
    assert!(!responses
        .iter()
        .any(|m| m["method"] == json!("client/registerCapability")));

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn initialized_registers_djot_file_watchers_with_client_capability() {
    let dir = temp_dir("djot-ls-watch-registration-test");
    let root_uri = dir_uri(&dir);

    let first_msgs = [
        json!({
            "jsonrpc":"2.0",
            "id":1,
            "method":"initialize",
            "params":{
                "capabilities":{
                    "workspace":{
                        "didChangeWatchedFiles":{
                            "dynamicRegistration":true,
                            "relativePatternSupport":true
                        }
                    }
                },
                "processId":null,
                "rootUri":root_uri
            }
        }),
        json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
    ];
    let second_msgs = [
        json!({"jsonrpc":"2.0","id":99,"method":"shutdown","params":null}),
        json!({"jsonrpc":"2.0","method":"exit","params":null}),
    ];

    let responses = run_session_with_pause(
        &first_msgs,
        &second_msgs,
        std::time::Duration::from_millis(100),
    );
    let registration = responses
        .iter()
        .find(|m| m["method"] == json!("client/registerCapability"))
        .expect("missing client/registerCapability request");
    let watchers = &registration["params"]["registrations"][0]["registerOptions"]["watchers"];

    assert_eq!(watchers[0]["globPattern"], json!("**/*.dj"));
    assert_eq!(watchers[0]["kind"], json!(7));
    assert_eq!(watchers[1]["globPattern"], json!("**/*.djot"));
    assert_eq!(watchers[1]["kind"], json!(7));

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn did_change_watched_files_indexes_created_djot_file() {
    let dir = temp_dir("djot-ls-watch-index-test");
    let topic = dir.join("topic.dj");
    std::fs::write(&topic, "# Topic\n\nbody\n").unwrap();
    let index = dir.join("index.dj");
    let index_uri = file_uri(&index);
    let topic_uri = file_uri(&topic);
    let doc = "# Index\n\n[topic](topic.dj#Topic)\n";

    let msgs = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{},"processId":null,"rootUri":null}}),
        json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
        json!({"jsonrpc":"2.0","method":"workspace/didChangeWatchedFiles","params":{"changes":[{"uri":topic_uri,"type":1}]}}),
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":index_uri,"languageId":"djot","version":1,"text":doc}}}),
        json!({"jsonrpc":"2.0","id":2,"method":"textDocument/definition","params":{"textDocument":{"uri":index_uri},"position":{"line":2,"character":2}}}),
        json!({"jsonrpc":"2.0","id":3,"method":"shutdown","params":null}),
        json!({"jsonrpc":"2.0","method":"exit","params":null}),
    ];

    let responses = run_session(&msgs);
    assert_eq!(response_result(&responses, 2)["uri"], json!(topic_uri));

    let _ = std::fs::remove_dir_all(dir);
}

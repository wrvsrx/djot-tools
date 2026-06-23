//! End-to-end tests for `textDocument/references`.

mod support;

use serde_json::{json, Value};

use support::{dir_uri, file_uri, response_result, run_session, temp_dir};

#[test]
fn references_finds_workspace_links_to_heading() {
    let dir = temp_dir("djot-ls-references-test");
    let a = dir.join("a.dj");
    let b = dir.join("b.dj");
    std::fs::write(&a, "# A\n\nsee [topic](b.dj#Topic)\n").unwrap();
    std::fs::write(&b, "# Intro\n\n[local](#Topic)\n\n## Topic\n\nbody\n").unwrap();

    let root_uri = dir_uri(&dir);
    let b_uri = file_uri(&b);

    let refs = |id: i64, include_declaration: bool| {
        json!({"jsonrpc":"2.0","id":id,"method":"textDocument/references",
        "params":{
                   "textDocument":{"uri":b_uri},
                   "position":{"line":4,"character":3},
                   "context":{"includeDeclaration":include_declaration}
               }})
    };
    let msgs = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{},"processId":null,"rootUri":root_uri}}),
        json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
        refs(2, true),
        refs(3, false),
        json!({"jsonrpc":"2.0","id":99,"method":"shutdown","params":null}),
        json!({"jsonrpc":"2.0","method":"exit","params":null}),
    ];

    let responses = run_session(&msgs);
    let with_decl = locations_for(&responses, 2);
    let without_decl = locations_for(&responses, 3);

    assert_eq!(
        sorted_lines(&with_decl),
        vec![("a.dj", 2), ("b.dj", 2), ("b.dj", 4)]
    );
    assert_eq!(sorted_lines(&without_decl), vec![("a.dj", 2), ("b.dj", 2)]);
}

#[test]
fn references_resolves_from_link_position() {
    let dir = temp_dir("djot-ls-references-link-test");
    let a = dir.join("a.dj");
    let b = dir.join("b.dj");
    let doc_a = "# A\n\nsee [topic](b.dj#Topic)\n";
    std::fs::write(&a, doc_a).unwrap();
    std::fs::write(&b, "# Intro\n\n[local](#Topic)\n\n## Topic\n\nbody\n").unwrap();

    let root_uri = dir_uri(&dir);
    let a_uri = file_uri(&a);
    let link_col = doc_a.lines().nth(2).unwrap().find("b.dj").unwrap() as i64;
    let msgs = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{},"processId":null,"rootUri":root_uri}}),
        json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
        json!({"jsonrpc":"2.0","id":2,"method":"textDocument/references",
        "params":{
            "textDocument":{"uri":a_uri},
            "position":{"line":2,"character":link_col},
            "context":{"includeDeclaration":true}
        }}),
        json!({"jsonrpc":"2.0","id":99,"method":"shutdown","params":null}),
        json!({"jsonrpc":"2.0","method":"exit","params":null}),
    ];

    let responses = run_session(&msgs);
    let locations = locations_for(&responses, 2);

    assert_eq!(
        sorted_lines(&locations),
        vec![("a.dj", 2), ("b.dj", 2), ("b.dj", 4)]
    );
}

fn locations_for(responses: &[Value], id: i64) -> Vec<Value> {
    response_result(responses, id)
        .as_array()
        .expect("references result is not an array")
        .clone()
}

fn sorted_lines(locations: &[Value]) -> Vec<(&str, u64)> {
    let mut out = locations
        .iter()
        .map(|location| {
            let uri = location["uri"].as_str().unwrap();
            let filename = uri.rsplit('/').next().unwrap();
            let line = location["range"]["start"]["line"].as_u64().unwrap();
            (filename, line)
        })
        .collect::<Vec<_>>();
    out.sort_unstable();
    out
}

//! End-to-end tests for `textDocument/definition`.

mod support;

use serde_json::json;

use support::{dir_uri, file_uri, response_result, run_session, temp_dir};

/// `textDocument/definition` resolves same-document links — both explicit
/// `#id` links and implicit heading references — to the heading, and returns
/// nothing when the target file does not exist.
#[test]
fn definition_resolves_same_document_links() {
    // line 0: heading, line 2: two links, line 7: Epilogue heading, line 9: cross-file
    let doc = "# My Heading\n\nSee [inline](#My-Heading) and [Epilogue][].\n\n{#anchor}\nA block.\n\n## Epilogue\n\n[ext](other.dj#sec)\n";
    let def = |id: i64, line: i64, ch: i64| {
        json!({"jsonrpc":"2.0","id":id,"method":"textDocument/definition",
               "params":{"textDocument":{"uri":"file:///t.dj"},"position":{"line":line,"character":ch}}})
    };
    let msgs = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{},"processId":null,"rootUri":null}}),
        json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///t.dj","languageId":"djot","version":1,"text":doc}}}),
        def(10, 2, 8),  // inside [inline](#My-Heading) -> line 0
        def(11, 2, 35), // inside [Epilogue][]          -> line 7
        def(12, 9, 3),  // inside [ext](other.dj#sec)   -> null (file absent)
        json!({"jsonrpc":"2.0","id":99,"method":"shutdown","params":null}),
        json!({"jsonrpc":"2.0","method":"exit","params":null}),
    ];

    let responses = run_session(&msgs);
    let result = |id: i64| response_result(&responses, id).clone();

    // Explicit #id link jumps to the heading on line 0.
    assert_eq!(result(10)["range"]["start"]["line"], json!(0));
    assert_eq!(result(10)["uri"], json!("file:///t.dj"));
    // Implicit heading reference [Epilogue][] jumps to the Epilogue heading.
    assert_eq!(result(11)["range"]["start"]["line"], json!(7));
    // Cross-file target whose file does not exist on disk yields nothing.
    assert_eq!(result(12), json!(null));
}

/// `textDocument/definition` follows a `path#id` link into another file,
/// loading that file from disk on demand.
#[test]
fn definition_jumps_across_files() {
    let dir = temp_dir("djot-ls-xfile-test");
    let a = dir.join("a.dj");
    let b = dir.join("b.dj");
    std::fs::write(&a, "# A\n\nsee [to B](b.dj#Topic)\n").unwrap();
    std::fs::write(&b, "# Intro\n\ntext\n\n## Topic\n\nbody\n").unwrap();

    let a_uri = file_uri(&a);
    let b_uri = file_uri(&b);
    let doc_a = std::fs::read_to_string(&a).unwrap();
    let link_col = doc_a.lines().nth(2).unwrap().find("b.dj").unwrap() as i64;

    let msgs = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{},"processId":null,"rootUri":null}}),
        json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":a_uri,"languageId":"djot","version":1,"text":doc_a}}}),
        // cursor on the b.dj#Topic link in a.dj (line 2)
        json!({"jsonrpc":"2.0","id":2,"method":"textDocument/definition","params":{"textDocument":{"uri":a_uri},"position":{"line":2,"character":link_col}}}),
        json!({"jsonrpc":"2.0","id":99,"method":"shutdown","params":null}),
        json!({"jsonrpc":"2.0","method":"exit","params":null}),
    ];

    let responses = run_session(&msgs);
    let result = response_result(&responses, 2).clone();

    // Jumps into b.dj, to the "## Topic" heading (line 4).
    assert_eq!(result["uri"], json!(b_uri));
    assert_eq!(result["range"]["start"]["line"], json!(4));
}

/// The server indexes `.dj` / `.djot` files under the client-provided root
/// during initialize, so definition works for workspace files before didOpen.
#[test]
fn definition_uses_client_workspace_root_index() {
    let dir = temp_dir("djot-ls-root-index-test");
    let a = dir.join("a.dj");
    let b = dir.join("nested").join("b.djot");
    std::fs::create_dir_all(b.parent().unwrap()).unwrap();
    std::fs::write(&a, "# A\n\nsee [to B](nested/b.djot#Topic)\n").unwrap();
    std::fs::write(&b, "# Intro\n\ntext\n\n## Topic\n\nbody\n").unwrap();

    let root_uri = dir_uri(&dir);
    let a_uri = file_uri(&a);
    let b_uri = file_uri(&b);
    let doc_a = std::fs::read_to_string(&a).unwrap();
    let link_col = doc_a.lines().nth(2).unwrap().find("nested/b.djot").unwrap() as i64;

    let msgs = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{},"processId":null,"rootUri":root_uri}}),
        json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
        // No didOpen: a.dj and nested/b.djot must both come from root indexing.
        json!({"jsonrpc":"2.0","id":2,"method":"textDocument/definition","params":{"textDocument":{"uri":a_uri},"position":{"line":2,"character":link_col}}}),
        json!({"jsonrpc":"2.0","id":99,"method":"shutdown","params":null}),
        json!({"jsonrpc":"2.0","method":"exit","params":null}),
    ];

    let responses = run_session(&msgs);
    let result = response_result(&responses, 2).clone();

    assert_eq!(result["uri"], json!(b_uri));
    assert_eq!(result["range"]["start"]["line"], json!(4));
}

/// Cross-file links may point to filenames containing spaces. jotdown gives
/// the destination to djot-core as `other file.jd#Topic`; resolving it should
/// read that exact filename from disk and return an encoded file URI.
#[test]
fn definition_jumps_to_file_with_space_in_name() {
    let dir = temp_dir("djot-ls-space-file-test");
    let a = dir.join("a.dj");
    let b = dir.join("other file.jd");
    std::fs::write(&a, "# A\n\nsee [to Topic](other file.jd#Topic)\n").unwrap();
    std::fs::write(&b, "# Intro\n\ntext\n\n## Topic\n\nbody\n").unwrap();

    let a_uri = file_uri(&a);
    let b_uri = file_uri(&b);
    let doc_a = std::fs::read_to_string(&a).unwrap();
    let link_col = doc_a.lines().nth(2).unwrap().find("other file.jd").unwrap() as i64;

    let msgs = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{},"processId":null,"rootUri":null}}),
        json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":a_uri,"languageId":"djot","version":1,"text":doc_a}}}),
        json!({"jsonrpc":"2.0","id":2,"method":"textDocument/definition","params":{"textDocument":{"uri":a_uri},"position":{"line":2,"character":link_col}}}),
        json!({"jsonrpc":"2.0","id":99,"method":"shutdown","params":null}),
        json!({"jsonrpc":"2.0","method":"exit","params":null}),
    ];

    let responses = run_session(&msgs);
    let result = response_result(&responses, 2).clone();

    assert_eq!(result["uri"], json!(b_uri));
    assert_eq!(result["range"]["start"]["line"], json!(4));
}

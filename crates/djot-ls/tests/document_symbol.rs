//! End-to-end test: spawn the built `djot-ls` binary and drive a real
//! JSON-RPC session over stdio, asserting on the `textDocument/documentSymbol`
//! response.

use std::io::{Read, Write};
use std::process::{Command, Stdio};

use serde_json::{json, Value};

/// Wrap a JSON value in an LSP `Content-Length` frame.
fn frame(v: &Value) -> Vec<u8> {
    let body = serde_json::to_vec(v).unwrap();
    let mut out = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    out.extend_from_slice(&body);
    out
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Split a stream of `Content-Length`-framed messages into JSON values.
fn parse_frames(mut data: &[u8]) -> Vec<Value> {
    let mut msgs = Vec::new();
    while let Some(pos) = find(data, b"\r\n\r\n") {
        let header = std::str::from_utf8(&data[..pos]).unwrap();
        let len: usize = header
            .lines()
            .find_map(|l| l.strip_prefix("Content-Length: "))
            .expect("missing Content-Length")
            .trim()
            .parse()
            .unwrap();
        let start = pos + 4;
        let body = &data[start..start + len];
        msgs.push(serde_json::from_slice(body).unwrap());
        data = &data[start + len..];
    }
    msgs
}

/// Spawn the built binary, feed it the given JSON-RPC messages over stdio, and
/// return the parsed responses it writes back.
fn run_session(msgs: &[Value]) -> Vec<Value> {
    let mut payload = Vec::new();
    for m in msgs {
        payload.extend_from_slice(&frame(m));
    }

    let mut child = Command::new(env!("CARGO_BIN_EXE_djot-ls"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    // The payload is tiny, so writing it all before reading cannot deadlock.
    child.stdin.take().unwrap().write_all(&payload).unwrap();
    let mut out = Vec::new();
    child.stdout.take().unwrap().read_to_end(&mut out).unwrap();
    child.wait().unwrap();

    parse_frames(&out)
}

#[test]
fn document_symbol_returns_headings() {
    // Title > Section A > Sub, plus a sibling top-level heading.
    let doc = "# Title\n\nsome text\n\n## Section A\n\nmore\n\n### Sub\n\n# Second\n";
    let msgs = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{},"processId":null,"rootUri":null}}),
        json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///t.dj","languageId":"djot","version":1,"text":doc}}}),
        json!({"jsonrpc":"2.0","id":2,"method":"textDocument/documentSymbol","params":{"textDocument":{"uri":"file:///t.dj"}}}),
        json!({"jsonrpc":"2.0","id":3,"method":"shutdown","params":null}),
        json!({"jsonrpc":"2.0","method":"exit","params":null}),
    ];

    let responses = run_session(&msgs);
    let roots = responses
        .iter()
        .find(|m| m["id"] == json!(2))
        .expect("no documentSymbol response")["result"]
        .as_array()
        .expect("result is not an array")
        .clone();

    let name = |s: &Value| s["name"].as_str().unwrap().to_string();
    let detail = |s: &Value| s["detail"].as_str().unwrap().to_string();
    let children = |s: &Value| s["children"].as_array().cloned().unwrap_or_default();

    // Two top-level headings.
    assert_eq!(
        roots.iter().map(name).collect::<Vec<_>>(),
        ["Title", "Second"]
    );

    // Title (H1) > Section A (H2) > Sub (H3).
    let title = &roots[0];
    assert_eq!(detail(title), "H1");
    let lvl2 = children(title);
    assert_eq!(lvl2.iter().map(name).collect::<Vec<_>>(), ["Section A"]);
    assert_eq!(detail(&lvl2[0]), "H2");
    let lvl3 = children(&lvl2[0]);
    assert_eq!(lvl3.iter().map(name).collect::<Vec<_>>(), ["Sub"]);
    assert_eq!(detail(&lvl3[0]), "H3");

    // A parent's range must enclose its child's range (line-wise here).
    let title_end = title["range"]["end"]["line"].as_u64().unwrap();
    let sub_end = lvl3[0]["range"]["end"]["line"].as_u64().unwrap();
    assert!(title_end >= sub_end, "parent range must contain child range");

    // The "Second" sibling is a leaf.
    assert!(children(&roots[1]).is_empty());
}

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
    let answered = responses.iter().any(|m| m["id"] == json!(2) && m.get("result").is_some());
    assert!(answered, "server did not answer documentSymbol after didSave");
}

/// `textDocument/definition` resolves same-document links — both explicit
/// `#id` links and implicit heading references — to the heading, and returns
/// nothing for cross-file targets (not implemented yet).
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
        def(12, 9, 3),  // inside [ext](other.dj#sec)   -> null (cross-file)
        json!({"jsonrpc":"2.0","id":99,"method":"shutdown","params":null}),
        json!({"jsonrpc":"2.0","method":"exit","params":null}),
    ];

    let responses = run_session(&msgs);
    let result = |id: i64| {
        responses
            .iter()
            .find(|m| m["id"] == json!(id))
            .unwrap_or_else(|| panic!("no response for id {id}"))["result"]
            .clone()
    };

    // Explicit #id link jumps to the heading on line 0.
    assert_eq!(result(10)["range"]["start"]["line"], json!(0));
    assert_eq!(result(10)["uri"], json!("file:///t.dj"));
    // Implicit heading reference [Epilogue][] jumps to the Epilogue heading.
    assert_eq!(result(11)["range"]["start"]["line"], json!(7));
    // Cross-file target is not resolved yet.
    assert_eq!(result(12), json!(null));
}

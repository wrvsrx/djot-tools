#![allow(dead_code)]

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{json, Value};

pub fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

pub fn file_uri(path: &Path) -> String {
    lsp_types::Url::from_file_path(path).unwrap().to_string()
}

pub fn dir_uri(path: &Path) -> String {
    lsp_types::Url::from_directory_path(path)
        .unwrap()
        .to_string()
}

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
pub fn run_session(msgs: &[Value]) -> Vec<Value> {
    run_session_with_pause(msgs, &[], std::time::Duration::ZERO)
}

pub fn run_session_with_pause(
    first_msgs: &[Value],
    second_msgs: &[Value],
    pause: std::time::Duration,
) -> Vec<Value> {
    let mut payload = Vec::new();
    for m in first_msgs {
        payload.extend_from_slice(&frame(m));
    }
    let mut second_payload = Vec::new();
    for m in second_msgs {
        second_payload.extend_from_slice(&frame(m));
    }

    let mut child = Command::new(env!("CARGO_BIN_EXE_djot-ls"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    // The payload is tiny, so writing it all before reading cannot deadlock.
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(&payload).unwrap();
    if !pause.is_zero() {
        std::thread::sleep(pause);
    }
    stdin.write_all(&second_payload).unwrap();
    drop(stdin);
    let mut out = Vec::new();
    child.stdout.take().unwrap().read_to_end(&mut out).unwrap();
    child.wait().unwrap();

    parse_frames(&out)
}

pub fn response_result(responses: &[Value], id: i64) -> &Value {
    &response_for_id(responses, id)["result"]
}

pub fn response_error_message(responses: &[Value], id: i64) -> &str {
    response_for_id(responses, id)["error"]["message"]
        .as_str()
        .expect("error message is not a string")
}

pub fn diagnostics_for(responses: &[Value], uri: &str) -> Vec<Vec<Value>> {
    responses
        .iter()
        .filter(|message| {
            message["method"] == json!("textDocument/publishDiagnostics")
                && message["params"]["uri"] == json!(uri)
        })
        .map(|message| {
            message["params"]["diagnostics"]
                .as_array()
                .expect("diagnostics is not an array")
                .clone()
        })
        .collect()
}

pub fn last_diagnostics(responses: &[Value]) -> Vec<Value> {
    responses
        .iter()
        .rev()
        .find(|message| message["method"] == json!("textDocument/publishDiagnostics"))
        .expect("no publishDiagnostics notification")["params"]["diagnostics"]
        .as_array()
        .expect("diagnostics is not an array")
        .clone()
}

fn response_for_id(responses: &[Value], id: i64) -> &Value {
    responses
        .iter()
        .find(|message| message["id"] == json!(id))
        .unwrap_or_else(|| panic!("no response for id {id}"))
}

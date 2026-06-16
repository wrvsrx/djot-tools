use std::io::{Read, Write};
use std::process::{Command, Stdio};

use serde_json::Value;

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

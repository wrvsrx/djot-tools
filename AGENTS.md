# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

(Canonical file is `AGENTS.md`; `CLAUDE.md` is a symlink to it.)

## What this is

A Language Server (LSP) for [Djot](https://djot.net), written in Rust. It parses documents with [`jotdown`](https://docs.rs/jotdown) and serves them over LSP using [`async-lsp`](https://docs.rs/async-lsp). The roadmap lives in `docs/plan.dj` (documentSymbol → definition → diagnostics → completion → semantic tokens). Currently `textDocument/documentSymbol` (headings) is implemented.

## Commands

- Build: `cargo build` (binary is `target/debug/djot-ls`)
- Test: `cargo test`
- Run one test: `cargo test --test document_symbol did_save_does_not_crash_the_server`
- The dev environment is a Nix flake (`use_flake .` via direnv); `dev/envrc` is symlinked to the repo-root `.envrc`.

## Build gotcha: do not bump tokio

The crates index mirror in this environment lags and lacks `tokio-macros 2.7.0`, so resolving tokio ≥ 1.52 fails. `Cargo.toml` allows `tokio = "1.51.0"` (caret) but `Cargo.lock` holds it at exactly 1.51.0. **Do not run `cargo update` / `cargo update -p tokio` expecting a newer tokio** — it will try 1.52.x and fail. Keep the locked 1.51.0 until the mirror catches up.

## Runtime gotcha: every notification must be handled or the server crashes

This is the most important architectural constraint. The server uses async-lsp's **omni-trait** style (`Router::from_language_server` + `impl LanguageServer for ServerState`). The omni-trait pre-registers a handler for *every* standard LSP notification whose default returns `ControlFlow::Break(Err(Routing(...)))` — which breaks the main loop and makes `run_buffered(...).await.unwrap()` in `main` panic, killing the process.

Implication: **whenever you advertise a capability that causes editors to send a new notification** (`didSave`, `willSave`, `didChangeWatchedFiles`, `didChangeWorkspaceFolders`, etc.), you MUST add that method to `impl LanguageServer` — even as a no-op `ControlFlow::Continue(())` — or the server will crash in real editors. (`$/`-prefixed notifications, `exit`, and `initialized` are exempt.) A `Router::unhandled_notification` catch-all does *not* cover these, because the omni-trait already registered a breaking handler for each. The currently-handled set is `did_open`/`did_change`/`did_close`/`did_save`/`did_change_configuration` in `src/main.rs`.

## Architecture

Everything is in `src/main.rs` (single binary crate):

- `ServerState` holds `client: ClientSocket` and `documents: HashMap<Url, String>`. Because the omni-trait gives handlers `&mut self`, document state needs **no locking** — this is the main reason async-lsp was chosen over tower-lsp.
- Text sync is **FULL** (`TextDocumentSyncKind::FULL`): `did_change` replaces the whole stored string from the last content change. No incremental/rope handling yet.
- `document_symbol` parses on demand via `jotdown::Parser::new(text).into_offset_iter()`, which yields `(Event, Range<usize>)`. `heading_symbols()` walks the events, accumulating `Event::Str` between a heading's `Start`/`End` into the symbol name and using the byte spans for the range.
- `offset_to_position()` converts a byte offset to an LSP `Position` by scanning the text and counting `len_utf16()` per char (LSP columns are UTF-16). It is O(n) per call — fine for now, worth precomputing line starts if it shows up in profiles.
- `main()` wires the tower middleware stack (`Tracing`/`Lifecycle`/`CatchUnwind`/`Concurrency`/`ClientProcessMonitor`) around the router and runs `run_buffered` over real async stdio (`PipeStdin/PipeStdout::lock_tokio`). Tracing goes to **stderr** (stdout is the LSP transport).

## Testing approach

`tests/document_symbol.rs` is **black-box**: it spawns the built binary (`env!("CARGO_BIN_EXE_djot-ls")`) and drives a full `initialize → didOpen → … → shutdown → exit` JSON-RPC session over stdio, parsing `Content-Length`-framed responses with `serde_json`. This is why behavior like the didSave-crash regression can be tested without exposing internals. For testing private helpers (`heading_symbols`, `offset_to_position`) directly, add a `#[cfg(test)] mod tests` inside `src/main.rs` instead — faster and more precise.

## Editor testing

`dev/nvim.lua` registers the server for the `djot` filetype via Neovim's `vim.lsp.config`/`vim.lsp.enable` (cmd `./target/debug/djot-ls`). Build first, then open a `.dj` file. Server-side panics surface in Neovim's `:LspLog` (the binary's stderr is captured there).

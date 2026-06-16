# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

(Canonical file is `AGENTS.md`; `CLAUDE.md` is a symlink to it.)

## What this is

A Language Server (LSP) for [Djot](https://djot.net), written in Rust. It parses documents with [`jotdown`](https://docs.rs/jotdown) and serves them over LSP using [`async-lsp`](https://docs.rs/async-lsp). The roadmap lives in `docs/plan.dj` (documentSymbol → definition → diagnostics → completion → semantic tokens). `textDocument/documentSymbol` (nested headings) and same-file `textDocument/definition` are implemented.

This is a **Cargo workspace** (`crates/*`) so the djot semantics can be shared by more than one tool. Alongside the language server there is `djot-export`, a CLI that converts djot to a pandoc JSON AST (`djot-export doc.dj | pandoc -f json -o doc.pdf`).

## Commands

- Build: `cargo build` (binary is `target/debug/djot-ls`)
- Test: `cargo test` (whole workspace)
- Run one test: `cargo test -p djot-ls --test document_symbol did_save_does_not_crash_the_server`
- Test the core lib only: `cargo test -p djot-core`
- The dev environment is a Nix flake (`use_flake .` via direnv); `dev/envrc` is symlinked to the repo-root `.envrc`.

## Build gotcha: do not bump tokio

The crates index mirror in this environment lags and lacks `tokio-macros 2.7.0`, so resolving tokio ≥ 1.52 fails. `Cargo.toml` allows `tokio = "1.51.0"` (caret) but `Cargo.lock` holds it at exactly 1.51.0. **Do not run `cargo update` / `cargo update -p tokio` expecting a newer tokio** — it will try 1.52.x and fail. Keep the locked 1.51.0 until the mirror catches up.

## Runtime gotcha: every notification must be handled or the server crashes

This is the most important architectural constraint. The server uses async-lsp's **omni-trait** style (`Router::from_language_server` + `impl LanguageServer for ServerState`). The omni-trait pre-registers a handler for *every* standard LSP notification whose default returns `ControlFlow::Break(Err(Routing(...)))` — which breaks the main loop and makes `run_buffered(...).await.unwrap()` in `main` panic, killing the process.

Implication: **whenever you advertise a capability that causes editors to send a new notification** (`didSave`, `willSave`, `didChangeWatchedFiles`, `didChangeWorkspaceFolders`, etc.), you MUST add that method to `impl LanguageServer` — even as a no-op `ControlFlow::Continue(())` — or the server will crash in real editors. (`$/`-prefixed notifications, `exit`, and `initialized` are exempt.) A `Router::unhandled_notification` catch-all does *not* cover these, because the omni-trait already registered a breaking handler for each. The currently-handled set is `did_open`/`did_change`/`did_close`/`did_save`/`did_change_configuration` in `crates/djot-ls/src/main.rs`.

## Architecture

Two crates in a workspace, split along a deliberate boundary: **`djot-core` is protocol-agnostic and works in byte offsets only**; **`djot-ls` owns everything LSP** (lsp_types, async-lsp, UTF-16 positions). The planned exporter will reuse `djot-core` without pulling in any LSP types.

`crates/djot-core/src/lib.rs` (lib, depends only on `jotdown`):

- `heading_outline(text) -> Vec<Heading>` builds a **nested** outline. jotdown wraps each heading in a `Section` container that nests by level, so it walks the section `Start`/`End` events with a stack — the section span is `Heading::range`, the heading line is `selection_range`, nested sections become `children`. A `captured` flag stops headings inside non-section blocks (e.g. a blockquote) from overwriting a section's title.
- `build_index(text) -> DocIndex` collects `anchors` (heading/section ids plus any `{#id}` attribute → byte range) and `references` (every link → byte span + a `RefTarget` classified by `parse_dst`: `Internal #id` / `External path#id` / `Url`). jotdown resolves inline/reference/implicit links all to one destination string, so references are uniform.
- `metadata_block(text) -> Option<String>` returns the raw toml of a leading `{.metadata}` code block; `has_class` / `METADATA_CLASS` are the shared primitives for that convention (used by both the planned hover and `djot-export`).
- All ranges are `std::ops::Range<usize>` byte offsets. No lsp_types here.

`crates/djot-export/src/main.rs` (bin `djot-export`, depends on `djot-core` + `jotdown` + `serde_json`):

- Reads djot (file arg or stdin) and prints a pandoc JSON AST (`pandoc-api-version` `[1,23,1,1]`). Walks jotdown events with a `Frame` stack, mapping containers to pandoc nodes (sections → `Div`, headings → `Header`, lists, emphasis/strong, links, inline/fenced code, …); unhandled containers are spliced through so output stays valid. Covers a common subset only.
- The conversion is **where conventions become export semantics**: a `{.metadata}` code block (detected via `djot_core::has_class`/`METADATA_CLASS`) is dropped from the body instead of rendered — the TODO is to fold `djot_core::metadata_block` into pandoc `Meta`.
- Verify with a round-trip: `printf '# H\n' | ./target/debug/djot-export | pandoc -f json -t markdown`.

`crates/djot-ls/src/main.rs` (bin `djot-ls`, depends on `djot-core`):

- `ServerState` holds `client: ClientSocket` and `documents: HashMap<Url, String>`. Because the omni-trait gives handlers `&mut self`, document state needs **no locking** — this is the main reason async-lsp was chosen over tower-lsp.
- Text sync is **FULL** (`TextDocumentSyncKind::FULL`): `did_change` replaces the whole stored string from the last content change. No incremental/rope handling yet.
- `document_symbol` calls `heading_outline` then maps each `Heading` to `DocumentSymbol`; `definition` calls `build_index`, hit-tests the cursor byte offset against link spans, and resolves `Internal` targets to the anchor. Cross-file/`Url` targets return nothing yet.
- `offset_to_position`/`position_to_offset` convert between byte offsets and LSP `Position` (UTF-16 columns) — O(n) per call, fine for now, worth precomputing line starts if it shows up in profiles.
- `main()` wires the tower middleware stack (`Tracing`/`Lifecycle`/`CatchUnwind`/`Concurrency`/`ClientProcessMonitor`) around the router and runs `run_buffered` over real async stdio (`PipeStdin/PipeStdout::lock_tokio`). Tracing goes to **stderr** (stdout is the LSP transport).

## Testing approach

`crates/djot-ls/tests/document_symbol.rs` is **black-box**: it spawns the built binary (`env!("CARGO_BIN_EXE_djot-ls")`) and drives a full `initialize → didOpen → … → shutdown → exit` JSON-RPC session over stdio, parsing `Content-Length`-framed responses with `serde_json`. This is how the didSave-crash regression and the symbol/definition behavior are tested end to end. Pure semantics (`heading_outline`, `build_index`, `parse_dst`) are unit-tested directly in `djot-core` (`#[cfg(test)] mod tests` in its `lib.rs`) — faster and more precise.

## Editor testing

`dev/nvim.lua` registers the server for the `djot` filetype via Neovim's `vim.lsp.config`/`vim.lsp.enable` (cmd `./target/debug/djot-ls`). Build first, then open a `.dj` file. Server-side panics surface in Neovim's `:LspLog` (the binary's stderr is captured there).

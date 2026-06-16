# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

(Canonical file is `AGENTS.md`; `CLAUDE.md` is a symlink to it.)

## What this is

A Language Server (LSP) for [Djot](https://djot.net), written in Rust. It parses documents with [`jotdown`](https://docs.rs/jotdown) and serves them over LSP using [`async-lsp`](https://docs.rs/async-lsp). The roadmap lives in `docs/plan.dj` (documentSymbol → definition → diagnostics → completion → semantic tokens). `textDocument/documentSymbol` (nested headings) and `textDocument/definition` (same-file and cross-file links) are implemented.

This is a **Cargo workspace** (`crates/*`) so the djot semantics can be shared by more than one tool. Alongside the language server there is `djot-export`, a CLI that converts djot to a pandoc JSON AST (`djot-export doc.dj | pandoc -f json -o doc.pdf`).

## Commands

- Build: `cargo build` (binary is `target/debug/djot-ls`)
- Test: `cargo test` (whole workspace)
- Run one test: `cargo test -p djot-ls --test document_symbol did_save_does_not_crash_the_server`
- Test the core lib only: `cargo test -p djot-core`
- The dev environment is a Nix flake (`use_flake .` via direnv); `dev/envrc` is symlinked to the repo-root `.envrc`.
- Git hooks live in `dev/hooks/`; enable them once per clone with `git config core.hooksPath dev/hooks`. The `pre-commit` hook checks that `README.md` is still in sync with `README.dj` whenever either is committed.

## Generated README

`README.md` is generated from `README.dj` by this project's own exporter; do not edit it by hand. Regenerate after editing `README.dj`:

```
djot-export README.dj | pandoc -f json -t gfm --lua-filter=dev/title-heading.lua --lua-filter=dev/strip-sections.lua > README.md
```

`dev/title-heading.lua` turns the metadata `title` into the document's single H1 and demotes the other headings; `dev/strip-sections.lua` unwraps djot's implicit `<section>` divs.

## Build gotcha: do not bump tokio

The crates index mirror in this environment lags and lacks `tokio-macros 2.7.0`, so resolving tokio ≥ 1.52 fails. `Cargo.toml` allows `tokio = "1.51.0"` (caret) but `Cargo.lock` holds it at exactly 1.51.0. **Do not run `cargo update` / `cargo update -p tokio` expecting a newer tokio** — it will try 1.52.x and fail. Keep the locked 1.51.0 until the mirror catches up.

## Runtime gotcha: every notification must be handled or the server crashes

This is the most important architectural constraint. The server uses async-lsp's **omni-trait** style (`Router::from_language_server` + `impl LanguageServer for ServerState`). The omni-trait pre-registers a handler for *every* standard LSP notification whose default returns `ControlFlow::Break(Err(Routing(...)))` — which breaks the main loop and makes `run_buffered(...).await.unwrap()` in `main` panic, killing the process.

Implication: **whenever you advertise a capability that causes editors to send a new notification** (`didSave`, `willSave`, `didChangeWatchedFiles`, `didChangeWorkspaceFolders`, etc.), you MUST add that method to `impl LanguageServer` — even as a no-op `ControlFlow::Continue(())` — or the server will crash in real editors. (`$/`-prefixed notifications, `exit`, and `initialized` are exempt.) A `Router::unhandled_notification` catch-all does *not* cover these, because the omni-trait already registered a breaking handler for each. The currently-handled set is `did_open`/`did_change`/`did_close`/`did_save`/`did_change_configuration` in `crates/djot-ls/src/main.rs`.

## Architecture

Three crates in a workspace, split along a deliberate boundary: **`djot-core` is protocol-agnostic and works in byte offsets only**; **`djot-ls` owns everything LSP** (lsp_types, async-lsp, UTF-16 positions); **`djot-export` owns the pandoc AST**. Both binaries reuse `djot-core` without pulling in each other's types.

`crates/djot-core/src/lib.rs` (lib, depends only on `jotdown`):

- `heading_outline(text) -> Vec<Heading>` builds a **nested** outline. jotdown wraps each heading in a `Section` container that nests by level, so it walks the section `Start`/`End` events with a stack — the section span is `Heading::range`, the heading line is `selection_range`, nested sections become `children`. A `captured` flag stops headings inside non-section blocks (e.g. a blockquote) from overwriting a section's title.
- `build_index(text) -> DocIndex` collects `anchors` (heading/section ids plus any `{#id}` attribute → byte range) and `references` (every link → byte span + a `RefTarget` classified by `parse_dst`: `Internal #id` / `External path#id` / `Url`). jotdown resolves inline/reference/implicit links all to one destination string, so references are uniform.
- `metadata_block(text) -> Option<String>` returns the raw toml of a leading `{.metadata}` code block; `has_class` / `METADATA_CLASS` are the shared primitives for that convention (used by both the planned hover and `djot-export`).
- All ranges are `std::ops::Range<usize>` byte offsets. No lsp_types here.

`crates/djot-export/src/main.rs` (bin `djot-export`, depends on `djot-core` + `jotdown` + `serde_json`):

- Reads djot (file arg or stdin) and prints a pandoc JSON AST (`pandoc-api-version` `[1,23,1,1]`). Walks jotdown events with a `Frame` stack, mapping containers to pandoc nodes (sections → `Div`, headings → `Header`, lists, emphasis/strong, links, inline/fenced code, …); unhandled containers are spliced through so output stays valid. Covers a common subset only.
- The conversion is **where conventions become export semantics**: a `{.metadata}` code block (via `djot_core::metadata_block`) is parsed as toml and folded into pandoc `Meta` (`build_meta`/`toml_to_meta`) instead of rendered in the body, so its information is preserved rather than dropped.
- Verify with a round-trip: `printf '# H\n' | ./target/debug/djot-export | pandoc -f json -t markdown`.

`crates/djot-ls/src/main.rs` (bin `djot-ls`, depends on `djot-core`):

- `ServerState` holds `client: ClientSocket` and a `djot_core::Workspace` (path-keyed parsed documents). Because the omni-trait gives handlers `&mut self`, the index needs **no locking** — this is the main reason async-lsp was chosen over tower-lsp. URIs are mapped to/from paths with `Url::to_file_path`/`from_file_path` (file URIs only for now).
- Text sync is **FULL** (`TextDocumentSyncKind::FULL`): `did_change` reparses the whole document into the workspace; `did_close` drops the buffer (a later lookup re-reads from disk).
- `document_symbol` calls `heading_outline` on the stored text then maps each `Heading` to `DocumentSymbol`; `definition` (`resolve_definition`) hit-tests the cursor against link spans via the workspace, `resolve_target`s the link, lazily loads a cross-file target from disk if unseen, and returns the anchor's `Location`. Same-file and cross-file links go through the same path; external URLs return nothing.
- `offset_to_position`/`position_to_offset` convert between byte offsets and LSP `Position` (UTF-16 columns) — O(n) per call, fine for now, worth precomputing line starts if it shows up in profiles.
- `main()` wires the tower middleware stack (`Tracing`/`Lifecycle`/`CatchUnwind`/`Concurrency`/`ClientProcessMonitor`) around the router and runs `run_buffered` over real async stdio (`PipeStdin/PipeStdout::lock_tokio`). Tracing goes to **stderr** (stdout is the LSP transport).

## Testing approach

`crates/djot-ls/tests/document_symbol.rs` is **black-box**: it spawns the built binary (`env!("CARGO_BIN_EXE_djot-ls")`) and drives a full `initialize → didOpen → … → shutdown → exit` JSON-RPC session over stdio, parsing `Content-Length`-framed responses with `serde_json`. This is how the didSave-crash regression and the symbol/definition behavior are tested end to end. Pure semantics (`heading_outline`, `build_index`, `parse_dst`) are unit-tested directly in `djot-core` (`#[cfg(test)] mod tests` in its `lib.rs`) — faster and more precise.

## Editor testing

`dev/nvim.lua` registers the server for the `djot` filetype via Neovim's `vim.lsp.config`/`vim.lsp.enable` (cmd `./target/debug/djot-ls`). Build first, then open a `.dj` file. Server-side panics surface in Neovim's `:LspLog` (the binary's stderr is captured there).

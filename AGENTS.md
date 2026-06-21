# AGENTS.md

This file provides guidance to AI coding agents when working with code in this repository.

`AGENTS.md` is the canonical instruction file. Tool-specific files, such as
`CLAUDE.md`, should point agents here rather than duplicating these
instructions.

## What this is

A Language Server (LSP) for [Djot](https://djot.net), written in Rust. It parses documents with [`jotdown`](https://docs.rs/jotdown) and serves them over LSP using [`async-lsp`](https://docs.rs/async-lsp). The roadmap lives in `docs/plan.dj` (documentSymbol → definition → references → hover → diagnostics → completion → semantic tokens). `textDocument/documentSymbol` (nested headings), `textDocument/definition` (same-file and cross-file links), `textDocument/references` (backlinks), and `textDocument/hover` (target information) are implemented.

This is a **Cargo workspace** (`crates/*`) so the djot semantics can be shared by more than one tool. Alongside the language server there is `djot-export`, a CLI that uses Pandoc's native Djot reader and applies project-specific export semantics to produce a pandoc JSON AST (`djot-export doc.dj | pandoc -f json -o doc.pdf`), and `djot-filter`, a CLI for filtering directories of djot documents with CEL predicates over path, title, and reverse references.

## Project layout

- `Cargo.toml` is the workspace root. Members are `crates/djot-core`,
  `crates/djot-ls`, `crates/djot-export`, and `crates/djot-filter`.
- `crates/djot-core/` is the protocol-agnostic djot analysis library.
- `crates/djot-ls/` is the `djot-ls` LSP binary and its black-box integration
  tests.
- `crates/djot-export/` is the `djot-export` CLI.
- `crates/djot-filter/` is the `djot-filter` CLI.
- `docs/plan.dj` is the feature roadmap.
- `docs/semantics.dj` describes the current project semantics layered on top of
  Djot syntax.
- `flake.nix` packages the workspace binaries as the `djot-tools` Nix package
  with `buildRustPackage`. The version is read from `Cargo.toml` with
  `builtins.fromTOML`.
- `examples/*.dj` are small manual test fixtures for outlines, links, and
  article-style documents.
- `dev/` contains editor/dev helpers: Neovim LSP config, README export Lua
  filters, and git hooks.
- `README.dj` is the source for `README.md`.
- `AGENTS.md` is canonical agent guidance. `CLAUDE.md` is only a compatibility
  pointer to this file.

## Commands

- Build: `cargo build` (binary is `target/debug/djot-ls`)
- Test: `cargo test` (whole workspace, including `djot-core`, `djot-export`,
  and the `djot-ls` integration tests)
- Run one test: `cargo test -p djot-ls --test document_symbol did_save_does_not_crash_the_server`
- Test the core lib only: `cargo test -p djot-core`
- Test the exporter only: `cargo test -p djot-export`
- Test the filter only: `cargo test -p djot-filter`
- Run exporter manually: `printf '# H\n' | cargo run -p djot-export -- | pandoc -f json -t markdown` (requires `pandoc`)
- Run filter manually: `cargo run -p djot-filter -- --root docs --query 'title.matches("semantics")'`
- Filter referenced docs: `cargo run -p djot-filter -- --root notes --query '"index.dj" in directly_referenced_by'`
- Build the Nix package: `nix build .`; the package name is `djot-tools` and
  installs `djot-ls`, `djot-export`, and `djot-filter`.
- The dev environment is a Nix flake (`use_flake .` via direnv); `dev/envrc` is symlinked to the repo-root `.envrc`.
- Git hooks live in `dev/hooks/`; enable them once per clone with `git config core.hooksPath dev/hooks`. The `pre-commit` hook checks that `README.md` is still in sync with `README.dj` whenever either is committed.

## Commit workflow

When implementing a feature or a fix, start from the current main branch state
and create a short-lived topic branch before changing code. Use a descriptive
branch name such as `feat/task-wait` or `fix/diagnostics-refresh`. Do not work
directly on `main` for feature or fix implementation unless the user explicitly
asks for that.

Commit each coherent piece of code as it is completed instead of waiting until
the end of the whole task. For a non-trivial change, split the work into small,
logical commits instead of one broad commit. Prefer this order when it applies:

- protocol-agnostic core data/model changes first;
- core behavior/API changes with focused unit tests next;
- LSP/CLI integration and black-box tests after the shared behavior exists;
- docs or roadmap status updates last, in their own commit when they are just
  reflecting completed work.

For longer tasks, commit after each coherent group of completed steps so the
history stays reviewable and later work can build on stable checkpoints. When
the implementation is complete and tests pass, summarize the branch and ask the
user to confirm before merging it back into `main`.

Before each commit, check `git status --short` and `git diff` so unrelated user
changes are not included. Run the narrowest relevant tests before intermediate
commits, and run the full relevant suite before the final implementation
commit. Use concise conventional-style messages in the form
`type(scope): subject` for code changes, such as `feat(core): ...`,
`fix(ls): ...`, `test(filter): ...`, or `chore(dev): ...`. Use `docs: ...`
for documentation-only changes unless the surrounding history clearly uses a
more specific docs scope.

After the user confirms the completed branch, merge it into `main` using a
normal non-destructive merge workflow. Check `git status --short` before and
after the merge, and do not include unrelated user changes in the merge commit.

## Release workflow

When asked to release a version, use this sequence:

- bump `[workspace.package].version` in `Cargo.toml` from the current `*-dev`
  version to the release version, such as `0.2.0-dev` to `0.2.0`;
- do not edit `Cargo.lock` by hand; run a Cargo command such as
  `cargo check --workspace` so Cargo updates the workspace package versions in
  the lockfile;
- commit the release version with a message like `chore: release 0.2.0`;
- create a git tag with the exact release version, such as `git tag 0.2.0`,
  pointing at the release commit;
- bump `[workspace.package].version` in `Cargo.toml` to the next development
  version, such as `0.3.0-dev`;
- run `cargo check --workspace` again so Cargo updates `Cargo.lock`;
- commit the development-version bump with a message like
  `chore: bump version to 0.3.0-dev`.

## Generated README

`README.md` is generated from `README.dj` by this project's own exporter; do not edit it by hand. Regenerate after editing `README.dj`:

```
djot-export README.dj | pandoc -f json -t gfm --lua-filter=dev/title-heading.lua --lua-filter=dev/strip-sections.lua > README.md
```

`dev/title-heading.lua` turns the metadata `title` into the document's single H1 and demotes the other headings; `dev/strip-sections.lua` unwraps djot's implicit `<section>` divs.

## Semantics documentation

`docs/semantics.dj` describes only the semantics that are currently implemented
and shared by the tools: document/workspace identity, metadata, anchors,
references, target resolution, and current semantic diagnostics. Keep LSP
operations such as hover, completion, definition, references, and rename out of
that document unless their behavior changes the underlying semantics; those
operations should follow naturally from the semantic model. Future plans and
unimplemented semantics, including task/note semantics, belong in
`docs/plan.dj`.

## Roadmap documentation

`docs/plan.dj` is a roadmap, not user documentation. Keep it DRY:

- current command usage and examples belong in `README.dj` (then regenerate
  `README.md`);
- currently implemented Djot conventions and shared semantics belong in
  `docs/semantics.dj`;
- `docs/plan.dj` should link to those documents for current behavior and keep
  only future work, open design questions, or very brief completed-status
  markers when useful for roadmap context.

## Feature update checklist

When implementing a new feature, update the adjacent project materials in the
same change set when they apply:

- add or adjust the narrowest relevant tests, using `djot-core` unit tests for
  protocol-agnostic semantics and black-box `djot-ls` tests for LSP behavior;
- update `docs/semantics.dj` only when the feature changes shared, implemented
  Djot semantics, not merely an LSP/CLI presentation of existing semantics;
- update `README.dj` for current user-facing commands, behavior, or examples,
  then regenerate `README.md` instead of editing it by hand;
- update `docs/plan.dj` for roadmap status, remaining work, or open design
  questions;
- update `examples/*.dj` when the feature benefits from a manual playground,
  demo fixture, completion target, or cross-reference target.

## Runtime gotcha: every notification must be handled or the server crashes

This is the most important architectural constraint. The server uses async-lsp's **omni-trait** style (`Router::from_language_server` + `impl LanguageServer for ServerState`). The omni-trait pre-registers a handler for *every* standard LSP notification whose default returns `ControlFlow::Break(Err(Routing(...)))` — which breaks the main loop and makes `run_buffered(...).await.unwrap()` in `main` panic, killing the process.

Implication: **whenever you advertise a capability that causes editors to send a new notification** (`didSave`, `willSave`, `didChangeWatchedFiles`, `didChangeWorkspaceFolders`, etc.), you MUST add that method to `impl LanguageServer` — even as a no-op `ControlFlow::Continue(())` — or the server will crash in real editors. (`$/`-prefixed notifications, `exit`, and `initialized` are exempt.) A `Router::unhandled_notification` catch-all does *not* cover these, because the omni-trait already registered a breaking handler for each. The currently-handled set is `did_open`/`did_change`/`did_close`/`did_save`/`did_change_configuration` in `crates/djot-ls/src/main.rs`.

## Architecture

Four crates in a workspace, split along a deliberate boundary:

- **`djot-core` is protocol-agnostic and works in byte offsets only**.
- **`djot-ls` owns everything LSP** (`lsp_types`, `async-lsp`, UTF-16
  positions).
- **`djot-export` owns the pandoc JSON AST**.
- **`djot-filter` owns directory filtering CLI behavior**.

All binaries reuse `djot-core` without pulling in each other's types.

`crates/djot-core/src/lib.rs` (lib, depends on `jotdown` and `serde`):

- `heading_outline(text) -> Vec<Heading>` builds a **nested** outline. jotdown wraps each heading in a `Section` container that nests by level, so it walks the section `Start`/`End` events with a stack — the section span is `Heading::range`, the heading line is `selection_range`, nested sections become `children`. A `captured` flag stops headings inside non-section blocks (e.g. a blockquote) from overwriting a section's title.
- `build_index(text) -> DocIndex` collects `anchors` (heading/section ids plus any `{#id}` attribute → byte range) and `references` (every link → byte span + a `RefTarget` classified by `parse_dst`: `Internal #id` / `External path#id` / `Url`). jotdown resolves inline/reference/implicit links all to one destination string, so references are uniform.
- `metadata_block(text) -> Option<String>` returns the raw toml of a leading `{.metadata}` code block; `has_class` / `METADATA_CLASS` are the shared primitives for that convention (used by the planned metadata hover and `djot-export`).
- `resolve_target(from, target)` normalizes internal and relative cross-file
  targets; URLs deliberately return `None`.
- `Workspace` stores parsed documents by normalized path, supports active-buffer
  insertion/removal, `reference_at` hit-testing for definition, anchor lookup,
  `anchor_at` hit-testing for references, and `references_to` scanning across
  indexed documents. It does no file I/O itself.
- All ranges are `std::ops::Range<usize>` byte offsets. No lsp_types here.

`crates/djot-export/src/main.rs` (bin `djot-export`, depends on `djot-core` + `pandoc_types` + `serde_json` + `toml`; requires the `pandoc` executable at runtime):

- Reads djot (file arg or stdin), invokes `pandoc -f djot -t json`, parses the
  resulting pandoc JSON with `pandoc_types`, and prints the transformed pandoc
  JSON AST.
- This is **where conventions become export semantics**: the first
  `{.metadata}` code block is parsed as toml and folded into pandoc `Meta`
  instead of rendered in the body, so its information is preserved rather than
  dropped. Pandoc owns the Djot syntax conversion.
- Unit tests live in the same file and cover the pandoc AST metadata
  transformation. The CLI round-trip requires `pandoc`.
- Verify with a round-trip: `printf '# H\n' | ./target/debug/djot-export | pandoc -f json -t markdown`.

`crates/djot-filter/src/main.rs` (bin `djot-filter`, depends on `djot-core` + `cel` + `clap` + `shlex` + `skim` + `toml`):

- Recursively scans a root directory for `.dj` / `.djot` files, loads them into
  `djot_core::Workspace`, and prints root-relative paths that match all
  filters.
- `--query EXPR` compiles a CEL predicate once and evaluates it against each
  candidate document. The query context exposes root-relative `path`, metadata
  `title`, `directly_referenced_by`, and `transitively_referenced_by`. Reference
  lists contain root-relative paths of documents that link to the current
  document; the transitive list includes direct referrers.
- `--interactive` opens the filtered results in skim. Each item displays and
  outputs the root-relative path, matches against `path + full text`, and uses
  an in-memory ANSI-highlighted preview of the file content instead of a shell
  preview command. The item list highlights paths, while search text remains
  plain `path + full text`.
  When the user accepts a selection, `djot-filter` opens selected files with
  `$EDITOR`; editor arguments are parsed with `shlex` and file paths are passed
  as direct process arguments so spaces are preserved. In skim, `ctrl-n`
  creates a new file from the current query relative to the scan root, rejects
  empty or root-escaping paths, adds `.dj` when the query lacks a `.dj` /
  `.djot` extension, and opens the created file with `$EDITOR`.
- `tasks` queries expose task `title`, `created`, `due`, `done`, `recur`, and
  `prev` fields.
- Unit tests live in the same file and cover CEL query behavior, reverse
  reference predicates, skim item behavior, editor command handling, and file
  creation.

`crates/djot-ls/src/main.rs` (bin `djot-ls`, depends on `djot-core`):

- `ServerState` holds `client: ClientSocket` and a `djot_core::Workspace` (path-keyed parsed documents). Because the omni-trait gives handlers `&mut self`, the index needs **no locking** — this is the main reason async-lsp was chosen over tower-lsp. URIs are mapped to/from paths with `Url::to_file_path`/`from_file_path` (file URIs only for now).
- Text sync is **FULL** (`TextDocumentSyncKind::FULL`): `did_change`
  reparses the whole document into the workspace; `did_close` restores the
  disk-indexed version for workspace files and removes non-workspace buffers.
- Advertised capabilities are currently `textDocument/documentSymbol`,
  `textDocument/definition`, `textDocument/references`, and
  `textDocument/hover`.
- `initialize` records client-provided `workspaceFolders`, falling back to
  `rootUri` for older clients. `initialized` then indexes `.dj` / `.djot` files
  under those roots and reports work-done progress with `$/progress`. With no
  client root, it indexes only opened buffers and lazily loaded definition
  targets.
- `document_symbol` calls `heading_outline` on the stored text then maps each `Heading` to `DocumentSymbol`; `definition` (`resolve_definition`) hit-tests the cursor against link spans via the workspace, `resolve_target`s the link, lazily loads a cross-file target from disk if unseen, and returns the anchor's `Location`. Same-file and cross-file links go through the same path; external URLs return nothing. `references` resolves either the anchor or link under the cursor, then returns locations from `Workspace::references_to`, optionally including the anchor declaration. `hover` resolves the anchor or link under the cursor and shows target kind, id, path:line, and a djot source preview.
- `offset_to_position`/`position_to_offset` convert between byte offsets and LSP `Position` (UTF-16 columns) — O(n) per call, fine for now, worth precomputing line starts if it shows up in profiles.
- `main()` wires the tower middleware stack (`Tracing`/`Lifecycle`/`CatchUnwind`/`Concurrency`/`ClientProcessMonitor`) around the router and runs `run_buffered` over real async stdio (`PipeStdin/PipeStdout::lock_tokio`). Tracing goes to **stderr** (stdout is the LSP transport).

## Testing approach

`crates/djot-ls/tests/document_symbol.rs` is **black-box**: it spawns the built binary (`env!("CARGO_BIN_EXE_djot-ls")`) and drives a full `initialize → didOpen → … → shutdown → exit` JSON-RPC session over stdio, parsing `Content-Length`-framed responses with `serde_json`. This is how the didSave-crash regression and the symbol/definition behavior are tested end to end.

Pure semantics (`heading_outline`, `build_index`, `parse_dst`,
`resolve_target`, `Workspace`) are unit-tested directly in `djot-core`
(`#[cfg(test)] mod tests` in its `lib.rs`) — faster and more precise.
Exporter behavior is unit-tested in `crates/djot-export/src/main.rs`. Filter
behavior is unit-tested in `crates/djot-filter/src/main.rs`.

## Editor testing

`dev/nvim.lua` registers the server for the `djot` filetype via Neovim's `vim.lsp.config`/`vim.lsp.enable` (cmd `./target/debug/djot-ls`). Build first, then open a `.dj` file. Server-side panics surface in Neovim's `:LspLog` (the binary's stderr is captured there).

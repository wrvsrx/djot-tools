---
name: djot-semantic-notes
description: Edit Djot note repositories that follow djot-tools semantics. Use when creating, modifying, completing, canceling, recurring, or dependency-linking tasks in .dj/.djot notes; preserving djot-tools task attributes and reference spelling; writing the metadata definition list or [@key]{.cite} citations for djot-export; or helping an LLM operate on notes written for djot-ls, djot-notes, or djot-export.
---

# Djot Semantic Notes

Use this skill when editing Djot notes that follow the semantics implemented by
the same released version of `djot-tools`.

## Authority

Treat this bundled skill as the portable LLM guide for the release that shipped
it. If working inside the `djot-tools` source repository, prefer
`docs/semantics.dj` whenever it conflicts with this skill.

## Core Rules

- Preserve ordinary Djot syntax and `.dj` / `.djot` file conventions.
- Use explicit anchors and relative Djot links for semantic references.
- Do not invent semantics that `djot-tools` does not implement.
- Keep user text, ordering, indentation style, and non-task attributes unless a
  requested edit requires changing them.
- For base Djot syntax questions, also use the bundled `djot-markup` skill.
- When applying task changes, read `references/tasks.md` first.
- When writing notes for `djot-export` (metadata definition list, citations), read
  `references/export.md` first.

## References

- `references/tasks.md`: task blocks, metadata fields, recurrence, dependencies,
  and examples.
- `references/export.md`: `djot-export` semantics — the `{.metadata}` definition list and
  `[@key]{.cite}` citations.

---
name: djot-markup
description: Write, edit, review, or convert Djot markup files. Use for .dj and .djot files when preserving Djot syntax, avoiding Markdown-only habits, using Djot attributes, fenced divs, links, tables, code blocks, or other base markup constructs.
---

# Djot Markup

Djot is a lightweight markup syntax derived from CommonMark, but normal editing
should preserve intended structure and reader-facing output rather than overfit
to Markdown habits.

## Core Workflow

- Treat `.dj` and `.djot` files as Djot, not Markdown.
- Preserve nearby style when editing existing files.
- Prefer concise, readable source markup over clever syntax.
- If a repository also uses `djot-tools` semantic notes, use the
  `djot-semantic-notes` skill for task and reference semantics.

## Markdown Differences

- Put blank lines around block-level elements: headings, block quotes, code
  blocks, thematic breaks, lists, tables, divs, and paragraphs.
- Always put a blank line before a list, including a nested list.
- Use only ATX headings: `#`, `##`, etc. Djot has no Setext headings.
- Leave a blank line after headings; Djot headings may span multiple lines.
- Trailing `#` characters in headings are literal content, not closing markers.
- Use fenced code blocks only. Djot has no indented code blocks.
- Block quotes need `> ` with a space after `>`, unless `>` is followed by a
  newline.
- Use `_emphasis_` for emphasis and `*strong*` for strong emphasis.
- Use backslash-newline for a hard line break, not two trailing spaces.
- Raw HTML must be explicit: inline `` `<span>`{=html} `` or a code fence with
  a language specifier such as `=html`.
- Pipe tables must start and end every row with `|`.
- Link titles use attributes: `[text](url){title="Title"}`.
- Reference link labels are case-sensitive.
- Shortcut reference links like `[text]` are not links by themselves.

## Common Syntax

Inline:

```djot
[text](https://example.com)
[text][ref]
[text][]
![alt](image.png)
`verbatim`
_emphasis_
*strong*
{=highlight=}
{+inserted+}
{-deleted-}
H~2~O and x^2^
$inline math$
[^note]
[span text]{.class #id key="value"}
```

Blocks:

````djot
# Heading

> Block quote

- list item

  - nested item

1. ordered item

- [ ] task
- [x] done

: term

  definition

```lua
print("code")
```

::: warning
A fenced div with block content.
:::

| name | value |
|------|------:|
| a    |     1 |

[^note]: Footnote content.

{#custom-id .important}
Paragraph with block attributes.
````

## Attributes And Containers

- Attach inline attributes immediately after the inline element:
  `*word*{.important}`.
- Attach block attributes on the line immediately before the block.
- Attribute shorthand is `{#id .class key="value"}`.
- Use bracketed spans for arbitrary inline content: `[text]{.class}`.
- Use fenced divs for arbitrary block content: `::: class` plus block content
  and a closing `:::`.
- Raw inline content uses `{=format}` after verbatim content.
- Raw block content uses a code fence language of `=format`.

## Ambiguity Rules

- Inline containers cannot overlap; the first opener that gets closed wins.
- Use `{_` and `_}` or `{*` and `*}` to force opener or closer interpretation
  when emphasis or strong emphasis is ambiguous.
- Verbatim spans do not parse nested markup.
- If a verbatim span is not closed before the inline context ends, it extends to
  the end of that context.

## Tables

- Every row starts and ends with `|`.
- Separator rows use `-` with optional `:` for alignment.
- Alignment markers: `:---` left, `---:` right, `:---:` center, `---` default.
- Cell contents are inline only; do not put block content inside pipe table
  cells.
- Escape literal pipes as `\|` unless they are inside verbatim spans.

## Source References

When uncertain, consult the official Djot sources:

- https://djot.net/
- https://github.com/jgm/djot/blob/main/doc/quickstart-for-markdown-users.md
- https://htmlpreview.github.io/?https://github.com/jgm/djot/blob/master/doc/syntax.html

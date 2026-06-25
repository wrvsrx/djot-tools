//! Layer A: semantics-agnostic djot *syntax* analysis layered on jotdown.
//!
//! jotdown yields parsed values but not the source byte ranges that edits need
//! — which colons open a fenced div, where in a line a new attribute block
//! belongs. This module recovers those ranges from jotdown's spans without
//! knowing what the syntax *means* (tasks, metadata, references); the semantic
//! layers read the ranges instead of re-scanning the source. Keeping it free of
//! project semantics is deliberate, so it can later be lifted into a standalone
//! djot syntax crate.

use std::ops::Range;

use jotdown::Parser;

/// The id djot derives for a heading with the given `title` text, following
/// jotdown's slugging rules. A syntactic operation (no project semantics), kept
/// here so the rest of the crate need not parse djot itself.
pub fn heading_id(title: &str) -> Option<String> {
    let source = format!("# {}\n", title.trim());
    Parser::new(&source).find_map(|event| match event {
        jotdown::Event::Start(jotdown::Container::Heading { id, .. }, _) => Some(id.into_owned()),
        _ => None,
    })
}

// ---- Event stream ----------------------------------------------------------

/// Attributes attached to a container, owned and decoupled from jotdown so that
/// semantic layers never name the parser type.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Attributes {
    pairs: Vec<(String, String)>,
}

impl Attributes {
    /// The value of `key`, following jotdown's resolution (last value wins;
    /// `class` is the concatenation of all class tokens).
    pub fn get_value(&self, key: &str) -> Option<&str> {
        self.pairs
            .iter()
            .find(|(existing, _)| existing == key)
            .map(|(_, value)| value.as_str())
    }

    /// Whether the container carries the given class token.
    pub fn has_class(&self, class: &str) -> bool {
        self.get_value("class")
            .is_some_and(|value| value.split_whitespace().any(|token| token == class))
    }

    fn from_jotdown(attrs: &jotdown::Attributes) -> Self {
        let mut pairs: Vec<(String, String)> = Vec::new();
        for (key, _) in attrs.unique_pairs() {
            if pairs.iter().any(|(existing, _)| existing == key) {
                continue;
            }
            if let Some(value) = attrs.get_value(key) {
                pairs.push((key.to_string(), value.to_string()));
            }
        }
        Self { pairs }
    }
}

/// A block or inline container. Mirrors the jotdown variants the analysis
/// distinguishes; everything else collapses to [`Container::Other`], which still
/// pairs Start/End so nesting is preserved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Container {
    Section,
    Heading { level: u16, id: String },
    Div { class: String },
    ListItem,
    TaskListItem { checked: bool },
    Link { dst: String },
    Paragraph,
    CodeBlock,
    Other,
}

/// A parse event carrying the owned data and (via [`parse`]) source span the
/// analysis needs. Inline text and events not relevant to analysis flatten into
/// [`Event::Other`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Start(Container, Attributes),
    End(Container),
    Str(String),
    Softbreak,
    Hardbreak,
    Other,
}

/// Parse `text` into the cst event stream, each event paired with its source
/// byte span. This is the single place jotdown drives the walk; semantic layers
/// consume these events instead of the parser.
pub fn parse(text: &str) -> impl Iterator<Item = (Event, Range<usize>)> + '_ {
    Parser::new(text)
        .into_offset_iter()
        .map(|(event, span)| (convert_event(event), span))
}

fn convert_event(event: jotdown::Event) -> Event {
    match event {
        jotdown::Event::Start(container, attrs) => Event::Start(
            convert_container(container),
            Attributes::from_jotdown(&attrs),
        ),
        jotdown::Event::End(container) => Event::End(convert_container(container)),
        jotdown::Event::Str(text) => Event::Str(text.into_owned()),
        jotdown::Event::Softbreak => Event::Softbreak,
        jotdown::Event::Hardbreak => Event::Hardbreak,
        _ => Event::Other,
    }
}

fn convert_container(container: jotdown::Container) -> Container {
    match container {
        jotdown::Container::Section { .. } => Container::Section,
        jotdown::Container::Heading { level, id, .. } => Container::Heading {
            level,
            id: id.into_owned(),
        },
        jotdown::Container::Div { class } => Container::Div {
            class: class.into_owned(),
        },
        jotdown::Container::ListItem => Container::ListItem,
        jotdown::Container::TaskListItem { checked } => Container::TaskListItem { checked },
        jotdown::Container::Link(dst, _) => Container::Link {
            dst: dst.into_owned(),
        },
        jotdown::Container::Paragraph => Container::Paragraph,
        jotdown::Container::CodeBlock { .. } => Container::CodeBlock,
        _ => Container::Other,
    }
}

/// Syntactic anchors of a fenced div's opening fence that edits use to place a
/// new attribute block.
///
/// Recovered from the div's own span, so it is independent of the fence's colon
/// count or list nesting: a nested `:::: task` resolves to itself rather than to
/// an inner `::: task`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DivFence {
    /// Byte span of the opening fence line itself (e.g. `:::: task`).
    pub fence_range: Range<usize>,
    /// Range to replace when inserting a new `{...}` attribute line. Empty (an
    /// insertion point) for a bare fence; the `<indent>- ` marker for the
    /// compact `- ::: task` list form, which `attribute_prefix` re-emits.
    pub attribute_insert: Range<usize>,
    /// Prefix emitted before the first inserted attribute line.
    pub attribute_prefix: String,
    /// Prefix emitted before any subsequent inserted attribute line.
    pub continued_attribute_prefix: String,
    /// Prefix re-emitted before the fence after the inserted line(s).
    pub fence_prefix: String,
    /// Indent of the div's content (fence indent plus any list-marker width).
    pub indent: String,
}

/// Resolve the opening fence of the div whose source range is `div_span`.
///
/// `div_span` is jotdown's span for the div, which also covers the block
/// attribute lines preceding the fence, so we scan forward to the first fence
/// line rather than assuming `div_span.start` is the fence. Fence detection is
/// colon-count agnostic, which is what keeps a 4-colon outer fence from being
/// skipped in favour of a 3-colon inner one.
pub fn div_fence(text: &str, div_span: &Range<usize>) -> Option<DivFence> {
    let mut offset = div_span.start;
    while offset <= div_span.end {
        let (line_start, line_end) = line_bounds(text, offset)?;
        let line = text.get(line_start..line_end)?;
        if let Some(fence) = div_fence_from_line(line_start, line) {
            return Some(fence);
        }
        if line_end >= div_span.end || line_end == text.len() {
            break;
        }
        offset = next_line_start(text, line_end)?;
    }
    None
}

fn div_fence_from_line(line_start: usize, line: &str) -> Option<DivFence> {
    let line = line.strip_suffix('\r').unwrap_or(line);
    let fence_range = line_start..line_start + line.len();
    let indent = leading_indent(line);
    let rest = &line[indent.len()..];

    if is_div_fence(rest) {
        return Some(DivFence {
            fence_range,
            attribute_insert: line_start..line_start,
            attribute_prefix: indent.to_string(),
            continued_attribute_prefix: indent.to_string(),
            fence_prefix: String::new(),
            indent: indent.to_string(),
        });
    }

    if !is_div_fence(rest.strip_prefix("- ")?) {
        return None;
    }
    let marker_end = line_start + indent.len() + "- ".len();
    Some(DivFence {
        fence_range,
        attribute_insert: line_start..marker_end,
        attribute_prefix: format!("{indent}- "),
        continued_attribute_prefix: format!("{indent}  "),
        fence_prefix: format!("{indent}  "),
        indent: format!("{indent}  "),
    })
}

/// Whether `rest` (a line with its indent and any list marker stripped) opens a
/// djot fenced div: three or more colons.
fn is_div_fence(rest: &str) -> bool {
    rest.bytes().take_while(|&byte| byte == b':').count() >= 3
}

pub(crate) fn leading_indent(line: &str) -> &str {
    let indent_len = line
        .char_indices()
        .find(|(_, c)| *c != ' ' && *c != '\t')
        .map(|(i, _)| i)
        .unwrap_or(line.len());
    &line[..indent_len]
}

pub(crate) fn line_bounds(text: &str, offset: usize) -> Option<(usize, usize)> {
    if offset > text.len() {
        return None;
    }
    let start = text[..offset].rfind('\n').map_or(0, |i| i + 1);
    let end = text[offset..].find('\n').map_or(text.len(), |i| offset + i);
    Some((start, end))
}

pub(crate) fn previous_line_start(text: &str, line_start: usize) -> Option<usize> {
    if line_start == 0 {
        return None;
    }
    let previous_end = line_start.checked_sub('\n'.len_utf8())?;
    Some(text[..previous_end].rfind('\n').map_or(0, |i| i + 1))
}

pub(crate) fn next_line_start(text: &str, line_end: usize) -> Option<usize> {
    if line_end >= text.len() {
        None
    } else {
        Some(line_end + '\n'.len_utf8())
    }
}

// ---- Attribute syntax ------------------------------------------------------

/// The shape of a single djot attribute token inside a `{...}` block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttrKind {
    /// `#id` shorthand or an `id="..."` pair.
    Id,
    /// `.class` shorthand.
    Class,
    /// A `key="value"` / `key=value` pair (any key other than `id`).
    Pair,
    /// A `%...%` comment.
    Comment,
}

/// One attribute token inside a `{...}` block, with its source byte ranges.
///
/// jotdown parses attribute *values* but not their spans; this records the
/// spans recovered from the source so semantic layers can locate ids, values,
/// and whole tokens for edits without re-scanning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpannedAttribute {
    pub kind: AttrKind,
    /// The whole token, e.g. `key="v"`, `#id`, `.class`, `%c%`.
    pub token_range: Range<usize>,
    /// The key bytes of a pair (including the `id` of an `id=` pair); `None`
    /// for shorthand `#`/`.` tokens and comments.
    pub key_range: Option<Range<usize>>,
    /// The value bytes: the id/class name, or the unquoted inner of a pair
    /// value; `None` for comments and bare keys.
    pub value_range: Option<Range<usize>>,
}

impl SpannedAttribute {
    fn text<'a>(&self, range: &Option<Range<usize>>, text: &'a str) -> Option<&'a str> {
        range.as_ref().and_then(|range| text.get(range.clone()))
    }

    /// The key as a string slice of `text`, if this token has one.
    pub fn key<'a>(&self, text: &'a str) -> Option<&'a str> {
        self.text(&self.key_range, text)
    }

    /// The value as a string slice of `text`, if this token has one.
    pub fn value<'a>(&self, text: &'a str) -> Option<&'a str> {
        self.text(&self.value_range, text)
    }
}

/// Parse a single `{...}` block into its attribute tokens. `brace_range` spans
/// the braces inclusively (`start` at `{`, `end` one past `}`).
pub fn attribute_block(text: &str, brace_range: &Range<usize>) -> Vec<SpannedAttribute> {
    if brace_range.end <= brace_range.start + 1 {
        return Vec::new();
    }
    parse_attributes(text, (brace_range.start + 1)..(brace_range.end - 1))
}

/// Locate every `{...}` attribute block within `span` and parse them, flattened
/// in source order. Brace matching is quote/escape/comment aware, so a `}` or
/// `%` inside a quoted value does not terminate the block early.
pub fn attribute_blocks(text: &str, span: &Range<usize>) -> Vec<SpannedAttribute> {
    let bytes = text.as_bytes();
    let end = span.end.min(text.len());
    let mut out = Vec::new();
    let mut i = span.start;
    while i < end {
        if bytes[i] == b'{' {
            if let Some(close) = attribute_block_end(bytes, i, end) {
                out.extend(parse_attributes(text, (i + 1)..close));
                i = close + 1;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// Index of the `}` that closes the block opened at `open`, respecting quoted
/// values and `%...%` comments.
fn attribute_block_end(bytes: &[u8], open: usize, end: usize) -> Option<usize> {
    let mut i = open + 1;
    let mut quote: Option<u8> = None;
    while i < end {
        let byte = bytes[i];
        match quote {
            Some(active) => {
                if byte == b'\\' {
                    i += 1;
                } else if byte == active {
                    quote = None;
                }
            }
            None => match byte {
                b'"' | b'\'' => quote = Some(byte),
                b'%' => {
                    i += 1;
                    while i < end && bytes[i] != b'%' {
                        i += 1;
                    }
                }
                b'}' => return Some(i),
                _ => {}
            },
        }
        i += 1;
    }
    None
}

fn parse_attributes(text: &str, inner: Range<usize>) -> Vec<SpannedAttribute> {
    let bytes = text.as_bytes();
    let end = inner.end.min(text.len());
    let mut i = inner.start;
    let mut out = Vec::new();
    while i < end {
        while i < end && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= end {
            break;
        }
        let token_start = i;
        match bytes[i] {
            b'%' => {
                i += 1;
                while i < end && bytes[i] != b'%' {
                    i += 1;
                }
                if i < end {
                    i += 1;
                }
                out.push(SpannedAttribute {
                    kind: AttrKind::Comment,
                    token_range: token_start..i,
                    key_range: None,
                    value_range: None,
                });
            }
            b'.' => {
                let value_start = i + 1;
                i = value_start;
                while i < end && is_attr_name_byte(bytes[i]) {
                    i += 1;
                }
                out.push(SpannedAttribute {
                    kind: AttrKind::Class,
                    token_range: token_start..i,
                    key_range: None,
                    value_range: Some(value_start..i),
                });
            }
            b'#' => {
                let value_start = i + 1;
                i = value_start;
                while i < end && is_attr_name_byte(bytes[i]) {
                    i += 1;
                }
                out.push(SpannedAttribute {
                    kind: AttrKind::Id,
                    token_range: token_start..i,
                    key_range: None,
                    value_range: Some(value_start..i),
                });
            }
            byte if is_attr_name_byte(byte) => {
                let key_start = i;
                i += 1;
                while i < end && is_attr_name_byte(bytes[i]) {
                    i += 1;
                }
                let key_range = key_start..i;
                if i >= end || bytes[i] != b'=' {
                    out.push(SpannedAttribute {
                        kind: AttrKind::Pair,
                        token_range: token_start..i,
                        key_range: Some(key_range),
                        value_range: None,
                    });
                    continue;
                }
                i += 1; // consume '='
                let value_range = attribute_value(bytes, &mut i, end);
                let kind = if text.get(key_range.clone()) == Some("id") {
                    AttrKind::Id
                } else {
                    AttrKind::Pair
                };
                out.push(SpannedAttribute {
                    kind,
                    token_range: token_start..i,
                    key_range: Some(key_range),
                    value_range: Some(value_range),
                });
            }
            _ => {
                i += 1;
            }
        }
    }
    out
}

fn attribute_value(bytes: &[u8], i: &mut usize, end: usize) -> Range<usize> {
    if *i < end && (bytes[*i] == b'"' || bytes[*i] == b'\'') {
        let quote = bytes[*i];
        *i += 1;
        let value_start = *i;
        while *i < end {
            match bytes[*i] {
                b'\\' => {
                    *i += 1;
                    if *i < end {
                        *i += 1;
                    }
                }
                byte if byte == quote => break,
                _ => *i += 1,
            }
        }
        let value_end = *i;
        if *i < end {
            *i += 1; // consume closing quote
        }
        value_start..value_end
    } else {
        let value_start = *i;
        while *i < end && is_attr_name_byte(bytes[*i]) {
            *i += 1;
        }
        value_start..*i
    }
}

pub(crate) fn is_attr_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'_' | b'-')
}

// ---- Links -----------------------------------------------------------------

/// Syntactic spans within an inline link `[label](destination)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkSyntax {
    /// Byte span of the destination text inside the parentheses, with any
    /// `<...>` angle brackets stripped.
    pub dst_range: Range<usize>,
}

/// Locate the destination span of the inline link covering `link_span`.
///
/// Returns `None` for reference/collapsed/autolink forms that carry no inline
/// `](destination)`, where the destination is not present at the link site.
pub fn link_syntax(text: &str, link_span: &Range<usize>) -> Option<LinkSyntax> {
    let source = text.get(link_span.clone())?;
    let open = source.rfind("](")?;
    let dst_rel = open + "](".len();
    let close_rel = dst_rel + source.get(dst_rel..)?.rfind(')')?;
    let mut start = link_span.start + dst_rel;
    let mut end = link_span.start + close_rel;
    if end < start {
        return None;
    }
    let bytes = text.as_bytes();
    if end > start && bytes.get(start) == Some(&b'<') && bytes.get(end - 1) == Some(&b'>') {
        start += 1;
        end -= 1;
    }
    Some(LinkSyntax {
        dst_range: start..end,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(text: &str) -> Vec<SpannedAttribute> {
        attribute_block(text, &(0..text.len()))
    }

    #[test]
    fn parses_shorthand_classes_ids_and_pairs() {
        let text = "{.work #the-id key=\"a value\" bare=x}";
        let attrs = block(text);
        let kinds: Vec<_> = attrs.iter().map(|attr| attr.kind).collect();
        assert_eq!(
            kinds,
            vec![
                AttrKind::Class,
                AttrKind::Id,
                AttrKind::Pair,
                AttrKind::Pair
            ]
        );
        assert_eq!(attrs[0].value(text), Some("work"));
        assert_eq!(attrs[1].value(text), Some("the-id"));
        assert_eq!(attrs[2].key(text), Some("key"));
        assert_eq!(attrs[2].value(text), Some("a value"));
        assert_eq!(&text[attrs[2].token_range.clone()], "key=\"a value\"");
        assert_eq!(attrs[3].value(text), Some("x"));
    }

    #[test]
    fn id_pair_is_classified_as_id() {
        let text = "{id=\"x-1\"}";
        let attrs = block(text);
        assert_eq!(attrs.len(), 1);
        assert_eq!(attrs[0].kind, AttrKind::Id);
        assert_eq!(attrs[0].key(text), Some("id"));
        assert_eq!(attrs[0].value(text), Some("x-1"));
    }

    #[test]
    fn comments_and_escaped_quotes_are_preserved() {
        let text = "{%a comment% note=\"esc \\\" end\" tag='two words'}";
        let attrs = block(text);
        assert_eq!(attrs[0].kind, AttrKind::Comment);
        assert_eq!(&text[attrs[0].token_range.clone()], "%a comment%");
        assert_eq!(attrs[1].value(text), Some("esc \\\" end"));
        assert_eq!(attrs[2].value(text), Some("two words"));
    }

    #[test]
    fn blocks_skip_braces_inside_quoted_values() {
        let text = "x {k=\"a}b\"} y {#id}";
        let attrs = attribute_blocks(text, &(0..text.len()));
        assert_eq!(attrs.len(), 2);
        assert_eq!(attrs[0].value(text), Some("a}b"));
        assert_eq!(attrs[1].kind, AttrKind::Id);
        assert_eq!(attrs[1].value(text), Some("id"));
    }

    #[test]
    fn div_fence_is_colon_count_agnostic() {
        let text = ":::: task\nbody\n::::\n";
        let fence = div_fence(text, &(0..text.len())).unwrap();
        assert_eq!(&text[fence.fence_range.clone()], ":::: task");
        assert_eq!(fence.attribute_insert, 0..0);
    }

    #[test]
    fn parse_exposes_div_class_and_attributes() {
        let text = "{#anchor key=\"v\"}\n:::: task\nbody\n::::\n";
        let events: Vec<_> = parse(text).map(|(event, _)| event).collect();
        let attrs = events
            .iter()
            .find_map(|event| match event {
                Event::Start(Container::Div { class }, attrs) if class == "task" => Some(attrs),
                _ => None,
            })
            .expect("task div start");
        assert_eq!(attrs.get_value("id"), Some("anchor"));
        assert_eq!(attrs.get_value("key"), Some("v"));
        // The fence class is `Container::Div.class`, not an attribute class.
        assert!(!attrs.has_class("task"));
    }

    #[test]
    fn link_syntax_finds_destination_and_strips_angle_brackets() {
        let text = "see [the label](dir/file.dj#sec) here";
        let span = text.find('[').unwrap()..text.find(')').unwrap() + 1;
        let link = link_syntax(text, &span).unwrap();
        assert_eq!(&text[link.dst_range.clone()], "dir/file.dj#sec");

        let angled = "[x](<a b.dj>)";
        let link = link_syntax(angled, &(0..angled.len())).unwrap();
        assert_eq!(&angled[link.dst_range], "a b.dj");

        // Reference-style links carry no inline destination.
        let reference = "[x][ref]";
        assert_eq!(link_syntax(reference, &(0..reference.len())), None);
    }
}

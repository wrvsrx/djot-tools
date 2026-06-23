use std::collections::HashMap;
use std::ops::Range;
use std::path::Path;

use jotdown::{Attributes, Container, Event, Parser};
use serde::{Deserialize, Serialize};

use crate::diagnostics::{AnalysisDiagnostic, DiagnosticKind};
use crate::edits::TextEdit;
use crate::references::{
    parse_dst, reference_target_id_range, reference_target_path_range, task_dependencies,
    task_dependency_references, task_prev_reference, Reference, ReferenceKind,
};
use crate::tasks::{document_local_task_diagnostics, Task, TaskDependency};
use crate::{METADATA_CLASS, TASK_CLASS};

/// A heading node in the document outline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heading {
    /// Heading text. Empty if the heading has no textual content.
    pub name: String,
    /// Heading level (1–6).
    pub level: u16,
    /// The whole section span: heading line + body + nested subsections.
    pub range: Range<usize>,
    /// The heading line itself (a good "selection"/jump target).
    pub selection_range: Range<usize>,
    /// Subsections nested under this heading.
    pub children: Vec<Heading>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeTaskListItem {
    pub range: Range<usize>,
    pub title_range: Option<Range<usize>>,
    pub title: String,
    pub checked: bool,
}

/// A jump target: anything bearing an id — a heading/section, or any block or
/// inline carrying an explicit `{#id}` attribute.
///
/// Derives `Serialize`/`Deserialize` so a persistent on-disk index cache can be
/// layered on later without touching this type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Anchor {
    /// Byte span to jump to (the heading or anchored line).
    pub range: Range<usize>,
    /// Byte span of the anchor id/name that should be replaced by rename.
    pub rename_range: Range<usize>,
    /// Whether the id is explicit source syntax (`{#id}`) rather than an
    /// implicit id generated from heading text.
    pub explicit: bool,
}

/// Per-document index of anchors (by id) and outgoing references.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocIndex {
    pub anchors: HashMap<String, Anchor>,
    pub references: Vec<Reference>,
}

/// Shared per-document analysis used by workspace-level tools.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Analysis {
    pub index: DocIndex,
    pub metadata: Option<String>,
    pub tasks: Vec<Task>,
    pub native_task_list_items: Vec<NativeTaskListItem>,
    /// Document-local diagnostics. Workspace-dependent diagnostics, such as
    /// unresolved cross-file references, are added by [`Workspace`].
    pub diagnostics: Vec<AnalysisDiagnostic>,
}

/// Build the heading hierarchy for `text`.
///
/// jotdown wraps each heading in a `Section` container that nests by heading
/// level, so the section nesting *is* the outline hierarchy. Each section's span
/// becomes [`Heading::range`] and the heading line becomes
/// [`Heading::selection_range`].
pub fn heading_outline(text: &str) -> Vec<Heading> {
    let mut roots: Vec<Heading> = Vec::new();
    let mut stack: Vec<SectionFrame> = Vec::new();

    for (event, span) in Parser::new(text).into_offset_iter() {
        match event {
            Event::Start(Container::Section { .. }, _) => {
                stack.push(SectionFrame::new(span.start));
            }
            Event::Start(Container::Heading { level, .. }, _) => {
                // Only the first heading directly inside a section is that
                // section's title; ignore headings in nested non-section blocks.
                if let Some(top) = stack.last_mut() {
                    if !top.captured {
                        top.level = level;
                        top.selection = span.clone();
                        top.capturing = true;
                    }
                }
            }
            Event::Str(s) => {
                if let Some(top) = stack.last_mut() {
                    if top.capturing {
                        top.name.push_str(&s);
                    }
                }
            }
            Event::End(Container::Heading { .. }) => {
                if let Some(top) = stack.last_mut() {
                    if top.capturing {
                        top.capturing = false;
                        top.captured = true;
                        top.selection.end = span.end;
                    }
                }
            }
            Event::End(Container::Section { .. }) => {
                if let Some(frame) = stack.pop() {
                    let heading = frame.into_heading(span.end);
                    match stack.last_mut() {
                        Some(parent) => parent.children.push(heading),
                        None => roots.push(heading),
                    }
                }
            }
            _ => {}
        }
    }

    roots
}

/// Walk the document once, collecting anchors and references.
pub fn build_index(text: &str) -> DocIndex {
    analyze(text).index
}

/// Analyze one document, collecting shared semantic data in one parser pass.
pub fn analyze(text: &str) -> Analysis {
    let mut anchors: HashMap<String, Anchor> = HashMap::new();
    let mut seen_anchor_ranges: HashMap<String, Range<usize>> = HashMap::new();
    let mut references = Vec::new();
    let mut tasks = Vec::new();
    let mut native_task_list_items = Vec::new();
    let mut diagnostics = Vec::new();
    let mut metadata = None;
    let mut metadata_capture: Option<String> = None;
    let mut open_headings: Vec<HeadingAnchorFrame> = Vec::new();
    let mut open_links: Vec<(String, usize)> = Vec::new();
    let mut task_stack: Vec<TaskFrame> = Vec::new();
    let mut native_task_stack: Vec<NativeTaskFrame> = Vec::new();
    let mut list_item_metadata: Vec<TaskMetadata> = Vec::new();

    for (event, span) in Parser::new(text).into_offset_iter() {
        match event {
            Event::Start(Container::Heading { id, .. }, _) => {
                open_headings.push(HeadingAnchorFrame {
                    id: id.into_owned(),
                    start: span.start,
                    text_range: None,
                });
            }
            Event::Start(container, attrs) => {
                if let Some(id) = attrs.get_value("id") {
                    let id = id.to_string();
                    let rename_range =
                        anchor_id_range(text, &span, &id).unwrap_or_else(|| span.clone());
                    insert_anchor(
                        &mut anchors,
                        &mut seen_anchor_ranges,
                        &mut diagnostics,
                        id,
                        Anchor {
                            range: span.clone(),
                            rename_range,
                            explicit: true,
                        },
                    );
                }

                match &container {
                    Container::CodeBlock { .. }
                        if metadata.is_none()
                            && metadata_capture.is_none()
                            && has_class(&attrs, METADATA_CLASS) =>
                    {
                        metadata_capture = Some(String::new());
                    }
                    Container::ListItem | Container::TaskListItem { .. } => {
                        list_item_metadata.push(TaskMetadata::from_attributes(text, &span, &attrs));
                        if let Container::TaskListItem { checked } = &container {
                            native_task_stack.push(NativeTaskFrame {
                                range_start: span.start,
                                checked: *checked,
                                capturing_title: false,
                                captured_title: false,
                                title_range: None,
                                title: String::new(),
                            });
                        }
                    }
                    Container::Div { class } if class == TASK_CLASS => {
                        if let Some(reference) = task_prev_reference(text, &span, &attrs) {
                            references.push(reference);
                        }
                        references.extend(task_dependency_references(text, &span, &attrs));

                        let inherited = list_item_metadata.last();
                        let task_metadata = TaskMetadata::from_attributes_with_fallback(
                            text, &span, &attrs, inherited,
                        );
                        let depth = task_stack.len();
                        task_stack.push(TaskFrame {
                            range_start: span.start,
                            depth,
                            id: task_metadata.id,
                            created: task_metadata.created,
                            done: task_metadata.done,
                            canceled: task_metadata.canceled,
                            due: task_metadata.due,
                            wait: task_metadata.wait,
                            recur: task_metadata.recur,
                            prev: task_metadata.prev,
                            depends: task_metadata.depends,
                            capturing_title: false,
                            captured_title: false,
                            title_range: None,
                            title: String::new(),
                        });
                    }
                    Container::Link(dst, _) => {
                        open_links.push((dst.to_string(), span.start));
                    }
                    Container::Paragraph => {
                        if let Some(frame) = task_stack.last_mut() {
                            if !frame.capturing_title && !frame.captured_title {
                                frame.capturing_title = true;
                            }
                        }
                        if let Some(frame) = native_task_stack.last_mut() {
                            if !frame.capturing_title && !frame.captured_title {
                                frame.capturing_title = true;
                            }
                        }
                    }
                    _ => {}
                }
            }
            Event::Str(s) => {
                if let Some(heading) = open_headings.last_mut() {
                    match &mut heading.text_range {
                        Some(range) => range.end = span.end,
                        None => heading.text_range = Some(span.clone()),
                    }
                }
                if let Some(frame) = task_stack.last_mut() {
                    if frame.capturing_title {
                        frame.title.push_str(&s);
                        match &mut frame.title_range {
                            Some(range) => range.end = span.end,
                            None => frame.title_range = Some(span.clone()),
                        }
                    }
                }
                if let Some(frame) = native_task_stack.last_mut() {
                    if frame.capturing_title {
                        frame.title.push_str(&s);
                        match &mut frame.title_range {
                            Some(range) => range.end = span.end,
                            None => frame.title_range = Some(span.clone()),
                        }
                    }
                }
                if let Some(content) = metadata_capture.as_mut() {
                    content.push_str(&s);
                }
            }
            Event::Softbreak | Event::Hardbreak => {
                if let Some(frame) = task_stack.last_mut() {
                    if frame.capturing_title && !frame.title.is_empty() {
                        frame.title.push(' ');
                    }
                }
                if let Some(frame) = native_task_stack.last_mut() {
                    if frame.capturing_title && !frame.title.is_empty() {
                        frame.title.push(' ');
                    }
                }
            }
            Event::End(Container::Heading { .. }) => {
                if let Some(heading) = open_headings.pop() {
                    let range = heading.start..span.end;
                    let explicit_range = anchor_id_range(text, &range, &heading.id);
                    let explicit = explicit_range.is_some();
                    let rename_range = explicit_range
                        .or(heading.text_range)
                        .unwrap_or_else(|| range.clone());
                    insert_anchor(
                        &mut anchors,
                        &mut seen_anchor_ranges,
                        &mut diagnostics,
                        heading.id,
                        Anchor {
                            range,
                            rename_range,
                            explicit,
                        },
                    );
                }
            }
            Event::End(Container::Link(_, _)) => {
                if let Some((dst, start)) = open_links.pop() {
                    let source = start..span.end;
                    let target = parse_dst(&dst);
                    let target_path_range = reference_target_path_range(text, &source, &target);
                    let target_id_range = reference_target_id_range(text, &source, &target);
                    references.push(Reference {
                        source,
                        target_path_range,
                        target_id_range,
                        target,
                        kind: ReferenceKind::Link,
                    });
                }
            }
            Event::End(Container::Paragraph) => {
                if let Some(frame) = task_stack.last_mut() {
                    if frame.capturing_title {
                        frame.capturing_title = false;
                        frame.captured_title = true;
                    }
                }
                if let Some(frame) = native_task_stack.last_mut() {
                    if frame.capturing_title {
                        frame.capturing_title = false;
                        frame.captured_title = true;
                    }
                }
            }
            Event::End(Container::Div { class }) if class == TASK_CLASS => {
                if let Some(frame) = task_stack.pop() {
                    tasks.push(frame.into_task(span.end));
                }
            }
            Event::End(Container::ListItem | Container::TaskListItem { .. }) => {
                if let Event::End(Container::TaskListItem { .. }) = event {
                    if let Some(frame) = native_task_stack.pop() {
                        native_task_list_items.push(frame.into_item(span.end));
                    }
                }
                list_item_metadata.pop();
            }
            Event::End(Container::CodeBlock { .. }) => {
                if let Some(content) = metadata_capture.take() {
                    metadata = Some(content);
                }
            }
            _ => {}
        }
    }

    tasks.sort_by_key(|task| task.range.start);
    native_task_list_items.sort_by_key(|item| item.range.start);
    diagnostics.extend(document_local_task_diagnostics(&tasks));

    Analysis {
        index: DocIndex {
            anchors,
            references,
        },
        metadata,
        tasks,
        native_task_list_items,
        diagnostics,
    }
}

/// Whether a djot element's attributes include the given class.
pub fn has_class(attrs: &Attributes, class: &str) -> bool {
    attrs
        .get_value("class")
        .is_some_and(|v| v.to_string().split_whitespace().any(|c| c == class))
}

/// Return the raw text of the document's first `{.metadata}`-classed code block,
/// if any. This is the shared primitive behind metadata hover and export.
pub fn metadata_block(text: &str) -> Option<String> {
    analyze(text).metadata
}

pub fn tasks(text: &str) -> Vec<Task> {
    analyze(text).tasks
}

pub fn metadata_insertion_edit(
    text: &str,
    offset: usize,
    path: &Path,
    created: &str,
) -> Option<TextEdit> {
    if metadata_block(text).is_some() || !text.get(..offset)?.trim().is_empty() {
        return None;
    }

    Some(TextEdit {
        range: 0..0,
        new_text: format!(
            "{{.metadata}}\n``` toml\ntitle = \"{}\"\ncreated = \"{}\"\n```\n\n",
            escape_toml_string(&default_metadata_title(path)),
            created
        ),
    })
}

fn default_metadata_title(path: &Path) -> String {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.is_empty())
        .unwrap_or("Untitled")
        .to_string()
}

fn escape_toml_string(value: &str) -> String {
    let mut escaped = String::new();
    for c in value.chars() {
        match c {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            c if c.is_control() => {
                escaped.push_str(&format!("\\u{:04X}", c as u32));
            }
            c => escaped.push(c),
        }
    }
    escaped
}

fn insert_anchor(
    anchors: &mut HashMap<String, Anchor>,
    seen: &mut HashMap<String, Range<usize>>,
    diagnostics: &mut Vec<AnalysisDiagnostic>,
    id: String,
    anchor: Anchor,
) {
    record_anchor_occurrence(seen, diagnostics, id.clone(), anchor.rename_range.clone());
    anchors.entry(id).or_insert(anchor);
}

fn record_anchor_occurrence(
    seen: &mut HashMap<String, Range<usize>>,
    diagnostics: &mut Vec<AnalysisDiagnostic>,
    id: String,
    range: Range<usize>,
) {
    if let Some(first_range) = seen.get(&id) {
        diagnostics.push(AnalysisDiagnostic {
            range,
            kind: DiagnosticKind::DuplicateAnchor {
                id,
                first_range: first_range.clone(),
            },
        });
    } else {
        seen.insert(id, range);
    }
}

fn anchor_id_range(text: &str, range: &Range<usize>, id: &str) -> Option<Range<usize>> {
    let source = text.get(range.clone())?;
    let mut found = None;
    let mut offset = 0;

    while let Some(relative_start) = source[offset..].find('{') {
        let start = offset + relative_start;
        let Some(relative_end) = source[start..].find('}') else {
            break;
        };
        let end = start + relative_end + 1;
        if let Some(id_range) = attribute_id_range(source, start..end, id) {
            found = Some(range.start + id_range.start..range.start + id_range.end);
        }
        offset = end;
    }

    found
}

fn attribute_id_range(source: &str, range: Range<usize>, id: &str) -> Option<Range<usize>> {
    let bytes = source.as_bytes();
    let mut i = range.start + 1;
    let end = range.end.saturating_sub(1);
    let mut found = None;

    while i < end {
        while i < end && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= end {
            break;
        }

        match bytes[i] {
            b'%' => {
                i += 1;
                while i < end && bytes[i] != b'%' {
                    i += 1;
                }
                if i < end {
                    i += 1;
                }
            }
            b'.' => {
                i += 1;
                while i < end && is_attr_name_byte(bytes[i]) {
                    i += 1;
                }
            }
            b'#' => {
                let value_start = i + 1;
                i = value_start;
                while i < end && is_attr_name_byte(bytes[i]) {
                    i += 1;
                }
                if source.get(value_start..i) == Some(id) {
                    found = Some(value_start..i);
                }
            }
            byte if is_attr_name_byte(byte) => {
                let key_start = i;
                i += 1;
                while i < end && is_attr_name_byte(bytes[i]) {
                    i += 1;
                }
                let key = &source[key_start..i];
                if i >= end || bytes[i] != b'=' {
                    continue;
                }
                i += 1;

                let value_range = if i < end && bytes[i] == b'"' {
                    i += 1;
                    let value_start = i;
                    while i < end {
                        match bytes[i] {
                            b'\\' => {
                                i += 1;
                                if i < end {
                                    i += 1;
                                }
                            }
                            b'"' => break,
                            _ => i += 1,
                        }
                    }
                    let value_end = i;
                    if i < end {
                        i += 1;
                    }
                    value_start..value_end
                } else {
                    let value_start = i;
                    while i < end && is_attr_name_byte(bytes[i]) {
                        i += 1;
                    }
                    value_start..i
                };

                if key == "id" && source.get(value_range.clone()) == Some(id) {
                    found = Some(value_range);
                }
            }
            _ => break,
        }
    }

    found
}

fn is_attr_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'_' | b'-')
}

pub(crate) fn attribute_value_range(
    text: &str,
    range: &Range<usize>,
    key: &str,
    value: &str,
) -> Option<Range<usize>> {
    let source = text.get(range.clone())?;
    let mut search_start = 0;

    while search_start < source.len() {
        let key_start = source.get(search_start..)?.find(key)? + search_start;
        let before = source.get(..key_start)?.chars().next_back();
        if before.is_some_and(|c| !(c == '{' || c.is_whitespace())) {
            search_start = key_start + key.len();
            continue;
        }

        let mut cursor = key_start + key.len();
        cursor = skip_ascii_whitespace(source, cursor);
        if source.as_bytes().get(cursor) != Some(&b'=') {
            search_start = key_start + key.len();
            continue;
        }
        cursor += 1;
        cursor = skip_ascii_whitespace(source, cursor);

        let (value_start, value_end) = match source.as_bytes().get(cursor).copied() {
            Some(quote @ (b'"' | b'\'')) => {
                let value_start = cursor + 1;
                let mut pos = value_start;
                let mut escaped = false;
                loop {
                    let byte = *source.as_bytes().get(pos)?;
                    if escaped {
                        escaped = false;
                    } else if byte == b'\\' {
                        escaped = true;
                    } else if byte == quote {
                        break (value_start, pos);
                    }
                    pos += 1;
                }
            }
            Some(_) => {
                let value_start = cursor;
                let mut value_end = cursor;
                while let Some(byte) = source.as_bytes().get(value_end) {
                    if byte.is_ascii_whitespace() || *byte == b'}' {
                        break;
                    }
                    value_end += 1;
                }
                (value_start, value_end)
            }
            None => return None,
        };

        if source.get(value_start..value_end)? == value {
            return Some(range.start + value_start..range.start + value_end);
        }

        search_start = key_start + key.len();
    }

    None
}

fn skip_ascii_whitespace(source: &str, mut cursor: usize) -> usize {
    while source
        .as_bytes()
        .get(cursor)
        .is_some_and(u8::is_ascii_whitespace)
    {
        cursor += 1;
    }
    cursor
}

fn datetime_attribute(attrs: &Attributes, key: &str) -> Option<String> {
    attrs
        .get_value(key)
        .map(|value| value.to_string())
        .filter(|value| is_rfc3339_datetime(value))
}

fn string_attribute(attrs: &Attributes, key: &str) -> Option<String> {
    attrs.get_value(key).map(|value| value.to_string())
}

fn is_rfc3339_datetime(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() < 20 {
        return false;
    }

    if bytes.get(4) != Some(&b'-')
        || bytes.get(7) != Some(&b'-')
        || bytes.get(10) != Some(&b'T')
        || bytes.get(13) != Some(&b':')
        || bytes.get(16) != Some(&b':')
    {
        return false;
    }

    let Some(year) = parse_fixed_u32(value, 0, 4) else {
        return false;
    };
    let Some(month) = parse_fixed_u32(value, 5, 7) else {
        return false;
    };
    let Some(day) = parse_fixed_u32(value, 8, 10) else {
        return false;
    };
    let Some(hour) = parse_fixed_u32(value, 11, 13) else {
        return false;
    };
    let Some(minute) = parse_fixed_u32(value, 14, 16) else {
        return false;
    };
    let Some(second) = parse_fixed_u32(value, 17, 19) else {
        return false;
    };

    if year == 0
        || !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return false;
    }

    let mut offset_start = 19;
    if bytes.get(offset_start) == Some(&b'.') {
        offset_start += 1;
        let fraction_start = offset_start;
        while bytes
            .get(offset_start)
            .is_some_and(|byte| byte.is_ascii_digit())
        {
            offset_start += 1;
        }
        if offset_start == fraction_start {
            return false;
        }
    }

    match bytes.get(offset_start) {
        Some(b'Z') => offset_start + 1 == bytes.len(),
        Some(b'+') | Some(b'-') => {
            if offset_start + 6 != bytes.len() || bytes.get(offset_start + 3) != Some(&b':') {
                return false;
            }
            let Some(offset_hour) = parse_fixed_u32(value, offset_start + 1, offset_start + 3)
            else {
                return false;
            };
            let Some(offset_minute) = parse_fixed_u32(value, offset_start + 4, offset_start + 6)
            else {
                return false;
            };
            offset_hour <= 23 && offset_minute <= 59
        }
        _ => false,
    }
}

fn parse_fixed_u32(value: &str, start: usize, end: usize) -> Option<u32> {
    value.get(start..end)?.parse().ok()
}

/// A djot section being assembled while walking the event stream.
struct SectionFrame {
    range_start: usize,
    level: u16,
    name: String,
    selection: Range<usize>,
    capturing: bool,
    captured: bool,
    children: Vec<Heading>,
}

struct HeadingAnchorFrame {
    id: String,
    start: usize,
    text_range: Option<Range<usize>>,
}

struct TaskFrame {
    range_start: usize,
    depth: usize,
    id: Option<String>,
    created: Option<String>,
    done: Option<String>,
    canceled: Option<String>,
    due: Option<String>,
    wait: Option<String>,
    recur: Option<String>,
    prev: Option<String>,
    depends: Vec<TaskDependency>,
    capturing_title: bool,
    captured_title: bool,
    title_range: Option<Range<usize>>,
    title: String,
}

struct NativeTaskFrame {
    range_start: usize,
    checked: bool,
    capturing_title: bool,
    captured_title: bool,
    title_range: Option<Range<usize>>,
    title: String,
}

impl NativeTaskFrame {
    fn into_item(self, end: usize) -> NativeTaskListItem {
        NativeTaskListItem {
            range: self.range_start..end,
            title_range: self.title_range,
            title: self.title,
            checked: self.checked,
        }
    }
}

#[derive(Clone)]
struct TaskMetadata {
    id: Option<String>,
    created: Option<String>,
    done: Option<String>,
    canceled: Option<String>,
    due: Option<String>,
    wait: Option<String>,
    recur: Option<String>,
    prev: Option<String>,
    depends: Vec<TaskDependency>,
}

impl TaskMetadata {
    fn from_attributes(text: &str, span: &Range<usize>, attrs: &Attributes) -> Self {
        Self {
            id: string_attribute(attrs, "id"),
            created: datetime_attribute(attrs, "created"),
            done: datetime_attribute(attrs, "done"),
            canceled: datetime_attribute(attrs, "canceled"),
            due: datetime_attribute(attrs, "due"),
            wait: datetime_attribute(attrs, "wait"),
            recur: string_attribute(attrs, "recur"),
            prev: string_attribute(attrs, "prev"),
            depends: task_dependencies(text, span, attrs),
        }
    }

    fn from_attributes_with_fallback(
        text: &str,
        span: &Range<usize>,
        attrs: &Attributes,
        fallback: Option<&Self>,
    ) -> Self {
        let own = Self::from_attributes(text, span, attrs);
        Self {
            id: match attrs.get_value("id") {
                Some(_) => own.id,
                None => own
                    .id
                    .or_else(|| fallback.and_then(|metadata| metadata.id.clone())),
            },
            created: match attrs.get_value("created") {
                Some(_) => own.created,
                None => own
                    .created
                    .or_else(|| fallback.and_then(|metadata| metadata.created.clone())),
            },
            done: match attrs.get_value("done") {
                Some(_) => own.done,
                None => own
                    .done
                    .or_else(|| fallback.and_then(|metadata| metadata.done.clone())),
            },
            canceled: match attrs.get_value("canceled") {
                Some(_) => own.canceled,
                None => own
                    .canceled
                    .or_else(|| fallback.and_then(|metadata| metadata.canceled.clone())),
            },
            due: match attrs.get_value("due") {
                Some(_) => own.due,
                None => own
                    .due
                    .or_else(|| fallback.and_then(|metadata| metadata.due.clone())),
            },
            wait: match attrs.get_value("wait") {
                Some(_) => own.wait,
                None => own
                    .wait
                    .or_else(|| fallback.and_then(|metadata| metadata.wait.clone())),
            },
            recur: match attrs.get_value("recur") {
                Some(_) => own.recur,
                None => own
                    .recur
                    .or_else(|| fallback.and_then(|metadata| metadata.recur.clone())),
            },
            prev: match attrs.get_value("prev") {
                Some(_) => own.prev,
                None => own
                    .prev
                    .or_else(|| fallback.and_then(|metadata| metadata.prev.clone())),
            },
            depends: match attrs.get_value("depends") {
                Some(_) => own.depends,
                None => {
                    if own.depends.is_empty() {
                        fallback
                            .map(|metadata| metadata.depends.clone())
                            .unwrap_or_default()
                    } else {
                        own.depends
                    }
                }
            },
        }
    }
}

impl TaskFrame {
    fn into_task(self, range_end: usize) -> Task {
        Task {
            range: self.range_start..range_end,
            title_range: self.title_range,
            title: self.title.trim().to_string(),
            depth: self.depth,
            id: self.id,
            created: self.created,
            done: self.done,
            canceled: self.canceled,
            due: self.due,
            wait: self.wait,
            recur: self.recur,
            prev: self.prev,
            depends: self.depends,
        }
    }
}

impl SectionFrame {
    fn new(start: usize) -> Self {
        SectionFrame {
            range_start: start,
            level: 0,
            name: String::new(),
            selection: start..start,
            capturing: false,
            captured: false,
            children: Vec::new(),
        }
    }

    fn into_heading(self, section_end: usize) -> Heading {
        Heading {
            name: self.name,
            level: self.level,
            range: self.range_start..section_end,
            selection_range: self.selection,
            children: self.children,
        }
    }
}

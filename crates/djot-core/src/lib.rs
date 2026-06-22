//! Protocol-agnostic djot document analysis shared by the language server and
//! (in the future) the exporter.
//!
//! Everything here works in **byte offsets** into the source text. Consumers
//! that need editor coordinates (LSP UTF-16 positions) or a particular AST
//! (pandoc) convert at their own boundary — this crate never depends on those.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::ops::Range;
use std::path::{Component, Path, PathBuf};

use chrono::{DateTime, Datelike, Duration, FixedOffset, SecondsFormat, TimeZone, Timelike};
use iso8601_duration::Duration as IsoDuration;
use jotdown::{Attributes, Container, Event, Parser};
use serde::{Deserialize, Serialize};

mod diagnostics;
mod edits;

pub use diagnostics::{AnalysisDiagnostic, DiagnosticKind};
pub use edits::{
    apply_text_edits, DocumentTextEdit, EditError, FileRenameEdit, TextEdit, WorkspaceEdit,
};

/// The class that marks a leading code block as document metadata. This is a
/// djot-ls / djot-export convention layered on djot's native attribute syntax,
/// not part of djot itself — other djot tools simply see a classed code block.
pub const METADATA_CLASS: &str = "metadata";
pub const TASK_CLASS: &str = "task";

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

/// A semantic reference in the document and what it points at.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reference {
    /// Byte range of the reference source, for cursor hit-testing.
    pub source: Range<usize>,
    /// Byte span of the target path inside the link source, if this link names
    /// a file path in editable source syntax.
    pub target_path_range: Option<Range<usize>>,
    /// Byte span of the target anchor id inside the link source, if this link
    /// names an anchor in editable source syntax.
    pub target_id_range: Option<Range<usize>>,
    pub target: RefTarget,
    pub kind: ReferenceKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReferenceKind {
    Link,
    TaskPrev,
    TaskDependency,
}

/// The resolved destination of a link. jotdown hands us a single destination
/// string for every link form (inline, reference, implicit), so we only need to
/// classify that string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RefTarget {
    /// `#id` — an anchor in the same document.
    Internal { id: String },
    /// `path` or `path#id` — another file.
    External { path: String, id: Option<String> },
    /// `http(s):`, `mailto:`, … — not a block/heading reference.
    Url(String),
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
    /// Document-local diagnostics. Workspace-dependent diagnostics, such as
    /// unresolved cross-file references, are added by [`Workspace`].
    pub diagnostics: Vec<AnalysisDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task {
    pub range: Range<usize>,
    pub title_range: Option<Range<usize>>,
    pub title: String,
    pub depth: usize,
    pub id: Option<String>,
    pub created: Option<String>,
    pub done: Option<String>,
    pub canceled: Option<String>,
    pub due: Option<String>,
    pub wait: Option<String>,
    pub recur: Option<String>,
    pub prev: Option<String>,
    pub depends: Vec<TaskDependency>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskDependency {
    pub source: String,
    pub range: Range<usize>,
    pub target: RefTarget,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskStatusEdit {
    pub edits: Vec<TextEdit>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStatus {
    Done,
    Canceled,
}

impl TaskStatus {
    fn attribute(self) -> &'static str {
        match self {
            Self::Done => "done",
            Self::Canceled => "canceled",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskEditError {
    TaskIdNotFound { id: String },
    TaskAlreadyDone { id: String },
    TaskCanceled { id: String },
    CannotBuildEdit { id: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TaskRef {
    pub path: PathBuf,
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTaskDependency {
    pub source: String,
    pub target: TaskRef,
    pub task: Task,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepeatRule {
    Days(i64),
    Weeks(i64),
    Months(i32),
    Years(i32),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenameTarget {
    /// The document containing the anchor declaration.
    pub path: PathBuf,
    pub id: String,
    /// The source range under the cursor that should be selected before rename.
    pub range: Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenameTargetError {
    NotRenameable,
    ImplicitHeadingAnchor,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathRenameTarget {
    pub old_path: PathBuf,
    pub range: Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathRenameError {
    NotRenameable,
    NonDjotPath,
    TargetNotIndexed,
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
    let mut diagnostics = Vec::new();
    let mut metadata = None;
    let mut metadata_capture: Option<String> = None;
    let mut open_headings: Vec<HeadingAnchorFrame> = Vec::new();
    let mut open_links: Vec<(String, usize)> = Vec::new();
    let mut task_stack: Vec<TaskFrame> = Vec::new();
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
            }
            Event::End(Container::Div { class }) if class == TASK_CLASS => {
                if let Some(frame) = task_stack.pop() {
                    tasks.push(frame.into_task(span.end));
                }
            }
            Event::End(Container::ListItem | Container::TaskListItem { .. }) => {
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
    diagnostics.extend(document_local_task_diagnostics(&tasks));

    Analysis {
        index: DocIndex {
            anchors,
            references,
        },
        metadata,
        tasks,
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

pub fn task_list_item_conversion_edit(
    text: &str,
    offset: usize,
    created: &str,
) -> Option<TextEdit> {
    let (line_start, line_end) = line_bounds(text, offset)?;
    let line = text.get(line_start..line_end)?;
    let content = line.strip_suffix('\r').unwrap_or(line);
    let indent = leading_indent(content);
    let rest = &content[indent.len()..];
    let title = rest.strip_prefix("- [ ] ")?.trim();
    if title.is_empty() {
        return None;
    }

    Some(TextEdit {
        range: line_start..line_end,
        new_text: format!(
            "{indent}- {{created=\"{created}\"}}\n{indent}  ::: task\n{indent}  {title}\n{indent}  :::"
        ),
    })
}

pub fn task_status_edits_at(
    text: &str,
    offset: usize,
    status: TaskStatus,
    timestamp: &str,
) -> Option<TaskStatusEdit> {
    let analysis = analyze(text);
    let task = analysis.tasks.iter().find(|task| {
        task.done.is_none()
            && task.canceled.is_none()
            && task.range.start <= offset
            && offset <= task.range.end
    })?;
    task_status_edits_for_task(text, task, status, timestamp, true)
}

pub fn task_done_edits_by_id(
    text: &str,
    id: &str,
    done: &str,
) -> Result<Vec<TextEdit>, TaskEditError> {
    let analysis = analyze(text);
    let task = analysis
        .tasks
        .iter()
        .find(|task| task.id.as_deref() == Some(id))
        .ok_or_else(|| TaskEditError::TaskIdNotFound { id: id.to_string() })?;
    if task.done.is_some() {
        return Err(TaskEditError::TaskAlreadyDone { id: id.to_string() });
    }
    if task.canceled.is_some() {
        return Err(TaskEditError::TaskCanceled { id: id.to_string() });
    }

    task_status_edits_for_task(text, task, TaskStatus::Done, done, false)
        .map(|edit| edit.edits)
        .ok_or_else(|| TaskEditError::CannotBuildEdit { id: id.to_string() })
}

pub fn apply_task_text_edits(text: String, edits: Vec<TextEdit>) -> Result<String, EditError> {
    apply_text_edits(text, edits)
}

fn task_status_edits_for_task(
    text: &str,
    task: &Task,
    status: TaskStatus,
    timestamp: &str,
    allow_generated_current_id: bool,
) -> Option<TaskStatusEdit> {
    if task.recur.is_some() && task.due.is_some() {
        recurring_task_status_edits(text, task, status, timestamp, allow_generated_current_id)
    } else {
        simple_task_status_edits(text, task, status, timestamp)
    }
}

fn simple_task_status_edits(
    text: &str,
    task: &Task,
    status: TaskStatus,
    timestamp: &str,
) -> Option<TaskStatusEdit> {
    let attribute = status.attribute();
    let opening = task_opening_fence(text, &task.range)?;
    Some(TaskStatusEdit {
        edits: vec![TextEdit {
            range: opening.attribute_insert.clone(),
            new_text: format!(
                "{}{{{attribute}=\"{timestamp}\"}}\n{}",
                opening.attribute_prefix, opening.fence_prefix
            ),
        }],
    })
}

fn recurring_task_status_edits(
    text: &str,
    task: &Task,
    status: TaskStatus,
    timestamp: &str,
    allow_generated_current_id: bool,
) -> Option<TaskStatusEdit> {
    let attribute = status.attribute();
    let due = DateTime::parse_from_rfc3339(task.due.as_deref()?).ok()?;
    let recur = task.recur.as_deref()?;
    let next_due = next_recur_due(due, recur)?;
    let next_wait = task
        .wait
        .as_deref()
        .and_then(|wait| DateTime::parse_from_rfc3339(wait).ok())
        .and_then(|wait| next_recur_due(wait, recur));
    let opening = task_opening_fence(text, &task.range)?;
    let indent = opening.task_indent.as_str();

    let anchors = analyze(text).index.anchors;
    let mut reserved = HashSet::new();
    let current_id = match task.id.clone() {
        Some(id) => id,
        None if allow_generated_current_id => {
            let id = task_instance_id(&task.title, due, &anchors, &reserved)?;
            reserved.insert(id.clone());
            id
        }
        None => return None,
    };
    let next_id = task_instance_id(&task.title, next_due, &anchors, &reserved)?;
    let next_insert = line_bounds(text, task.range.end)?.1;
    let recur = escape_attribute_value(recur);
    let next_due_text = next_due.to_rfc3339_opts(SecondsFormat::Secs, true);
    let next_wait_text = next_wait.map(|wait| wait.to_rfc3339_opts(SecondsFormat::Secs, true));
    let next_wait_attribute = next_wait_text
        .as_deref()
        .map(|wait| format!(" wait=\"{}\"", escape_attribute_value(wait)))
        .unwrap_or_default();
    let current_id_text = escape_attribute_value(&current_id);
    let current_id_attribute = anchor_attribute(&current_id);
    let next_id_attribute = anchor_attribute(&next_id);
    let div = inherited_task_source(text.get(task.range.clone())?, indent);
    let list_item = single_task_list_item_context(text, opening.line_start, task.range.end, indent);

    let mut status_text = String::new();
    let mut attribute_prefix = opening.attribute_prefix.as_str();
    if task.id.is_none() {
        status_text.push_str(&format!("{attribute_prefix}{current_id_attribute}\n"));
        attribute_prefix = opening.continued_attribute_prefix.as_str();
    }
    status_text.push_str(&format!(
        "{attribute_prefix}{{{attribute}=\"{timestamp}\"}}\n{}",
        opening.fence_prefix
    ));

    let next_edit = match list_item {
        Some(context) => TextEdit {
            range: context.insert..context.insert,
            new_text: format!(
                "\n{list_indent}- {next_id_attribute}\n{indent}{{created=\"{timestamp}\" due=\"{next_due_text}\"{next_wait_attribute} recur=\"{recur}\" prev=\"#{current_id_text}\"}}\n{div}",
                list_indent = context.list_indent,
            ),
        },
        None => TextEdit {
            range: next_insert..next_insert,
            new_text: format!(
                "\n\n{indent}{next_id_attribute}\n{indent}{{created=\"{timestamp}\" due=\"{next_due_text}\"{next_wait_attribute} recur=\"{recur}\" prev=\"#{current_id_text}\"}}\n{div}"
            ),
        },
    };

    Some(TaskStatusEdit {
        edits: vec![
            TextEdit {
                range: opening.attribute_insert,
                new_text: status_text,
            },
            next_edit,
        ],
    })
}

/// Classify a link destination string into a [`RefTarget`].
pub fn parse_dst(dst: &str) -> RefTarget {
    if dst.contains("://") || dst.starts_with("mailto:") {
        RefTarget::Url(dst.to_string())
    } else if let Some(id) = dst.strip_prefix('#') {
        RefTarget::Internal { id: id.to_string() }
    } else if let Some((path, id)) = dst.split_once('#') {
        RefTarget::External {
            path: path.to_string(),
            id: Some(id.to_string()),
        }
    } else {
        RefTarget::External {
            path: dst.to_string(),
            id: None,
        }
    }
}

/// A link target resolved to a concrete file and optional anchor id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTarget {
    pub path: PathBuf,
    /// `None` means the file itself (no fragment).
    pub id: Option<String>,
}

/// Resolve a [`RefTarget`] (as seen in the document at `from`) to a concrete
/// file + anchor. Returns `None` for external URLs, which are not file targets.
pub fn resolve_target(from: &Path, target: &RefTarget) -> Option<ResolvedTarget> {
    match target {
        RefTarget::Url(_) => None,
        RefTarget::Internal { id } => Some(ResolvedTarget {
            path: from.to_path_buf(),
            id: Some(id.clone()),
        }),
        RefTarget::External { path, id } => {
            let base = from.parent().unwrap_or_else(|| Path::new(""));
            Some(ResolvedTarget {
                path: normalize(&base.join(path)),
                id: id.clone(),
            })
        }
    }
}

/// Lexically normalize a path (resolve `.`/`..` without touching the
/// filesystem), so equal logical paths compare equal as index keys.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// One indexed document: its text (for offset→position conversion at the LSP
/// boundary) and its parsed analysis.
#[derive(Debug)]
pub struct DocEntry {
    pub text: String,
    pub analysis: Analysis,
}

/// An in-memory index of multiple documents, keyed by normalized path. This is
/// the foundation for cross-file definition and (later) workspace-wide
/// find-references; it does no I/O itself — callers load file contents in.
#[derive(Debug, Default)]
pub struct Workspace {
    docs: HashMap<PathBuf, DocEntry>,
}

impl Workspace {
    pub fn new() -> Self {
        Self::default()
    }

    /// Parse `text` and store it under `path`, replacing any prior entry.
    pub fn insert(&mut self, path: PathBuf, text: String) {
        let analysis = analyze(&text);
        self.docs
            .insert(normalize(&path), DocEntry { text, analysis });
    }

    pub fn remove(&mut self, path: &Path) {
        self.docs.remove(&normalize(path));
    }

    pub fn contains(&self, path: &Path) -> bool {
        self.docs.contains_key(&normalize(path))
    }

    pub fn get(&self, path: &Path) -> Option<&DocEntry> {
        self.docs.get(&normalize(path))
    }

    /// All indexed documents.
    pub fn documents(&self) -> impl Iterator<Item = (&Path, &DocEntry)> {
        self.docs
            .iter()
            .map(|(path, entry)| (path.as_path(), entry))
    }

    /// The reference whose source span covers `offset` in the document at `path`.
    pub fn reference_at(&self, path: &Path, offset: usize) -> Option<&Reference> {
        self.get(path)?
            .analysis
            .index
            .references
            .iter()
            .find(|r| r.source.contains(&offset))
    }

    /// The anchor with `id` in the document at `path`.
    pub fn anchor(&self, path: &Path, id: &str) -> Option<&Anchor> {
        self.get(path)?.analysis.index.anchors.get(id)
    }

    /// The anchor whose source span covers `offset` in the document at `path`.
    pub fn anchor_at(&self, path: &Path, offset: usize) -> Option<(&str, &Anchor)> {
        self.get(path)?
            .analysis
            .index
            .anchors
            .iter()
            .find(|(_, anchor)| anchor.range.contains(&offset))
            .map(|(id, anchor)| (id.as_str(), anchor))
    }

    /// Every loaded reference that points at `(path, id)` — the basis for
    /// find-references. Scans all loaded documents (so completeness requires the
    /// caller to have loaded the whole workspace first).
    pub fn references_to(&self, path: &Path, id: &str) -> Vec<(PathBuf, Range<usize>)> {
        let target = normalize(path);
        let mut out = Vec::new();
        for (src, entry) in &self.docs {
            for reference in &entry.analysis.index.references {
                if let Some(resolved) = resolve_target(src, &reference.target) {
                    if resolved.path == target && resolved.id.as_deref() == Some(id) {
                        out.push((src.clone(), reference.source.clone()));
                    }
                }
            }
        }
        out
    }

    pub fn task_by_id(&self, path: &Path, id: &str) -> Option<Task> {
        let entry = self.get(path)?;
        entry
            .analysis
            .tasks
            .iter()
            .find(|task| task.id.as_deref() == Some(id))
            .cloned()
    }

    pub fn task_dependencies(&self, path: &Path, task: &Task) -> Vec<ResolvedTaskDependency> {
        let source_path = normalize(path);
        task.depends
            .iter()
            .filter_map(|dependency| {
                let target = self.resolve_task_dependency(&source_path, dependency)?;
                let task = self.task_by_id(&target.path, &target.id)?;
                Some(ResolvedTaskDependency {
                    source: dependency.source.clone(),
                    target,
                    task,
                })
            })
            .collect()
    }

    pub fn open_task_dependencies(&self, path: &Path, task: &Task) -> Vec<ResolvedTaskDependency> {
        self.task_dependencies(path, task)
            .into_iter()
            .filter(|dependency| {
                dependency.task.done.is_none() && dependency.task.canceled.is_none()
            })
            .collect()
    }

    pub fn is_task_blocked(&self, path: &Path, task: &Task) -> bool {
        !self.open_task_dependencies(path, task).is_empty()
    }

    pub fn directly_blocking_tasks(&self, path: &Path, id: &str) -> Vec<TaskRef> {
        let target = TaskRef {
            path: normalize(path),
            id: id.to_string(),
        };
        let mut blocking = Vec::new();
        for (source_path, entry) in &self.docs {
            for task in &entry.analysis.tasks {
                let Some(source_id) = &task.id else {
                    continue;
                };
                if task.depends.iter().any(|dependency| {
                    self.resolve_task_dependency(source_path, dependency)
                        .is_some_and(|dependency_target| dependency_target == target)
                }) {
                    blocking.push(TaskRef {
                        path: source_path.clone(),
                        id: source_id.clone(),
                    });
                }
            }
        }
        blocking.sort_by(|a, b| (&a.path, &a.id).cmp(&(&b.path, &b.id)));
        blocking
    }

    fn resolve_task_dependency(&self, from: &Path, dependency: &TaskDependency) -> Option<TaskRef> {
        let target = resolve_target(from, &dependency.target)?;
        Some(TaskRef {
            path: target.path,
            id: target.id?,
        })
    }

    /// Resolve the anchor symbol under `offset`, either from the anchor
    /// declaration itself or from an editable link target that points to it.
    pub fn rename_target_at(
        &self,
        path: &Path,
        offset: usize,
    ) -> Result<RenameTarget, RenameTargetError> {
        let path = normalize(path);
        if let Some((id, anchor)) = self.anchor_rename_at(&path, offset) {
            if !anchor.explicit {
                return Err(RenameTargetError::ImplicitHeadingAnchor);
            }
            return Ok(RenameTarget {
                path,
                id: id.to_string(),
                range: anchor.rename_range.clone(),
            });
        }

        let reference = self
            .reference_at(&path, offset)
            .ok_or(RenameTargetError::NotRenameable)?;
        let target_id_range = reference
            .target_id_range
            .clone()
            .ok_or(RenameTargetError::NotRenameable)?;
        if !contains_inclusive(&target_id_range, offset) {
            return Err(RenameTargetError::NotRenameable);
        }
        let target =
            resolve_target(&path, &reference.target).ok_or(RenameTargetError::NotRenameable)?;
        let id = target.id.ok_or(RenameTargetError::NotRenameable)?;
        let anchor = self
            .anchor(&target.path, &id)
            .ok_or(RenameTargetError::NotRenameable)?;
        if !anchor.explicit {
            return Err(RenameTargetError::ImplicitHeadingAnchor);
        }

        Ok(RenameTarget {
            path: target.path,
            id,
            range: target_id_range,
        })
    }

    fn anchor_rename_at(&self, path: &Path, offset: usize) -> Option<(&str, &Anchor)> {
        self.get(path)?
            .analysis
            .index
            .anchors
            .iter()
            .find(|(_, anchor)| contains_inclusive(&anchor.rename_range, offset))
            .map(|(id, anchor)| (id.as_str(), anchor))
    }

    /// Text edits for renaming an explicit anchor and all indexed references to
    /// it. Scans all loaded documents, so completeness requires the caller to
    /// have indexed the workspace first.
    pub fn anchor_rename_edits(
        &self,
        path: &Path,
        id: &str,
        replacement: &str,
    ) -> Vec<DocumentTextEdit> {
        let target = normalize(path);
        let mut edits = Vec::new();

        if let Some(anchor) = self.anchor(&target, id) {
            if !anchor.explicit {
                return Vec::new();
            }
            edits.push(DocumentTextEdit {
                path: target.clone(),
                edit: TextEdit {
                    range: anchor.rename_range.clone(),
                    new_text: replacement.to_string(),
                },
            });
        } else {
            return Vec::new();
        }

        for (src, entry) in &self.docs {
            for reference in &entry.analysis.index.references {
                let Some(range) = &reference.target_id_range else {
                    continue;
                };
                let Some(resolved) = resolve_target(src, &reference.target) else {
                    continue;
                };
                if resolved.path == target && resolved.id.as_deref() == Some(id) {
                    edits.push(DocumentTextEdit {
                        path: src.clone(),
                        edit: TextEdit {
                            range: range.clone(),
                            new_text: replacement.to_string(),
                        },
                    });
                }
            }
        }

        edits
    }

    /// Resolve a file path link under `offset` to the indexed document it
    /// targets. Only Djot file targets can be renamed this way.
    pub fn path_rename_target_at(
        &self,
        path: &Path,
        offset: usize,
    ) -> Result<PathRenameTarget, PathRenameError> {
        let path = normalize(path);
        let reference = self
            .reference_at(&path, offset)
            .ok_or(PathRenameError::NotRenameable)?;
        let range = reference
            .target_path_range
            .clone()
            .ok_or(PathRenameError::NotRenameable)?;
        if !contains_inclusive(&range, offset) {
            return Err(PathRenameError::NotRenameable);
        }

        let target =
            resolve_target(&path, &reference.target).ok_or(PathRenameError::NotRenameable)?;
        if !is_djot_file_path(&target.path) {
            return Err(PathRenameError::NonDjotPath);
        }
        if !self.contains(&target.path) {
            return Err(PathRenameError::TargetNotIndexed);
        }

        Ok(PathRenameTarget {
            old_path: target.path,
            range,
        })
    }

    /// Text edits for updating all indexed links when moving a document from
    /// `old_path` to `new_path`.
    pub fn path_rename_text_edits(
        &self,
        old_path: &Path,
        new_path: &Path,
    ) -> Vec<DocumentTextEdit> {
        let old_path = normalize(old_path);
        let new_path = normalize(new_path);
        let mut edits = Vec::new();

        for (src, entry) in &self.docs {
            for reference in &entry.analysis.index.references {
                let Some(range) = &reference.target_path_range else {
                    continue;
                };
                let Some(resolved) = resolve_target(src, &reference.target) else {
                    continue;
                };
                if resolved.path == old_path {
                    edits.push(DocumentTextEdit {
                        path: src.clone(),
                        edit: TextEdit {
                            range: range.clone(),
                            new_text: relative_link_path(src, &new_path),
                        },
                    });
                }
            }
        }

        edits
    }

    /// A protocol-agnostic workspace edit plan for moving a document and
    /// updating indexed links to point at the new path. This performs no file
    /// system checks; callers that own I/O must validate conflicts separately.
    pub fn path_rename_edit_plan(&self, old_path: &Path, new_path: &Path) -> Vec<WorkspaceEdit> {
        let old_path = normalize(old_path);
        let new_path = normalize(new_path);
        std::iter::once(WorkspaceEdit::RenameFile(FileRenameEdit {
            old_path: old_path.clone(),
            new_path: new_path.clone(),
        }))
        .chain(
            self.path_rename_text_edits(&old_path, &new_path)
                .into_iter()
                .map(WorkspaceEdit::Text),
        )
        .collect()
    }

    /// Diagnostics for unresolved file and anchor references in one loaded
    /// document. URLs are intentionally ignored.
    pub fn diagnostics_for(&self, path: &Path) -> Vec<AnalysisDiagnostic> {
        let Some(entry) = self.get(path) else {
            return Vec::new();
        };

        let mut diagnostics = entry.analysis.diagnostics.clone();

        for reference in &entry.analysis.index.references {
            if reference.kind == ReferenceKind::TaskDependency {
                continue;
            }
            if !is_diagnostic_target(&reference.target) {
                continue;
            }

            let Some(target) = resolve_target(path, &reference.target) else {
                continue;
            };

            let Some(target_entry) = self.get(&target.path) else {
                if let RefTarget::External { path, .. } = &reference.target {
                    diagnostics.push(AnalysisDiagnostic {
                        range: reference.source.clone(),
                        kind: DiagnosticKind::UnresolvedPath { path: path.clone() },
                    });
                }
                continue;
            };

            if let Some(id) = target.id {
                let Some(anchor) = target_entry.analysis.index.anchors.get(&id) else {
                    diagnostics.push(AnalysisDiagnostic {
                        range: reference.source.clone(),
                        kind: DiagnosticKind::UnresolvedAnchor { id },
                    });
                    continue;
                };

                if reference.kind == ReferenceKind::TaskPrev
                    && !anchor_targets_task(&target_entry.analysis.tasks, &anchor.range)
                {
                    diagnostics.push(AnalysisDiagnostic {
                        range: reference.source.clone(),
                        kind: DiagnosticKind::InvalidTaskPrevTarget { id },
                    });
                }
            }
        }

        diagnostics.extend(self.task_dependency_diagnostics(path, entry));

        diagnostics
    }

    fn task_dependency_diagnostics(
        &self,
        path: &Path,
        entry: &DocEntry,
    ) -> Vec<AnalysisDiagnostic> {
        let path = normalize(path);
        let graph = self.task_dependency_graph();
        let mut diagnostics = Vec::new();

        for task in &entry.analysis.tasks {
            let task_ref = task.id.as_ref().map(|id| TaskRef {
                path: path.clone(),
                id: id.clone(),
            });

            for dependency in &task.depends {
                if matches!(dependency.target, RefTarget::Url(_)) {
                    diagnostics.push(AnalysisDiagnostic {
                        range: dependency.range.clone(),
                        kind: DiagnosticKind::InvalidTaskDependencyTarget {
                            target: dependency.source.clone(),
                        },
                    });
                    continue;
                }

                if let Some(diagnostic) = self.invalid_task_dependency_diagnostic(&path, dependency)
                {
                    diagnostics.push(diagnostic);
                    continue;
                }

                if let Some(target) = self.resolve_task_dependency(&path, dependency) {
                    if task_ref.as_ref() == Some(&target) {
                        diagnostics.push(AnalysisDiagnostic {
                            range: dependency.range.clone(),
                            kind: DiagnosticKind::TaskSelfDependency {
                                target: dependency.source.clone(),
                            },
                        });
                    }
                }
            }

            if let Some(task_ref) = task_ref {
                if has_dependency_cycle(&graph, &task_ref) {
                    diagnostics.push(AnalysisDiagnostic {
                        range: task.range.clone(),
                        kind: DiagnosticKind::TaskDependencyCycle { id: task_ref.id },
                    });
                }
            }

            if task.done.is_none() && task.canceled.is_none() {
                let blockers = self.open_task_dependencies(&path, &task);
                if !blockers.is_empty() {
                    diagnostics.push(AnalysisDiagnostic {
                        range: task
                            .title_range
                            .clone()
                            .unwrap_or_else(|| task.range.clone()),
                        kind: DiagnosticKind::TaskBlocked {
                            count: blockers.len(),
                        },
                    });
                }
            }
        }

        diagnostics
    }

    fn invalid_task_dependency_diagnostic(
        &self,
        path: &Path,
        dependency: &TaskDependency,
    ) -> Option<AnalysisDiagnostic> {
        if !is_diagnostic_target(&dependency.target) {
            return None;
        }

        let target = resolve_target(path, &dependency.target)?;
        let Some(target_entry) = self.get(&target.path) else {
            if let RefTarget::External { path, .. } = &dependency.target {
                return Some(AnalysisDiagnostic {
                    range: dependency.range.clone(),
                    kind: DiagnosticKind::UnresolvedPath { path: path.clone() },
                });
            }
            return None;
        };

        let Some(id) = target.id else {
            return None;
        };
        let Some(anchor) = target_entry.analysis.index.anchors.get(&id) else {
            return Some(AnalysisDiagnostic {
                range: dependency.range.clone(),
                kind: DiagnosticKind::UnresolvedAnchor { id },
            });
        };

        if !anchor_targets_task(&target_entry.analysis.tasks, &anchor.range) {
            return Some(AnalysisDiagnostic {
                range: dependency.range.clone(),
                kind: DiagnosticKind::InvalidTaskDependencyTarget {
                    target: dependency.source.clone(),
                },
            });
        }

        None
    }

    fn task_dependency_graph(&self) -> HashMap<TaskRef, Vec<TaskRef>> {
        let mut graph: HashMap<TaskRef, Vec<TaskRef>> = HashMap::new();
        for (path, entry) in &self.docs {
            for task in &entry.analysis.tasks {
                let Some(id) = &task.id else {
                    continue;
                };
                let source = TaskRef {
                    path: path.clone(),
                    id: id.clone(),
                };
                let edges = task
                    .depends
                    .iter()
                    .filter_map(|dependency| {
                        let target = self.resolve_task_dependency(path, dependency)?;
                        self.task_by_id(&target.path, &target.id).map(|_| target)
                    })
                    .collect::<Vec<_>>();
                graph.insert(source, edges);
            }
        }
        graph
    }
}

fn has_dependency_cycle(graph: &HashMap<TaskRef, Vec<TaskRef>>, start: &TaskRef) -> bool {
    fn visit(
        graph: &HashMap<TaskRef, Vec<TaskRef>>,
        start: &TaskRef,
        current: &TaskRef,
        seen: &mut HashSet<TaskRef>,
    ) -> bool {
        let Some(edges) = graph.get(current) else {
            return false;
        };
        for next in edges {
            if next == start {
                return true;
            }
            if seen.insert(next.clone()) && visit(graph, start, next, seen) {
                return true;
            }
        }
        false
    }

    let mut seen = HashSet::new();
    visit(graph, start, start, &mut seen)
}

struct ListTaskContext<'a> {
    list_indent: &'a str,
    insert: usize,
}

fn single_task_list_item_context<'a>(
    text: &str,
    task_line_start: usize,
    task_range_end: usize,
    task_indent: &'a str,
) -> Option<ListTaskContext<'a>> {
    let list_indent = task_indent
        .strip_suffix("  ")
        .or_else(|| task_indent.strip_suffix('\t'))?;
    let list_start = containing_list_item_start(text, task_line_start, list_indent, task_indent)?;
    let list_end = list_item_end(text, list_start, list_indent)?;
    let task_end_line_offset = task_range_end.saturating_sub(1);
    if list_end != line_bounds(text, task_end_line_offset).map(|(_, end)| end)? {
        return None;
    }
    if has_indented_content_after(text, list_end, list_indent) {
        return None;
    }
    if count_task_fences(text.get(list_start..list_end)?) != 1 {
        return None;
    }

    Some(ListTaskContext {
        list_indent,
        insert: list_end,
    })
}

fn containing_list_item_start(
    text: &str,
    task_line_start: usize,
    list_indent: &str,
    task_indent: &str,
) -> Option<usize> {
    let (_, current_line_end) = line_bounds(text, task_line_start)?;
    let current_line = text
        .get(task_line_start..current_line_end)?
        .strip_suffix('\r')
        .unwrap_or(text.get(task_line_start..current_line_end)?);
    let current_indent = leading_indent(current_line);
    let current_trimmed = current_line.trim_start();
    if current_indent == list_indent && current_trimmed.starts_with("- ") {
        return Some(task_line_start);
    }

    let mut line_start = task_line_start;
    while let Some(start) = previous_line_start(text, line_start) {
        let (_, line_end) = line_bounds(text, start)?;
        let line = text
            .get(start..line_end)?
            .strip_suffix('\r')
            .unwrap_or(text.get(start..line_end)?);
        if line.trim().is_empty() {
            line_start = start;
            continue;
        }
        let indent = leading_indent(line);
        let trimmed = line.trim_start();
        if indent == list_indent && trimmed.starts_with("- ") {
            return Some(start);
        }
        if indent.len() < task_indent.len() {
            return None;
        }
        line_start = start;
    }
    None
}

fn list_item_end(text: &str, list_start: usize, list_indent: &str) -> Option<usize> {
    let (_, first_end) = line_bounds(text, list_start)?;
    let mut line_start = next_line_start(text, first_end)?;
    let mut last_end = first_end;

    while line_start < text.len() {
        let (_, line_end) = line_bounds(text, line_start)?;
        let line = text
            .get(line_start..line_end)?
            .strip_suffix('\r')
            .unwrap_or(text.get(line_start..line_end)?);
        if !line.trim().is_empty() {
            let indent = leading_indent(line);
            let trimmed = line.trim_start();
            if indent.len() <= list_indent.len()
                && (trimmed.starts_with("- ") || trimmed.starts_with("+ "))
            {
                break;
            }
        }
        last_end = line_end;
        let Some(next) = next_line_start(text, line_end) else {
            break;
        };
        line_start = next;
    }

    Some(last_end)
}

fn has_indented_content_after(text: &str, line_end: usize, list_indent: &str) -> bool {
    let Some(mut line_start) = next_line_start(text, line_end) else {
        return false;
    };
    while line_start < text.len() {
        let Some((_, line_end)) = line_bounds(text, line_start) else {
            return false;
        };
        let Some(line) = text.get(line_start..line_end) else {
            return false;
        };
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.trim().is_empty() {
            line_start = match next_line_start(text, line_end) {
                Some(start) => start,
                None => return false,
            };
            continue;
        }
        let indent = leading_indent(line);
        if indent.len() <= list_indent.len() {
            return false;
        }
        return true;
    }

    false
}

fn count_task_fences(text: &str) -> usize {
    text.lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            trimmed
                .strip_prefix("- ")
                .unwrap_or(trimmed)
                .starts_with("::: task")
        })
        .count()
}

fn previous_line_start(text: &str, line_start: usize) -> Option<usize> {
    if line_start == 0 {
        return None;
    }
    let previous_end = line_start.checked_sub('\n'.len_utf8())?;
    Some(text[..previous_end].rfind('\n').map_or(0, |i| i + 1))
}

fn next_line_start(text: &str, line_end: usize) -> Option<usize> {
    if line_end >= text.len() {
        None
    } else {
        Some(line_end + '\n'.len_utf8())
    }
}

fn ensure_block_indent(block: &str, indent: &str) -> String {
    if indent.is_empty() {
        return block.to_string();
    }

    let mut out = String::new();
    for line in block.split_inclusive('\n') {
        let content = line.trim_end_matches(['\r', '\n']);
        if content.is_empty() || line.starts_with(indent) {
            out.push_str(line);
        } else {
            out.push_str(indent);
            out.push_str(line);
        }
    }
    out
}

fn inherited_task_source(source: &str, indent: &str) -> String {
    filter_recurring_instance_attributes(&ensure_block_indent(source, indent))
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

fn filter_recurring_instance_attributes(source: &str) -> String {
    let mut out = String::new();
    for line in source.split_inclusive('\n') {
        match filter_recurring_attribute_line(line) {
            AttributeLineFilter::Keep(line) => out.push_str(line),
            AttributeLineFilter::Replace(line) => out.push_str(&line),
            AttributeLineFilter::Drop => {}
        }
    }
    out
}

enum AttributeLineFilter<'a> {
    Keep(&'a str),
    Replace(String),
    Drop,
}

fn filter_recurring_attribute_line(line: &str) -> AttributeLineFilter<'_> {
    let line_without_newline = line.trim_end_matches(['\r', '\n']);
    let newline = &line[line_without_newline.len()..];
    let indent = leading_indent(line_without_newline);
    let content = &line_without_newline[indent.len()..];
    let Some(inner) = content.strip_prefix('{').and_then(|s| s.strip_suffix('}')) else {
        return AttributeLineFilter::Keep(line);
    };

    let Some(tokens) = attribute_tokens(inner) else {
        return AttributeLineFilter::Keep(line);
    };
    if tokens.is_empty() {
        return AttributeLineFilter::Keep(line);
    }

    let kept = tokens
        .iter()
        .filter(|token| !is_recurring_instance_attribute(token))
        .collect::<Vec<_>>();
    if kept.len() == tokens.len() {
        return AttributeLineFilter::Keep(line);
    }
    if kept.is_empty() {
        return AttributeLineFilter::Drop;
    }

    let mut replacement = String::new();
    replacement.push_str(indent);
    replacement.push('{');
    for (idx, token) in kept.iter().enumerate() {
        if idx > 0 {
            replacement.push(' ');
        }
        replacement.push_str(token);
    }
    replacement.push('}');
    replacement.push_str(newline);
    AttributeLineFilter::Replace(replacement)
}

fn attribute_tokens(inner: &str) -> Option<Vec<&str>> {
    let mut tokens = Vec::new();
    let mut start = None;
    let mut quote = None;
    let mut escaped = false;

    for (idx, ch) in inner.char_indices() {
        if start.is_none() {
            if ch.is_whitespace() {
                continue;
            }
            start = Some(idx);
        }

        if let Some(quoted) = quote {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == quoted {
                quote = None;
            }
            continue;
        }

        if ch == '"' || ch == '\'' {
            quote = Some(ch);
        } else if ch.is_whitespace() {
            if let Some(token_start) = start.take() {
                tokens.push(inner[token_start..idx].trim());
            }
        }
    }

    if quote.is_some() {
        return None;
    }
    if let Some(token_start) = start {
        tokens.push(inner[token_start..].trim());
    }

    Some(
        tokens
            .into_iter()
            .filter(|token| !token.is_empty())
            .collect(),
    )
}

fn is_recurring_instance_attribute(token: &str) -> bool {
    if token.starts_with('#') {
        return true;
    }
    let key = token.split_once('=').map_or(token, |(key, _)| key);
    matches!(
        key,
        "id" | "created" | "done" | "canceled" | "due" | "wait" | "recur" | "prev"
    )
}

pub fn next_recur_due(due: DateTime<FixedOffset>, recur: &str) -> Option<DateTime<FixedOffset>> {
    let rule = parse_repeat_rule(recur)?;
    match rule {
        RepeatRule::Days(days) => Some(due + Duration::days(days)),
        RepeatRule::Weeks(weeks) => Some(due + Duration::weeks(weeks)),
        RepeatRule::Months(months) => add_months(due, months),
        RepeatRule::Years(years) => add_months(due, years.checked_mul(12)?),
    }
}

fn add_months(due: DateTime<FixedOffset>, months: i32) -> Option<DateTime<FixedOffset>> {
    let month0 = due.month0() as i32 + months;
    let year = due.year() + month0.div_euclid(12);
    let month0 = month0.rem_euclid(12);
    let month = (month0 + 1) as u32;
    let day = due.day().min(last_day_of_month(year, month)?);
    due.timezone()
        .with_ymd_and_hms(year, month, day, due.hour(), due.minute(), due.second())
        .single()
}

fn last_day_of_month(year: i32, month: u32) -> Option<u32> {
    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    let first_next = chrono::NaiveDate::from_ymd_opt(next_year, next_month, 1)?;
    Some((first_next - Duration::days(1)).day())
}

fn task_instance_id(
    title: &str,
    due: DateTime<FixedOffset>,
    anchors: &HashMap<String, Anchor>,
    reserved: &HashSet<String>,
) -> Option<String> {
    let base = djot_heading_id(title)?;
    let date = due.format("%Y-%m-%d");
    let candidate = format!("{base}-{date}");
    Some(unique_anchor_id(candidate, anchors, reserved))
}

fn djot_heading_id(title: &str) -> Option<String> {
    let source = format!("# {}\n", title.trim());
    Parser::new(&source).find_map(|event| match event {
        Event::Start(Container::Heading { id, .. }, _) => Some(id.into_owned()),
        _ => None,
    })
}

fn unique_anchor_id(
    candidate: String,
    anchors: &HashMap<String, Anchor>,
    reserved: &HashSet<String>,
) -> String {
    if !anchors.contains_key(&candidate) && !reserved.contains(&candidate) {
        return candidate;
    }
    let mut count = 2;
    loop {
        let id = format!("{candidate}-{count}");
        if !anchors.contains_key(&id) && !reserved.contains(&id) {
            return id;
        }
        count += 1;
    }
}

fn leading_indent(line: &str) -> &str {
    let indent_len = line
        .char_indices()
        .find(|(_, c)| *c != ' ' && *c != '\t')
        .map(|(i, _)| i)
        .unwrap_or(line.len());
    &line[..indent_len]
}

fn escape_attribute_value(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn anchor_attribute(id: &str) -> String {
    if is_shorthand_anchor_id(id) {
        format!("{{#{id}}}")
    } else {
        format!("{{id=\"{}\"}}", escape_attribute_value(id))
    }
}

fn is_shorthand_anchor_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'_' | b'-'))
}

struct TaskOpeningFence {
    line_start: usize,
    attribute_insert: Range<usize>,
    attribute_prefix: String,
    continued_attribute_prefix: String,
    fence_prefix: String,
    task_indent: String,
}

fn task_opening_fence(text: &str, range: &Range<usize>) -> Option<TaskOpeningFence> {
    let mut offset = range.start;
    while offset <= range.end {
        let (line_start, line_end) = line_bounds(text, offset)?;
        let line = text.get(line_start..line_end)?;
        if let Some(opening) = task_opening_fence_from_line(line_start, line) {
            return Some(opening);
        }
        if line_end >= range.end || line_end == text.len() {
            break;
        }
        offset = line_end + '\n'.len_utf8();
    }
    None
}

fn task_opening_fence_from_line(line_start: usize, line: &str) -> Option<TaskOpeningFence> {
    let line = line.strip_suffix('\r').unwrap_or(line);
    let indent = leading_indent(line);
    let rest = &line[indent.len()..];
    if rest.starts_with("::: task") {
        return Some(TaskOpeningFence {
            line_start,
            attribute_insert: line_start..line_start,
            attribute_prefix: indent.to_string(),
            continued_attribute_prefix: indent.to_string(),
            fence_prefix: String::new(),
            task_indent: indent.to_string(),
        });
    }

    let fence = rest.strip_prefix("- ")?;
    if !fence.starts_with("::: task") {
        return None;
    }
    Some(TaskOpeningFence {
        line_start,
        attribute_insert: line_start..line_start + indent.len() + "- ".len(),
        attribute_prefix: format!("{indent}- "),
        continued_attribute_prefix: format!("{indent}  "),
        fence_prefix: format!("{indent}  "),
        task_indent: format!("{indent}  "),
    })
}

fn line_bounds(text: &str, offset: usize) -> Option<(usize, usize)> {
    if offset > text.len() {
        return None;
    }
    let start = text[..offset].rfind('\n').map_or(0, |i| i + 1);
    let end = text[offset..].find('\n').map_or(text.len(), |i| offset + i);
    Some((start, end))
}

fn anchor_targets_task(tasks: &[Task], anchor_range: &Range<usize>) -> bool {
    tasks
        .iter()
        .any(|task| ranges_overlap(anchor_range, &task.range))
}

fn ranges_overlap(a: &Range<usize>, b: &Range<usize>) -> bool {
    a.start < b.end && b.start < a.end
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

fn document_local_task_diagnostics(tasks: &[Task]) -> Vec<AnalysisDiagnostic> {
    let mut diagnostics = Vec::new();

    for task in tasks {
        if task.done.is_some() && task.canceled.is_some() {
            diagnostics.push(AnalysisDiagnostic {
                range: task.range.clone(),
                kind: DiagnosticKind::ConflictingTaskClosedState,
            });
        }

        let Some(recur) = task.recur.as_deref() else {
            continue;
        };

        if parse_repeat_rule(recur).is_none() {
            diagnostics.push(AnalysisDiagnostic {
                range: task.range.clone(),
                kind: DiagnosticKind::InvalidTaskRecur {
                    recur: recur.to_string(),
                },
            });
        }

        if task.due.is_none() {
            diagnostics.push(AnalysisDiagnostic {
                range: task.range.clone(),
                kind: DiagnosticKind::MissingTaskDueForRecur,
            });
        }
    }

    diagnostics
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

pub fn parse_repeat_rule(recur: &str) -> Option<RepeatRule> {
    let duration: IsoDuration = recur.parse().ok()?;
    let units = [
        duration.year,
        duration.month,
        duration.day,
        duration.hour,
        duration.minute,
        duration.second,
    ];
    if units.iter().filter(|value| **value > 0.0).count() != 1 {
        return None;
    }
    if duration.hour > 0.0 || duration.minute > 0.0 || duration.second > 0.0 {
        return None;
    }
    if duration.year > 0.0 {
        return integer_f32(duration.year).and_then(|years| {
            i32::try_from(years)
                .ok()
                .filter(|years| *years > 0)
                .map(RepeatRule::Years)
        });
    }
    if duration.month > 0.0 {
        return integer_f32(duration.month).and_then(|months| {
            i32::try_from(months)
                .ok()
                .filter(|months| *months > 0)
                .map(RepeatRule::Months)
        });
    }
    integer_f32(duration.day).and_then(|days| {
        if days > 0 && days % 7 == 0 {
            Some(RepeatRule::Weeks(days / 7))
        } else if days > 0 {
            Some(RepeatRule::Days(days))
        } else {
            None
        }
    })
}

fn integer_f32(value: f32) -> Option<i64> {
    if value.fract() == 0.0 && value <= i64::MAX as f32 {
        Some(value as i64)
    } else {
        None
    }
}

fn is_diagnostic_target(target: &RefTarget) -> bool {
    match target {
        RefTarget::Internal { .. } => true,
        RefTarget::External { path, .. } => is_djot_path(path),
        RefTarget::Url(_) => false,
    }
}

fn is_djot_path(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext == "dj" || ext == "djot")
}

fn is_djot_file_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext == "dj" || ext == "djot")
}

fn contains_inclusive(range: &Range<usize>, offset: usize) -> bool {
    range.start <= offset && offset <= range.end
}

fn relative_link_path(from_file: &Path, target: &Path) -> String {
    relative_path(from_file.parent().unwrap_or_else(|| Path::new("")), target)
        .display()
        .to_string()
}

fn relative_path(base: &Path, target: &Path) -> PathBuf {
    let base = normalize(base);
    let target = normalize(target);
    let base_components = path_components(&base);
    let target_components = path_components(&target);

    if base_components.first() != target_components.first() {
        return target;
    }

    let common_len = base_components
        .iter()
        .zip(target_components.iter())
        .take_while(|(base, target)| base == target)
        .count();

    let mut out = PathBuf::new();
    for _ in common_len..base_components.len() {
        out.push("..");
    }
    for component in &target_components[common_len..] {
        out.push(component);
    }

    if out.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        out
    }
}

fn path_components(path: &Path) -> Vec<OsString> {
    path.components()
        .filter_map(|component| match component {
            Component::CurDir => None,
            Component::ParentDir => Some(OsString::from("..")),
            Component::Normal(part) => Some(part.to_os_string()),
            Component::RootDir => Some(OsString::from(std::path::MAIN_SEPARATOR.to_string())),
            Component::Prefix(prefix) => Some(prefix.as_os_str().to_os_string()),
        })
        .collect()
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

fn reference_id_range(text: &str, range: &Range<usize>, id: &str) -> Option<Range<usize>> {
    let source = text.get(range.clone())?;
    let needle = format!("#{id}");
    let start = source.find(&needle)? + 1;
    Some(range.start + start..range.start + start + id.len())
}

fn reference_target_id_range(
    text: &str,
    source: &Range<usize>,
    target: &RefTarget,
) -> Option<Range<usize>> {
    let id = match target {
        RefTarget::Internal { id } => id,
        RefTarget::External { id: Some(id), .. } => id,
        RefTarget::External { id: None, .. } | RefTarget::Url(_) => return None,
    };
    reference_id_range(text, source, id)
}

fn reference_target_path_range(
    text: &str,
    source: &Range<usize>,
    target: &RefTarget,
) -> Option<Range<usize>> {
    let path = match target {
        RefTarget::External { path, .. } => path,
        RefTarget::Internal { .. } | RefTarget::Url(_) => return None,
    };
    if path.is_empty() {
        return None;
    }
    let source_text = text.get(source.clone())?;
    let start = source_text.find(path)?;
    Some(source.start + start..source.start + start + path.len())
}

fn task_prev_reference(text: &str, span: &Range<usize>, attrs: &Attributes) -> Option<Reference> {
    let prev = attrs.get_value("prev")?.to_string();
    let target = parse_dst(&prev);
    match &target {
        RefTarget::Internal { .. } => {}
        RefTarget::External { path, id: Some(_) } if is_djot_path(path) => {}
        RefTarget::External { .. } | RefTarget::Url(_) => return None,
    }

    let source = attribute_value_range(text, span, "prev", &prev)?;
    let target_path_range = reference_target_path_range(text, &source, &target);
    let target_id_range = reference_target_id_range(text, &source, &target);
    Some(Reference {
        source,
        target_path_range,
        target_id_range,
        target,
        kind: ReferenceKind::TaskPrev,
    })
}

fn task_dependency_references(
    text: &str,
    span: &Range<usize>,
    attrs: &Attributes,
) -> Vec<Reference> {
    task_dependencies(text, span, attrs)
        .into_iter()
        .map(|dependency| Reference {
            target_path_range: dependency_target_path_range(&dependency),
            target_id_range: dependency_target_id_range(&dependency),
            source: dependency.range,
            target: dependency.target,
            kind: ReferenceKind::TaskDependency,
        })
        .collect()
}

fn task_dependencies(text: &str, span: &Range<usize>, attrs: &Attributes) -> Vec<TaskDependency> {
    let Some(depends) = attrs.get_value("depends").map(|value| value.to_string()) else {
        return Vec::new();
    };
    let Some(value_range) = attribute_value_range(text, span, "depends", &depends) else {
        return Vec::new();
    };
    dependency_tokens(&depends)
        .into_iter()
        .map(|(source, relative_range)| TaskDependency {
            target: parse_dependency_target(source),
            source: source.to_string(),
            range: value_range.start + relative_range.start..value_range.start + relative_range.end,
        })
        .collect()
}

fn dependency_tokens(source: &str) -> Vec<(&str, Range<usize>)> {
    let mut tokens = Vec::new();
    let mut cursor = 0;
    while cursor < source.len() {
        while source
            .as_bytes()
            .get(cursor)
            .is_some_and(|byte| byte.is_ascii_whitespace())
        {
            cursor += 1;
        }
        let start = cursor;
        while source
            .as_bytes()
            .get(cursor)
            .is_some_and(|byte| !byte.is_ascii_whitespace())
        {
            cursor += 1;
        }
        if start < cursor {
            tokens.push((&source[start..cursor], start..cursor));
        }
    }
    tokens
}

fn parse_dependency_target(source: &str) -> RefTarget {
    if let Some(id) = source.strip_prefix('#') {
        RefTarget::Internal { id: id.to_string() }
    } else if let Some((path, id)) = source.split_once('#') {
        RefTarget::External {
            path: percent_decode_path(path),
            id: Some(id.to_string()),
        }
    } else {
        RefTarget::Url(source.to_string())
    }
}

fn dependency_target_path_range(dependency: &TaskDependency) -> Option<Range<usize>> {
    match &dependency.target {
        RefTarget::External { .. } => {
            let source = dependency.source.as_str();
            let hash = source.find('#')?;
            if hash == 0 {
                None
            } else {
                Some(dependency.range.start..dependency.range.start + hash)
            }
        }
        RefTarget::Internal { .. } | RefTarget::Url(_) => None,
    }
}

fn dependency_target_id_range(dependency: &TaskDependency) -> Option<Range<usize>> {
    match &dependency.target {
        RefTarget::Internal { .. } => {
            let start = dependency.range.start
                + dependency
                    .source
                    .strip_prefix('#')
                    .map_or(0, |_| '#'.len_utf8());
            Some(start..dependency.range.end)
        }
        RefTarget::External { .. } => {
            let hash = dependency.source.find('#')?;
            let start = dependency.range.start + hash + '#'.len_utf8();
            Some(start..dependency.range.end)
        }
        RefTarget::Url(_) => None,
    }
}

fn percent_decode_path(path: &str) -> String {
    let mut decoded = Vec::with_capacity(path.len());
    let bytes = path.as_bytes();
    let mut cursor = 0;
    while cursor < bytes.len() {
        if bytes[cursor] == b'%' && cursor + 2 < bytes.len() {
            if let Some(byte) = hex_byte(bytes[cursor + 1], bytes[cursor + 2]) {
                decoded.push(byte);
                cursor += 3;
                continue;
            }
        }
        decoded.push(bytes[cursor]);
        cursor += 1;
    }
    String::from_utf8(decoded).unwrap_or_else(|_| path.to_string())
}

fn hex_byte(high: u8, low: u8) -> Option<u8> {
    Some(hex_digit(high)? * 16 + hex_digit(low)?)
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn attribute_value_range(
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outline_nests_by_section_level() {
        let text = "# A\n\ntext\n\n## B\n\n### C\n\n# D\n";
        let roots = heading_outline(text);
        assert_eq!(
            roots.iter().map(|h| h.name.as_str()).collect::<Vec<_>>(),
            ["A", "D"]
        );
        let a = &roots[0];
        assert_eq!(a.level, 1);
        assert_eq!(
            a.children
                .iter()
                .map(|h| h.name.as_str())
                .collect::<Vec<_>>(),
            ["B"]
        );
        assert_eq!(a.children[0].children[0].name, "C");
        // Parent section range encloses its children.
        assert!(a.range.end >= a.children[0].range.end);
    }

    #[test]
    fn index_collects_anchors_and_references() {
        let text = "# My Heading\n\n[a](#My-Heading) [b][] [u](https://x.y) [f](o.dj#s)\n\n## b\n";
        let index = build_index(text);
        assert!(index.anchors.contains_key("My-Heading"));
        assert!(index.anchors.contains_key("b"));

        let targets: Vec<_> = index.references.iter().map(|r| &r.target).collect();
        assert!(targets.contains(&&RefTarget::Internal {
            id: "My-Heading".into()
        }));
        assert!(targets.contains(&&RefTarget::Url("https://x.y".into())));
        assert!(targets.contains(&&RefTarget::External {
            path: "o.dj".into(),
            id: Some("s".into()),
        }));
    }

    #[test]
    fn analysis_collects_shared_document_semantics() {
        let text = "{.metadata}\n``` toml\ntitle = \"x\"\n```\n\n# Topic\n\n{#task-a recur=\"P1Q\"}\n::: task\nTask A.\n:::\n\n[topic](#Topic)\n";
        let analysis = analyze(text);

        assert_eq!(analysis.metadata.as_deref(), Some("title = \"x\"\n"));
        assert!(analysis.index.anchors.contains_key("Topic"));
        assert_eq!(analysis.index.references.len(), 1);
        assert_eq!(analysis.tasks.len(), 1);
        assert_eq!(analysis.tasks[0].id.as_deref(), Some("task-a"));
        assert!(analysis.diagnostics.iter().any(|diagnostic| {
            diagnostic.kind
                == DiagnosticKind::InvalidTaskRecur {
                    recur: "P1Q".into(),
                }
        }));
        assert!(analysis
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.kind == DiagnosticKind::MissingTaskDueForRecur));
    }

    #[test]
    fn index_tracks_anchor_rename_ranges() {
        let text = "# My Heading\n\n{#custom}\nparagraph\n\n{prev=\"#quoted\" id=\"quoted\"}\nquoted paragraph\n\n{id=bare}\nbare paragraph\n\n{id=\"学习-anki\"}\nunicode paragraph\n";
        let index = build_index(text);

        let heading = &index.anchors["My-Heading"];
        assert_eq!(&text[heading.rename_range.clone()], "My Heading");
        assert!(!heading.explicit);

        let explicit = &index.anchors["custom"];
        assert_eq!(&text[explicit.rename_range.clone()], "custom");
        assert!(explicit.explicit);

        let quoted = &index.anchors["quoted"];
        assert_eq!(&text[quoted.rename_range.clone()], "quoted");
        assert!(quoted.explicit);

        let bare = &index.anchors["bare"];
        assert_eq!(&text[bare.rename_range.clone()], "bare");
        assert!(bare.explicit);

        let unicode = &index.anchors["学习-anki"];
        assert_eq!(&text[unicode.rename_range.clone()], "学习-anki");
        assert!(unicode.explicit);
    }

    #[test]
    fn index_tracks_reference_target_id_ranges() {
        let text = "[internal](#Topic) [external](other.dj#Section) [file](other.dj) [implicit][]";
        let index = build_index(text);

        let ranges = index
            .references
            .iter()
            .filter_map(|reference| {
                reference
                    .target_id_range
                    .clone()
                    .map(|range| text[range].to_string())
            })
            .collect::<Vec<_>>();

        assert_eq!(ranges, ["Topic", "Section"]);
    }

    #[test]
    fn index_tracks_reference_target_path_ranges() {
        let text = "[internal](#Topic) [external](other.dj#Section) [file](notes/other.dj) [url](https://example.com)";
        let index = build_index(text);

        let ranges = index
            .references
            .iter()
            .filter_map(|reference| {
                reference
                    .target_path_range
                    .clone()
                    .map(|range| text[range].to_string())
            })
            .collect::<Vec<_>>();

        assert_eq!(ranges, ["other.dj", "notes/other.dj"]);
    }

    #[test]
    fn index_tracks_task_prev_references() {
        let text = "{prev=\"#old-task\"}\n::: task\nNext task.\n:::\n\n{prev=\"other.dj#previous\"}\n::: task\nCross-file next task.\n:::\n\n{prev=\"other.dj\"}\n::: task\nFile-only prev is not a reference.\n:::\n";
        let index = build_index(text);

        let refs = index
            .references
            .iter()
            .map(|reference| {
                (
                    text[reference.source.clone()].to_string(),
                    reference
                        .target_path_range
                        .clone()
                        .map(|range| text[range].to_string()),
                    reference
                        .target_id_range
                        .clone()
                        .map(|range| text[range].to_string()),
                    reference.target.clone(),
                    reference.kind,
                )
            })
            .collect::<Vec<_>>();

        assert_eq!(
            refs,
            vec![
                (
                    "#old-task".to_string(),
                    None,
                    Some("old-task".to_string()),
                    RefTarget::Internal {
                        id: "old-task".to_string()
                    },
                    ReferenceKind::TaskPrev,
                ),
                (
                    "other.dj#previous".to_string(),
                    Some("other.dj".to_string()),
                    Some("previous".to_string()),
                    RefTarget::External {
                        path: "other.dj".to_string(),
                        id: Some("previous".to_string()),
                    },
                    ReferenceKind::TaskPrev,
                ),
            ]
        );
    }

    #[test]
    fn metadata_block_extracts_leading_toml() {
        let text = "{.metadata}\n``` toml\ntitle = \"x\"\n```\n\n# H\n";
        assert_eq!(metadata_block(text).as_deref(), Some("title = \"x\"\n"));
        // A plain code block is not metadata.
        assert_eq!(metadata_block("``` toml\ntitle = \"x\"\n```\n"), None);
    }

    #[test]
    fn tasks_extract_task_divs() {
        let text = "{#write-parser}\n{created=\"2026-06-18T09:00:00+08:00\" due=\"2026-06-20T09:00:00+08:00\" wait=\"2026-06-19T09:00:00+08:00\" done=\"2026-06-19T21:30:00+08:00\" canceled=\"2026-06-19T22:00:00+08:00\" recur=\"P1W\" prev=\"#previous-task\"}\n::: task\nWrite parser.\n\nDetails.\n:::\n\n::: note\nNot a task.\n:::\n";
        let found = tasks(text);

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id.as_deref(), Some("write-parser"));
        assert_eq!(
            found[0].created.as_deref(),
            Some("2026-06-18T09:00:00+08:00")
        );
        assert_eq!(found[0].done.as_deref(), Some("2026-06-19T21:30:00+08:00"));
        assert_eq!(
            found[0].canceled.as_deref(),
            Some("2026-06-19T22:00:00+08:00")
        );
        assert_eq!(found[0].due.as_deref(), Some("2026-06-20T09:00:00+08:00"));
        assert_eq!(found[0].wait.as_deref(), Some("2026-06-19T09:00:00+08:00"));
        assert_eq!(found[0].recur.as_deref(), Some("P1W"));
        assert_eq!(found[0].prev.as_deref(), Some("#previous-task"));
        assert_eq!(found[0].title, "Write parser.");
        assert_eq!(
            found[0]
                .title_range
                .clone()
                .map(|range| text[range].to_string()),
            Some("Write parser.".to_string())
        );
    }

    #[test]
    fn tasks_inherit_metadata_from_containing_list_item() {
        let text = "- {#write-parser created=\"2026-06-18T09:00:00Z\" canceled=\"2026-06-18T18:00:00Z\" due=\"2026-06-19T09:00:00Z\" wait=\"2026-06-18T21:00:00Z\" recur=\"P1D\" prev=\"#previous-task\"}\n  ::: task\n  Write parser.\n  :::\n";
        let found = tasks(text);

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id.as_deref(), Some("write-parser"));
        assert_eq!(found[0].created.as_deref(), Some("2026-06-18T09:00:00Z"));
        assert_eq!(found[0].due.as_deref(), Some("2026-06-19T09:00:00Z"));
        assert_eq!(found[0].wait.as_deref(), Some("2026-06-18T21:00:00Z"));
        assert_eq!(found[0].recur.as_deref(), Some("P1D"));
        assert_eq!(found[0].prev.as_deref(), Some("#previous-task"));
        assert_eq!(found[0].done, None);
        assert_eq!(found[0].canceled.as_deref(), Some("2026-06-18T18:00:00Z"));
        assert_eq!(found[0].title, "Write parser.");
    }

    #[test]
    fn tasks_report_depth_for_nested_task_divs() {
        let text = "::: task\nParent.\n\n::: task\nChild.\n\n::: task\nGrandchild.\n:::\n:::\n:::\n\n::: task\nSibling.\n:::\n";
        let found = tasks(text);

        assert_eq!(found.len(), 4);
        assert_eq!(
            found
                .iter()
                .map(|task| (task.title.as_str(), task.depth))
                .collect::<Vec<_>>(),
            vec![
                ("Parent.", 0),
                ("Child.", 1),
                ("Grandchild.", 2),
                ("Sibling.", 0)
            ]
        );
    }

    #[test]
    fn tasks_extract_dependency_tokens() {
        let text =
            "{depends=\"#draft #review other%20file.dj#publish\"}\n::: task\nBlocked task.\n:::\n";
        let found = tasks(text);

        assert_eq!(found.len(), 1);
        assert_eq!(
            found[0]
                .depends
                .iter()
                .map(|dependency| (dependency.source.as_str(), &dependency.target))
                .collect::<Vec<_>>(),
            vec![
                (
                    "#draft",
                    &RefTarget::Internal {
                        id: "draft".to_string()
                    }
                ),
                (
                    "#review",
                    &RefTarget::Internal {
                        id: "review".to_string()
                    }
                ),
                (
                    "other%20file.dj#publish",
                    &RefTarget::External {
                        path: "other file.dj".to_string(),
                        id: Some("publish".to_string())
                    }
                ),
            ]
        );
        assert_eq!(
            found[0]
                .depends
                .iter()
                .map(|dependency| text[dependency.range.clone()].to_string())
                .collect::<Vec<_>>(),
            vec!["#draft", "#review", "other%20file.dj#publish"]
        );
    }

    #[test]
    fn tasks_prefer_div_wait_over_containing_list_item() {
        let text = "- {wait=\"2026-06-18T21:00:00Z\"}\n  {wait=\"2026-06-19T09:00:00Z\"}\n  ::: task\n  Write parser.\n  :::\n";
        let found = tasks(text);

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].wait.as_deref(), Some("2026-06-19T09:00:00Z"));
    }

    #[test]
    fn tasks_reject_date_only_datetime_attributes() {
        let text = "{created=\"2026-06-18\" done=2026-06-19 canceled=2026-06-20 wait=\"2026-06-21\"}\n::: task\nDate-only metadata.\n:::\n\n{created=\"2026-06-18T09:00:00Z\" done=\"2026-06-19T13:30:00Z\" canceled=\"2026-06-20T13:30:00Z\" wait=\"2026-06-21T09:00:00Z\"}\n::: task\nDatetime metadata.\n:::\n";
        let found = tasks(text);

        assert_eq!(found.len(), 2);
        assert_eq!(found[0].created, None);
        assert_eq!(found[0].done, None);
        assert_eq!(found[0].canceled, None);
        assert_eq!(found[0].wait, None);
        assert_eq!(found[1].created.as_deref(), Some("2026-06-18T09:00:00Z"));
        assert_eq!(found[1].done.as_deref(), Some("2026-06-19T13:30:00Z"));
        assert_eq!(found[1].canceled.as_deref(), Some("2026-06-20T13:30:00Z"));
        assert_eq!(found[1].wait.as_deref(), Some("2026-06-21T09:00:00Z"));
    }

    #[test]
    fn task_done_edits_by_id_mark_task_done() {
        let text = "{#write-parser}\n::: task\nWrite parser.\n:::\n";
        let edits =
            task_done_edits_by_id(text, "write-parser", "2026-06-22T09:00:00+08:00").unwrap();
        let updated = apply_task_text_edits(text.to_string(), edits).unwrap();

        assert_eq!(
            updated,
            "{#write-parser}\n{done=\"2026-06-22T09:00:00+08:00\"}\n::: task\nWrite parser.\n:::\n"
        );
    }

    #[test]
    fn metadata_insertion_edit_adds_leading_metadata_block() {
        let text = "\n\n# Heading\n";
        let edit = metadata_insertion_edit(
            text,
            1,
            Path::new("/notes/my \"note\".dj"),
            "2026-06-22T09:00:00+08:00",
        )
        .unwrap();

        assert_eq!(edit.range, 0..0);
        assert_eq!(
            edit.new_text,
            "{.metadata}\n``` toml\ntitle = \"my \\\"note\\\"\"\ncreated = \"2026-06-22T09:00:00+08:00\"\n```\n\n"
        );
        assert!(metadata_insertion_edit("# Heading\n", 2, Path::new("x.dj"), "now").is_none());
    }

    #[test]
    fn task_list_item_conversion_edit_converts_open_native_task() {
        let text = "# Tasks\n\n  - [ ] Write parser.\n";
        let edit =
            task_list_item_conversion_edit(text, text.find("Write").unwrap(), "created").unwrap();

        assert_eq!(&text[edit.range.clone()], "  - [ ] Write parser.");
        assert_eq!(
            edit.new_text,
            "  - {created=\"created\"}\n    ::: task\n    Write parser.\n    :::"
        );
    }

    #[test]
    fn resolve_target_handles_internal_relative_and_url() {
        let from = PathBuf::from("/notes/sub/a.dj");
        assert_eq!(
            resolve_target(&from, &RefTarget::Internal { id: "x".into() }).unwrap(),
            ResolvedTarget {
                path: from.clone(),
                id: Some("x".into())
            }
        );
        assert_eq!(
            resolve_target(
                &from,
                &RefTarget::External {
                    path: "../b.dj".into(),
                    id: Some("y".into())
                }
            )
            .unwrap(),
            ResolvedTarget {
                path: PathBuf::from("/notes/b.dj"),
                id: Some("y".into())
            }
        );
        assert!(resolve_target(&from, &RefTarget::Url("https://x".into())).is_none());
    }

    #[test]
    fn workspace_cross_file_definition_and_backref() {
        let a = PathBuf::from("/notes/a.dj");
        let b = PathBuf::from("/notes/b.dj");
        let doc_a = "# A\n\nsee [to B](b.dj#Topic)\n";
        let mut ws = Workspace::new();
        ws.insert(a.clone(), doc_a.to_string());
        ws.insert(b.clone(), "# Topic\n\ntext\n".to_string());

        // Cursor on the link in a.dj resolves to b.dj#Topic, which exists.
        let offset = doc_a.find("b.dj").unwrap();
        let reference = ws.reference_at(&a, offset).expect("reference under cursor");
        let resolved = resolve_target(&a, &reference.target).expect("resolved");
        assert_eq!(resolved.path, b);
        assert_eq!(resolved.id.as_deref(), Some("Topic"));
        assert!(ws.anchor(&resolved.path, "Topic").is_some());
        let topic_text_offset = ws.get(&b).unwrap().text.find("Topic").unwrap();
        assert_eq!(ws.anchor_at(&b, topic_text_offset).unwrap().0, "Topic");

        // Backward: exactly one document references (b.dj, Topic).
        let back = ws.references_to(&b, "Topic");
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].0, a);
    }

    #[test]
    fn workspace_resolves_rename_target_from_anchor_or_reference() {
        let a = PathBuf::from("/notes/a.dj");
        let b = PathBuf::from("/notes/b.dj");
        let doc_a = "# A\n\nsee [to B](b.dj#topic)\n";
        let doc_b = "{#topic}\nTopic\n";
        let mut ws = Workspace::new();
        ws.insert(a.clone(), doc_a.to_string());
        ws.insert(b.clone(), doc_b.to_string());

        let from_anchor = ws
            .rename_target_at(&b, doc_b.find("topic").unwrap())
            .expect("rename target from anchor");
        assert_eq!(from_anchor.path, b);
        assert_eq!(from_anchor.id, "topic");
        assert_eq!(&doc_b[from_anchor.range], "topic");

        let from_reference = ws
            .rename_target_at(&a, doc_a.find("topic").unwrap())
            .expect("rename target from reference");
        assert_eq!(from_reference.path, PathBuf::from("/notes/b.dj"));
        assert_eq!(from_reference.id, "topic");
        assert_eq!(&doc_a[from_reference.range], "topic");
        assert_eq!(
            ws.rename_target_at(&a, doc_a.find("b.dj").unwrap()),
            Err(RenameTargetError::NotRenameable)
        );
    }

    #[test]
    fn workspace_renames_anchor_only_from_rename_range() {
        let path = PathBuf::from("/notes/tasks.dj");
        let doc = "{#topic}\n::: task\nTask title.\n:::\n\n- {#list-task}\n  ::: task\n  List task title.\n  :::\n";
        let mut ws = Workspace::new();
        ws.insert(path.clone(), doc.to_string());

        let from_anchor = ws
            .rename_target_at(&path, doc.find("topic").unwrap())
            .expect("rename target from explicit anchor");
        assert_eq!(from_anchor.id, "topic");
        assert_eq!(&doc[from_anchor.range], "topic");

        let from_list_anchor = ws
            .rename_target_at(&path, doc.find("list-task").unwrap())
            .expect("rename target from list item anchor");
        assert_eq!(from_list_anchor.id, "list-task");
        assert_eq!(&doc[from_list_anchor.range], "list-task");

        assert_eq!(
            ws.rename_target_at(&path, doc.find("Task title").unwrap()),
            Err(RenameTargetError::NotRenameable)
        );
        assert_eq!(
            ws.rename_target_at(&path, doc.find("List task title").unwrap()),
            Err(RenameTargetError::NotRenameable)
        );
    }

    #[test]
    fn workspace_collects_anchor_rename_edits() {
        let a = PathBuf::from("/notes/a.dj");
        let b = PathBuf::from("/notes/b.dj");
        let doc_a =
            "# A\n\n[local](#A) [other](b.dj#topic) [file](b.dj)\n\n{prev=\"b.dj#topic\"}\n::: task\nNext.\n:::\n";
        let doc_b = "{#topic}\nTopic\n\n[back](../notes/a.dj#A)\n";
        let mut ws = Workspace::new();
        ws.insert(a.clone(), doc_a.to_string());
        ws.insert(b.clone(), doc_b.to_string());

        let mut document_edits = ws
            .anchor_rename_edits(&b, "topic", "renamed")
            .into_iter()
            .map(|edit| {
                let text = &ws.get(&edit.path).unwrap().text;
                (
                    edit.path,
                    text[edit.edit.range].to_string(),
                    edit.edit.new_text,
                )
            })
            .collect::<Vec<_>>();
        document_edits.sort_by(|a, b| a.0.cmp(&b.0));

        assert_eq!(
            document_edits,
            vec![
                (
                    PathBuf::from("/notes/a.dj"),
                    "topic".to_string(),
                    "renamed".to_string()
                ),
                (
                    PathBuf::from("/notes/a.dj"),
                    "topic".to_string(),
                    "renamed".to_string()
                ),
                (
                    PathBuf::from("/notes/b.dj"),
                    "topic".to_string(),
                    "renamed".to_string()
                )
            ]
        );
    }

    #[test]
    fn workspace_rejects_rename_for_implicit_heading_anchor() {
        let a = PathBuf::from("/notes/a.dj");
        let b = PathBuf::from("/notes/b.dj");
        let doc_a = "# A\n\nsee [to B](b.dj#Topic)\n";
        let doc_b = "# Topic\n";
        let mut ws = Workspace::new();
        ws.insert(a.clone(), doc_a.to_string());
        ws.insert(b.clone(), doc_b.to_string());

        assert_eq!(
            ws.rename_target_at(&b, doc_b.find("Topic").unwrap()),
            Err(RenameTargetError::ImplicitHeadingAnchor)
        );
        assert_eq!(
            ws.rename_target_at(&a, doc_a.find("Topic").unwrap()),
            Err(RenameTargetError::ImplicitHeadingAnchor)
        );
        assert!(ws.anchor_rename_edits(&b, "Topic", "Renamed").is_empty());
    }

    #[test]
    fn workspace_resolves_path_rename_target_from_link_path() {
        let a = PathBuf::from("/notes/a.dj");
        let b = PathBuf::from("/notes/b.dj");
        let doc_a = "# A\n\nsee [to B](b.dj#topic)\n";
        let mut ws = Workspace::new();
        ws.insert(a.clone(), doc_a.to_string());
        ws.insert(b.clone(), "{#topic}\nTopic\n".to_string());

        let target = ws
            .path_rename_target_at(&a, doc_a.find("b.dj").unwrap())
            .expect("path rename target");

        assert_eq!(target.old_path, b);
        assert_eq!(&doc_a[target.range], "b.dj");
        assert_eq!(
            ws.path_rename_target_at(&a, doc_a.find("topic").unwrap()),
            Err(PathRenameError::NotRenameable)
        );
    }

    #[test]
    fn workspace_collects_path_rename_edit_plan_with_relative_replacements() {
        let a = PathBuf::from("/notes/a.dj");
        let b = PathBuf::from("/notes/b.dj");
        let c = PathBuf::from("/notes/sub/c.dj");
        let renamed = PathBuf::from("/notes/renamed.dj");
        let doc_a = "# A\n\n[topic](b.dj#topic)\n";
        let doc_c = "# C\n\n[topic](../b.dj)\n";
        let mut ws = Workspace::new();
        ws.insert(a.clone(), doc_a.to_string());
        ws.insert(b.clone(), "{#topic}\nTopic\n".to_string());
        ws.insert(c.clone(), doc_c.to_string());

        let plan = ws.path_rename_edit_plan(&b, &renamed);
        assert_eq!(
            plan.first(),
            Some(&WorkspaceEdit::RenameFile(FileRenameEdit {
                old_path: b,
                new_path: renamed,
            }))
        );

        let mut text_edits = plan
            .into_iter()
            .filter_map(|edit| match edit {
                WorkspaceEdit::Text(edit) => {
                    let text = &ws.get(&edit.path).unwrap().text;
                    Some((
                        edit.path,
                        text[edit.edit.range].to_string(),
                        edit.edit.new_text,
                    ))
                }
                WorkspaceEdit::RenameFile(_) => None,
            })
            .collect::<Vec<_>>();
        text_edits.sort_by(|a, b| a.0.cmp(&b.0));

        assert_eq!(
            text_edits,
            vec![
                (
                    PathBuf::from("/notes/a.dj"),
                    "b.dj".to_string(),
                    "renamed.dj".to_string()
                ),
                (
                    PathBuf::from("/notes/sub/c.dj"),
                    "../b.dj".to_string(),
                    "../renamed.dj".to_string()
                ),
            ]
        );
    }

    #[test]
    fn workspace_reports_unresolved_references() {
        let a = PathBuf::from("/notes/a.dj");
        let b = PathBuf::from("/notes/b.dj");
        let doc_a = "# A\n\n[bad](#Missing) [file](missing.dj) [anchor](b.dj#Nope) [plain](AGENTS.md) [dir](crates/djot-core) [license](LICENSE) [ok](https://example.com)\n";
        let mut ws = Workspace::new();
        ws.insert(a.clone(), doc_a.to_string());
        ws.insert(b, "# Existing\n".to_string());

        let diagnostics = ws.diagnostics_for(&a);
        assert_eq!(diagnostics.len(), 3);
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.kind
                == DiagnosticKind::UnresolvedAnchor {
                    id: "Missing".into(),
                }
        }));
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.kind
                == DiagnosticKind::UnresolvedPath {
                    path: "missing.dj".into(),
                }
        }));
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.kind == DiagnosticKind::UnresolvedAnchor { id: "Nope".into() }
        }));
    }

    #[test]
    fn workspace_reports_invalid_recurring_task_metadata() {
        let path = PathBuf::from("/notes/tasks.dj");
        let doc = "{recur=\"P1W\"}\n::: task\nMissing due.\n:::\n\n{due=\"2026-06-21T09:00:00+08:00\" recur=\"P1M1D\"}\n::: task\nInvalid recur.\n:::\n\n{due=\"2026-06-21T09:00:00+08:00\" recur=\"P1W\"}\n::: task\nValid recur.\n:::\n";
        let mut ws = Workspace::new();
        ws.insert(path.clone(), doc.to_string());

        let diagnostics = ws.diagnostics_for(&path);
        assert_eq!(diagnostics.len(), 2);
        assert!(diagnostics
            .iter()
            .any(|diagnostic| diagnostic.kind == DiagnosticKind::MissingTaskDueForRecur));
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.kind
                == DiagnosticKind::InvalidTaskRecur {
                    recur: "P1M1D".into(),
                }
        }));
    }

    #[test]
    fn workspace_reports_conflicting_task_closed_state() {
        let path = PathBuf::from("/notes/tasks.dj");
        let doc = "{done=\"2026-06-21T09:00:00Z\" canceled=\"2026-06-21T10:00:00Z\"}\n::: task\nConflicting task.\n:::\n";
        let mut ws = Workspace::new();
        ws.insert(path.clone(), doc.to_string());

        let diagnostics = ws.diagnostics_for(&path);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(
            diagnostics[0].kind,
            DiagnosticKind::ConflictingTaskClosedState
        );
        assert_eq!(&doc[diagnostics[0].range.clone()], doc);
    }

    #[test]
    fn workspace_reports_task_prev_target_that_is_not_a_task() {
        let path = PathBuf::from("/notes/tasks.dj");
        let doc = "{#note}\nPlain anchor.\n\n{prev=\"#note\"}\n::: task\nFollow-up task.\n:::\n";
        let mut ws = Workspace::new();
        ws.insert(path.clone(), doc.to_string());

        let diagnostics = ws.diagnostics_for(&path);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(
            diagnostics[0].kind,
            DiagnosticKind::InvalidTaskPrevTarget { id: "note".into() }
        );
        assert_eq!(&doc[diagnostics[0].range.clone()], "#note");
    }

    #[test]
    fn workspace_accepts_task_prev_target_inherited_from_list_item() {
        let path = PathBuf::from("/notes/tasks.dj");
        let doc = "- {#previous-task}\n  ::: task\n  Previous task.\n  :::\n\n{prev=\"#previous-task\"}\n::: task\nFollow-up task.\n:::\n";
        let mut ws = Workspace::new();
        ws.insert(path.clone(), doc.to_string());

        assert_eq!(ws.diagnostics_for(&path), Vec::new());
    }

    #[test]
    fn workspace_resolves_task_dependencies_and_blocked_state() {
        let a = PathBuf::from("/notes/a.dj");
        let b = PathBuf::from("/notes/b.dj");
        let doc_a = "{#draft}\n::: task\nDraft.\n:::\n\n{#done done=\"2026-06-21T09:00:00Z\"}\n::: task\nDone.\n:::\n\n{#blocked depends=\"#draft b.dj#review\"}\n::: task\nBlocked.\n:::\n\n{#ready depends=\"#done\"}\n::: task\nReady.\n:::\n";
        let doc_b = "{#review}\n::: task\nReview.\n:::\n";
        let mut ws = Workspace::new();
        ws.insert(a.clone(), doc_a.to_string());
        ws.insert(b.clone(), doc_b.to_string());

        let blocked = ws.task_by_id(&a, "blocked").unwrap();
        let ready = ws.task_by_id(&a, "ready").unwrap();
        assert_eq!(
            ws.open_task_dependencies(&a, &blocked)
                .into_iter()
                .map(|dependency| dependency.target)
                .collect::<Vec<_>>(),
            vec![
                TaskRef {
                    path: a.clone(),
                    id: "draft".to_string(),
                },
                TaskRef {
                    path: b.clone(),
                    id: "review".to_string(),
                },
            ]
        );
        assert!(ws.is_task_blocked(&a, &blocked));
        assert!(!ws.is_task_blocked(&a, &ready));
        assert_eq!(
            ws.directly_blocking_tasks(&a, "draft"),
            vec![TaskRef {
                path: a.clone(),
                id: "blocked".to_string(),
            }]
        );
    }

    #[test]
    fn workspace_reports_invalid_task_dependencies() {
        let path = PathBuf::from("/notes/tasks.dj");
        let doc = "{#note}\nNot a task.\n\n{#missing-depends depends=\"#missing\"}\n::: task\nMissing.\n:::\n\n{#bare-depends depends=\"missing\"}\n::: task\nBare.\n:::\n\n{#non-task-depends depends=\"#note\"}\n::: task\nNon task.\n:::\n\n{#self-depends depends=\"#self-depends\"}\n::: task\nSelf.\n:::\n";
        let mut ws = Workspace::new();
        ws.insert(path.clone(), doc.to_string());

        let diagnostics = ws.diagnostics_for(&path);
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.kind
                == DiagnosticKind::UnresolvedAnchor {
                    id: "missing".to_string(),
                }
        }));
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.kind
                == DiagnosticKind::InvalidTaskDependencyTarget {
                    target: "missing".to_string(),
                }
        }));
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.kind
                == DiagnosticKind::InvalidTaskDependencyTarget {
                    target: "#note".to_string(),
                }
        }));
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.kind
                == DiagnosticKind::TaskSelfDependency {
                    target: "#self-depends".to_string(),
                }
        }));
    }

    #[test]
    fn workspace_reports_dependency_cycles_and_blocked_tasks() {
        let path = PathBuf::from("/notes/tasks.dj");
        let doc =
            "{#a depends=\"#b\"}\n::: task\nA.\n:::\n\n{#b depends=\"#a\"}\n::: task\nB.\n:::\n";
        let mut ws = Workspace::new();
        ws.insert(path.clone(), doc.to_string());

        let diagnostics = ws.diagnostics_for(&path);
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.kind == DiagnosticKind::TaskDependencyCycle { id: "a".into() }
        }));
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.kind == DiagnosticKind::TaskDependencyCycle { id: "b".into() }
        }));
        assert!(diagnostics
            .iter()
            .any(|diagnostic| { diagnostic.kind == DiagnosticKind::TaskBlocked { count: 1 } }));
    }

    #[test]
    fn workspace_reports_duplicate_anchors() {
        let path = PathBuf::from("/notes/tasks.dj");
        let doc =
            "{id=\"task\"}\n::: task\nFirst task.\n:::\n\n{id=task}\n::: task\nSecond task.\n:::\n";
        let mut ws = Workspace::new();
        ws.insert(path.clone(), doc.to_string());

        let diagnostics = ws.diagnostics_for(&path);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(
            diagnostics[0].kind,
            DiagnosticKind::DuplicateAnchor {
                id: "task".into(),
                first_range: 5..9,
            }
        );
        assert_eq!(&doc[diagnostics[0].range.clone()], "task");
    }

    #[test]
    fn repeat_rule_accepts_supported_iso_duration_subset() {
        assert_eq!(parse_repeat_rule("P1D"), Some(RepeatRule::Days(1)));
        assert_eq!(parse_repeat_rule("P2W"), Some(RepeatRule::Weeks(2)));
        assert_eq!(parse_repeat_rule("P1M"), Some(RepeatRule::Months(1)));
        assert_eq!(parse_repeat_rule("P1Y"), Some(RepeatRule::Years(1)));
        assert_eq!(parse_repeat_rule("P1M1D"), None);
        assert_eq!(parse_repeat_rule("PT1H"), None);
        assert_eq!(parse_repeat_rule("weekly"), None);
    }

    #[test]
    fn recur_due_supports_iso_week_duration() {
        let due = DateTime::parse_from_rfc3339("2026-06-21T17:00:00+08:00").unwrap();
        let next = next_recur_due(due, "P1W").unwrap();

        assert_eq!(next.to_rfc3339(), "2026-06-28T17:00:00+08:00");
    }

    #[test]
    fn recur_due_adds_calendar_months() {
        let due = DateTime::parse_from_rfc3339("2026-01-31T17:00:00+08:00").unwrap();
        let next = next_recur_due(due, "P1M").unwrap();

        assert_eq!(next.to_rfc3339(), "2026-02-28T17:00:00+08:00");
    }

    #[test]
    fn recur_due_adds_calendar_years() {
        let due = DateTime::parse_from_rfc3339("2024-02-29T17:00:00+08:00").unwrap();
        let next = next_recur_due(due, "P1Y").unwrap();

        assert_eq!(next.to_rfc3339(), "2025-02-28T17:00:00+08:00");
    }

    #[test]
    fn recur_due_rejects_composite_and_time_durations() {
        let due = DateTime::parse_from_rfc3339("2026-06-21T17:00:00+08:00").unwrap();

        assert!(next_recur_due(due, "P1M1D").is_none());
        assert!(next_recur_due(due, "PT1H").is_none());
        assert!(next_recur_due(due, "weekly").is_none());
    }

    #[test]
    fn anchor_attribute_uses_shorthand_only_for_ascii_name_ids() {
        assert_eq!(
            anchor_attribute("daily-review-2026-06-22"),
            "{#daily-review-2026-06-22}"
        );
        assert_eq!(
            anchor_attribute("学习-anki-2026-06-22"),
            "{id=\"学习-anki-2026-06-22\"}"
        );
        assert_eq!(
            anchor_attribute("quote\"backslash\\"),
            "{id=\"quote\\\"backslash\\\\\"}"
        );
    }

    #[test]
    fn recurring_attribute_filter_drops_instance_attribute_lines() {
        let source = "  {#task created=\"2026-06-21T00:00:00Z\" due=\"2026-06-22T00:00:00Z\" wait=\"2026-06-21T20:00:00Z\" recur=\"P1D\" done=\"2026-06-21T12:00:00Z\" canceled=\"2026-06-21T13:00:00Z\" prev=\"#old\"}\n  ::: task\n  Title\n  :::\n";

        assert_eq!(
            filter_recurring_instance_attributes(source),
            "  ::: task\n  Title\n  :::\n"
        );
    }

    #[test]
    fn recurring_attribute_filter_keeps_unknown_attribute_lines_verbatim() {
        let source = "  {project=\"anki\" priority=\"high\" .work}\n  ::: task\n  Title\n  :::\n";

        assert_eq!(filter_recurring_instance_attributes(source), source);
    }

    #[test]
    fn recurring_attribute_filter_rebuilds_mixed_attribute_lines() {
        let source = "  {project=\"anki cards\" recur=\"P1D\" priority=\"high\" #old}\n";

        assert_eq!(
            filter_recurring_instance_attributes(source),
            "  {project=\"anki cards\" priority=\"high\"}\n"
        );
    }

    #[test]
    fn recurring_attribute_filter_handles_quoted_spaces_and_escapes() {
        let source =
            "  {note=\"keep \\\"quoted\\\" value\" due=\"2026-06-22T00:00:00Z\" tag='two words'}\n";

        assert_eq!(
            filter_recurring_instance_attributes(source),
            "  {note=\"keep \\\"quoted\\\" value\" tag='two words'}\n"
        );
    }

    #[test]
    fn parse_dst_classifies_destinations() {
        assert_eq!(parse_dst("#sec"), RefTarget::Internal { id: "sec".into() });
        assert_eq!(
            parse_dst("mailto:a@b.c"),
            RefTarget::Url("mailto:a@b.c".into())
        );
        assert_eq!(
            parse_dst("other.dj"),
            RefTarget::External {
                path: "other.dj".into(),
                id: None
            }
        );
    }

    #[test]
    fn jotdown_cursor_link_parsing_shapes() {
        for (marked, expected_str) in [
            ("[|", Some("[")),
            ("[foo|", Some("[foo")),
            ("[foo|]", Some("[foo]")),
            ("[foo](|", Some("[foo](")),
            ("[foo](|)", None),
            ("[|]", Some("[]")),
        ] {
            let (text, cursor) = strip_cursor_marker(marked);
            assert_eq!(
                str_event_touching_cursor(&text, cursor).as_deref(),
                expected_str,
                "unexpected Str event at cursor for {marked:?}"
            );
        }

        let (text, cursor) = strip_cursor_marker("[foo](|)");
        assert!(
            Parser::new(&text).into_offset_iter().any(|(event, span)| {
                span.start <= cursor
                    && cursor <= span.end
                    && matches!(event, Event::End(Container::Link(_, _)))
            }),
            "cursor in a complete empty destination is in the link end syntax span"
        );
    }

    fn strip_cursor_marker(marked: &str) -> (String, usize) {
        let cursor = marked.find('|').expect("cursor marker");
        (marked.replace('|', ""), cursor)
    }

    fn str_event_touching_cursor(text: &str, cursor: usize) -> Option<String> {
        Parser::new(text)
            .into_offset_iter()
            .find_map(|(event, span)| match event {
                Event::Str(s) if span.start <= cursor && cursor <= span.end => Some(s.to_string()),
                _ => None,
            })
    }
}

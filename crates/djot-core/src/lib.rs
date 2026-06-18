//! Protocol-agnostic djot document analysis shared by the language server and
//! (in the future) the exporter.
//!
//! Everything here works in **byte offsets** into the source text. Consumers
//! that need editor coordinates (LSP UTF-16 positions) or a particular AST
//! (pandoc) convert at their own boundary — this crate never depends on those.

use std::collections::HashMap;
use std::ops::Range;
use std::path::{Component, Path, PathBuf};

use jotdown::{Attributes, Container, Event, Parser};
use serde::{Deserialize, Serialize};

/// The class that marks a leading code block as document metadata. This is a
/// djot-ls / djot-export convention layered on djot's native attribute syntax,
/// not part of djot itself — other djot tools simply see a classed code block.
pub const METADATA_CLASS: &str = "metadata";

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
}

/// A link in the document and what it points at.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reference {
    /// Byte range of the whole link, for cursor hit-testing.
    pub source: Range<usize>,
    /// Byte span of the target anchor id inside the link source, if this link
    /// names an anchor in editable source syntax.
    pub target_id_range: Option<Range<usize>>,
    pub target: RefTarget,
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
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct DocIndex {
    pub anchors: HashMap<String, Anchor>,
    pub references: Vec<Reference>,
}

/// Protocol-agnostic diagnostics produced by djot analysis.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalysisDiagnostic {
    pub range: Range<usize>,
    pub kind: DiagnosticKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiagnosticKind {
    UnresolvedAnchor { id: String },
    UnresolvedPath { path: String },
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
    let mut anchors: HashMap<String, Anchor> = HashMap::new();
    let mut references = Vec::new();
    let mut open_headings: Vec<HeadingAnchorFrame> = Vec::new();
    // Stack of (destination, start byte) for links currently open.
    let mut open_links: Vec<(String, usize)> = Vec::new();

    for (event, span) in Parser::new(text).into_offset_iter() {
        match event {
            // Headings carry the (possibly auto-generated) id directly.
            Event::Start(Container::Heading { id, .. }, _) => {
                open_headings.push(HeadingAnchorFrame {
                    id: id.into_owned(),
                    start: span.start,
                    text_range: None,
                });
            }
            Event::Start(container, attrs) => {
                // Any other element with an explicit {#id} is also an anchor.
                if let Some(id) = attrs.get_value("id") {
                    let id = id.to_string();
                    let rename_range =
                        explicit_id_range(text, &span, &id).unwrap_or_else(|| span.clone());
                    anchors.entry(id).or_insert_with(|| Anchor {
                        range: span.clone(),
                        rename_range,
                    });
                }
                if let Container::Link(dst, _) = container {
                    open_links.push((dst.into_owned(), span.start));
                }
            }
            Event::Str(_) => {
                if let Some(heading) = open_headings.last_mut() {
                    match &mut heading.text_range {
                        Some(range) => range.end = span.end,
                        None => heading.text_range = Some(span.clone()),
                    }
                }
            }
            Event::End(Container::Heading { .. }) => {
                if let Some(heading) = open_headings.pop() {
                    let range = heading.start..span.end;
                    let rename_range = explicit_id_range(text, &range, &heading.id)
                        .or(heading.text_range)
                        .unwrap_or_else(|| range.clone());
                    anchors.entry(heading.id).or_insert_with(|| Anchor {
                        range,
                        rename_range,
                    });
                }
            }
            Event::End(Container::Link(_, _)) => {
                if let Some((dst, start)) = open_links.pop() {
                    let source = start..span.end;
                    let target = parse_dst(&dst);
                    let target_id_range = reference_target_id_range(text, &source, &target);
                    references.push(Reference {
                        source,
                        target_id_range,
                        target,
                    });
                }
            }
            _ => {}
        }
    }

    DocIndex {
        anchors,
        references,
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
    let mut content = String::new();
    let mut in_meta = false;
    for (event, _) in Parser::new(text).into_offset_iter() {
        match event {
            Event::Start(Container::CodeBlock { .. }, attrs)
                if has_class(&attrs, METADATA_CLASS) =>
            {
                in_meta = true;
            }
            Event::Str(s) if in_meta => content.push_str(&s),
            Event::End(Container::CodeBlock { .. }) if in_meta => return Some(content),
            _ => {}
        }
    }
    None
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
/// boundary) and its parsed [`DocIndex`].
#[derive(Debug)]
pub struct DocEntry {
    pub text: String,
    pub index: DocIndex,
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
        let index = build_index(&text);
        self.docs.insert(normalize(&path), DocEntry { text, index });
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
            .index
            .references
            .iter()
            .find(|r| r.source.contains(&offset))
    }

    /// The anchor with `id` in the document at `path`.
    pub fn anchor(&self, path: &Path, id: &str) -> Option<&Anchor> {
        self.get(path)?.index.anchors.get(id)
    }

    /// The anchor whose source span covers `offset` in the document at `path`.
    pub fn anchor_at(&self, path: &Path, offset: usize) -> Option<(&str, &Anchor)> {
        self.get(path)?
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
            for reference in &entry.index.references {
                if let Some(resolved) = resolve_target(src, &reference.target) {
                    if resolved.path == target && resolved.id.as_deref() == Some(id) {
                        out.push((src.clone(), reference.source.clone()));
                    }
                }
            }
        }
        out
    }

    /// Diagnostics for unresolved file and anchor references in one loaded
    /// document. URLs are intentionally ignored.
    pub fn diagnostics_for(&self, path: &Path) -> Vec<AnalysisDiagnostic> {
        let Some(entry) = self.get(path) else {
            return Vec::new();
        };

        let mut diagnostics = Vec::new();
        for reference in &entry.index.references {
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
                if !target_entry.index.anchors.contains_key(&id) {
                    diagnostics.push(AnalysisDiagnostic {
                        range: reference.source.clone(),
                        kind: DiagnosticKind::UnresolvedAnchor { id },
                    });
                }
            }
        }

        diagnostics
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

fn explicit_id_range(text: &str, range: &Range<usize>, id: &str) -> Option<Range<usize>> {
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
    explicit_id_range(text, source, id)
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
    fn index_tracks_anchor_rename_ranges() {
        let text = "# My Heading\n\n{#custom}\nparagraph\n";
        let index = build_index(text);

        let heading = &index.anchors["My-Heading"];
        assert_eq!(&text[heading.rename_range.clone()], "My Heading");

        let explicit = &index.anchors["custom"];
        assert_eq!(&text[explicit.rename_range.clone()], "custom");
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
    fn metadata_block_extracts_leading_toml() {
        let text = "{.metadata}\n``` toml\ntitle = \"x\"\n```\n\n# H\n";
        assert_eq!(metadata_block(text).as_deref(), Some("title = \"x\"\n"));
        // A plain code block is not metadata.
        assert_eq!(metadata_block("``` toml\ntitle = \"x\"\n```\n"), None);
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

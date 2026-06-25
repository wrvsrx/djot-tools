use std::ops::Range;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::analysis::attribute_value_range;
use crate::cst::{link_syntax, Attributes};
use crate::paths::{is_djot_path, normalize, percent_decode_path};
use crate::tasks::TaskDependency;

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

pub(crate) fn reference_target_id_range(
    text: &str,
    source: &Range<usize>,
    target: &RefTarget,
) -> Option<Range<usize>> {
    let dst = link_syntax(text, source)?.dst_range;
    target_id_range(text.get(dst.clone())?, &dst, target)
}

pub(crate) fn reference_target_path_range(
    text: &str,
    source: &Range<usize>,
    target: &RefTarget,
) -> Option<Range<usize>> {
    let dst = link_syntax(text, source)?.dst_range;
    target_path_range(text.get(dst.clone())?, &dst, target)
}

pub(crate) fn task_prev_reference(
    text: &str,
    span: &Range<usize>,
    attrs: &Attributes,
) -> Option<Reference> {
    let prev = attrs.get_value("prev")?.to_string();
    let target = parse_task_reference_target(&prev);
    match &target {
        RefTarget::Internal { .. } => {}
        RefTarget::External { path, id: Some(_) } if is_djot_path(path) => {}
        RefTarget::External { .. } | RefTarget::Url(_) => return None,
    }

    let source = attribute_value_range(text, span, "prev", &prev)?;
    let target_path_range = target_path_range(&prev, &source, &target);
    let target_id_range = target_id_range(&prev, &source, &target);
    Some(Reference {
        source,
        target_path_range,
        target_id_range,
        target,
        kind: ReferenceKind::TaskPrev,
    })
}

pub(crate) fn task_dependency_references(
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

pub(crate) fn task_dependencies(
    text: &str,
    span: &Range<usize>,
    attrs: &Attributes,
) -> Vec<TaskDependency> {
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
    parse_task_reference_target(source)
}

fn parse_task_reference_target(source: &str) -> RefTarget {
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

/// Byte range of the path portion of a target value (`path` of `path#id`),
/// given the value text and its source span. Shared by inline links and task
/// references — the `#` split is the djot-undefined `path#anchor` convention,
/// applied to a span the CST already delimited.
fn target_path_range(
    value: &str,
    range: &Range<usize>,
    target: &RefTarget,
) -> Option<Range<usize>> {
    match target {
        RefTarget::External { path, .. } => {
            if path.is_empty() {
                return None;
            }
            match value.find('#') {
                Some(0) => None,
                Some(hash) => Some(range.start..range.start + hash),
                None => Some(range.clone()),
            }
        }
        RefTarget::Internal { .. } | RefTarget::Url(_) => None,
    }
}

/// Byte range of the anchor id portion of a target value (`id` of `path#id` or
/// `#id`), given the value text and its source span.
fn target_id_range(value: &str, range: &Range<usize>, target: &RefTarget) -> Option<Range<usize>> {
    match target {
        RefTarget::Internal { .. } => {
            let start = range.start + value.strip_prefix('#').map_or(0, |_| '#'.len_utf8());
            Some(start..range.end)
        }
        RefTarget::External { id: Some(_), .. } => {
            let hash = value.find('#')?;
            Some(range.start + hash + '#'.len_utf8()..range.end)
        }
        RefTarget::External { id: None, .. } | RefTarget::Url(_) => None,
    }
}

fn dependency_target_path_range(dependency: &TaskDependency) -> Option<Range<usize>> {
    target_path_range(&dependency.source, &dependency.range, &dependency.target)
}

fn dependency_target_id_range(dependency: &TaskDependency) -> Option<Range<usize>> {
    target_id_range(&dependency.source, &dependency.range, &dependency.target)
}

pub(crate) fn is_diagnostic_target(target: &RefTarget) -> bool {
    match target {
        RefTarget::Internal { .. } => true,
        RefTarget::External { path, .. } => is_djot_path(path),
        RefTarget::Url(_) => false,
    }
}

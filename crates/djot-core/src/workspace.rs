use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};

use crate::analysis::{Analysis, Anchor};
use crate::analyze;
use crate::diagnostics::{AnalysisDiagnostic, DiagnosticKind};
use crate::paths::normalize;
use crate::references::{
    is_diagnostic_target, resolve_target, RefTarget, Reference, ReferenceKind,
};
use crate::tasks::{ResolvedTaskDependency, Task, TaskDependency, TaskRef};

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
    pub(crate) docs: HashMap<PathBuf, DocEntry>,
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

    pub fn task_at(&self, path: &Path, offset: usize) -> Option<&Task> {
        self.get(path)?
            .analysis
            .tasks
            .iter()
            .filter(|task| task.range.start <= offset && offset <= task.range.end)
            .max_by_key(|task| task.range.start)
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

fn anchor_targets_task(tasks: &[Task], anchor_range: &Range<usize>) -> bool {
    tasks
        .iter()
        .any(|task| ranges_overlap(anchor_range, &task.range))
}

fn ranges_overlap(a: &Range<usize>, b: &Range<usize>) -> bool {
    a.start < b.end && b.start < a.end
}

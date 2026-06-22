use std::ops::Range;

/// Protocol-agnostic diagnostics produced by djot analysis.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalysisDiagnostic {
    pub range: Range<usize>,
    pub kind: DiagnosticKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiagnosticKind {
    UnresolvedAnchor {
        id: String,
    },
    UnresolvedPath {
        path: String,
    },
    DuplicateAnchor {
        id: String,
        first_range: Range<usize>,
    },
    MissingTaskDueForRecur,
    InvalidTaskRecur {
        recur: String,
    },
    ConflictingTaskClosedState,
    InvalidTaskPrevTarget {
        id: String,
    },
    InvalidTaskDependencyTarget {
        target: String,
    },
    TaskSelfDependency {
        target: String,
    },
    TaskDependencyCycle {
        id: String,
    },
    TaskBlocked {
        count: usize,
    },
}

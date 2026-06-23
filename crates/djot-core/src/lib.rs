//! Protocol-agnostic djot document analysis shared by the language server and
//! (in the future) the exporter.
//!
//! Everything here works in **byte offsets** into the source text. Consumers
//! that need editor coordinates (LSP UTF-16 positions) or a particular AST
//! (pandoc) convert at their own boundary - this crate never depends on those.

mod analysis;
mod diagnostics;
mod edits;
mod paths;
mod references;
mod rename;
mod tasks;
mod workspace;

pub use analysis::{
    analyze, build_index, has_class, heading_outline, metadata_block, metadata_insertion_edit,
    tasks, Analysis, Anchor, DocIndex, Heading,
};
pub use diagnostics::{AnalysisDiagnostic, DiagnosticKind};
pub use edits::{
    apply_text_edits, DocumentTextEdit, EditError, FileRenameEdit, TextEdit, WorkspaceEdit,
};
pub use references::{
    parse_dst, resolve_target, RefTarget, Reference, ReferenceKind, ResolvedTarget,
};
pub use rename::{PathRenameError, PathRenameTarget, RenameTarget, RenameTargetError};
#[cfg(test)]
pub(crate) use tasks::{anchor_attribute, filter_recurring_instance_attributes};
pub use tasks::{
    next_recur_due, parse_repeat_rule, task_done_edits_by_id, task_list_item_conversion_edit,
    task_status_edits_at, RepeatRule, ResolvedTaskDependency, Task, TaskDependency, TaskEditError,
    TaskRef, TaskStatus, TaskStatusEdit,
};
pub use workspace::{DocEntry, Workspace};

/// The class that marks a leading code block as document metadata. This is a
/// djot-ls / djot-export convention layered on djot's native attribute syntax,
/// not part of djot itself - other djot tools simply see a classed code block.
pub const METADATA_CLASS: &str = "metadata";
pub const TASK_CLASS: &str = "task";

#[cfg(test)]
mod tests;

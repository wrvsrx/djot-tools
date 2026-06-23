use lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, CodeActionParams, CodeActionResponse,
};

use crate::edit_context::EditContext;
use djot_core::{
    metadata_insertion_edit, task_list_item_conversion_edit, task_status_edits_at, DocEntry,
    TaskStatus, Workspace,
};

pub(crate) fn resolve_code_actions(
    workspace: &Workspace,
    params: &CodeActionParams,
    path: &std::path::Path,
    entry: &DocEntry,
    offset: usize,
) -> CodeActionResponse {
    let edit_context = EditContext::now();
    let mut actions = Vec::new();

    if requested_code_action_kind_matches(
        params.context.only.as_deref(),
        &CodeActionKind::REFACTOR_REWRITE,
    ) {
        if let Some(insertion) =
            metadata_insertion_edit(&entry.text, offset, path, edit_context.timestamp())
        {
            let edit = EditContext::single_document_workspace_edit(
                params.text_document.uri.clone(),
                &entry.text,
                vec![insertion],
            );
            actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                title: "Add metadata".to_string(),
                kind: Some(CodeActionKind::REFACTOR_REWRITE),
                diagnostics: None,
                edit: Some(edit),
                command: None,
                is_preferred: Some(false),
                disabled: None,
                data: None,
            }));
        }

        if let Some(conversion) =
            task_list_item_conversion_edit(&entry.text, offset, edit_context.timestamp())
        {
            let edit = EditContext::single_document_workspace_edit(
                params.text_document.uri.clone(),
                &entry.text,
                vec![conversion],
            );
            actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                title: "Convert to task div".to_string(),
                kind: Some(CodeActionKind::REFACTOR_REWRITE),
                diagnostics: None,
                edit: Some(edit),
                command: None,
                is_preferred: Some(true),
                disabled: None,
                data: None,
            }));
        }
    }

    if requested_code_action_kind_matches(params.context.only.as_deref(), &CodeActionKind::QUICKFIX)
    {
        let task_is_blocked = entry
            .analysis
            .tasks
            .iter()
            .filter(|task| {
                task.done.is_none()
                    && task.canceled.is_none()
                    && task.range.start <= offset
                    && offset <= task.range.end
            })
            .max_by_key(|task| task.range.start)
            .is_some_and(|task| workspace.is_task_blocked(path, task));
        if !task_is_blocked {
            if let Some(completion) = task_status_edits_at(
                &entry.text,
                offset,
                TaskStatus::Done,
                edit_context.timestamp(),
            ) {
                let edit = EditContext::single_document_workspace_edit(
                    params.text_document.uri.clone(),
                    &entry.text,
                    completion.edits,
                );
                actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                    title: "Complete task".to_string(),
                    kind: Some(CodeActionKind::QUICKFIX),
                    diagnostics: None,
                    edit: Some(edit),
                    command: None,
                    is_preferred: Some(true),
                    disabled: None,
                    data: None,
                }));
            }
        }

        if let Some(cancellation) = task_status_edits_at(
            &entry.text,
            offset,
            TaskStatus::Canceled,
            edit_context.timestamp(),
        ) {
            let edit = EditContext::single_document_workspace_edit(
                params.text_document.uri.clone(),
                &entry.text,
                cancellation.edits,
            );
            actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                title: "Cancel task".to_string(),
                kind: Some(CodeActionKind::QUICKFIX),
                diagnostics: None,
                edit: Some(edit),
                command: None,
                is_preferred: Some(false),
                disabled: None,
                data: None,
            }));
        }
    }

    actions
}

fn requested_code_action_kind_matches(
    only: Option<&[CodeActionKind]>,
    action_kind: &CodeActionKind,
) -> bool {
    let Some(only) = only else {
        return true;
    };
    only.iter()
        .any(|requested| code_action_kind_includes(requested, action_kind))
}

fn code_action_kind_includes(requested: &CodeActionKind, action_kind: &CodeActionKind) -> bool {
    let requested = requested.as_str();
    let action_kind = action_kind.as_str();
    action_kind == requested
        || action_kind
            .strip_prefix(requested)
            .is_some_and(|suffix| suffix.starts_with('.'))
}

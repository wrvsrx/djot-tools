use std::path::Path;

use chrono::{DateTime, FixedOffset, Local, SecondsFormat};
use djot_core::{
    apply_text_edits, task_done_edits_by_id, tasks, EditError, Task, TaskEditError, TaskRef,
    Workspace,
};

use crate::query::{QueryPlan, TaskRecord};
use crate::render::task_table;
use crate::{display_path, is_djot_file, normalize, LoadedDocs, TaskAction};

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct TaskOutputRecord {
    pub(crate) status: String,
    pub(crate) title: String,
    pub(crate) source: String,
}
pub(crate) fn print_tasks(
    root: &Path,
    docs: &LoadedDocs,
    plan: Option<&QueryPlan>,
    tree: bool,
    heading: bool,
) -> Result<(), String> {
    let mut records = Vec::new();
    for path in &docs.paths {
        let Some(text) = docs.texts.get(path) else {
            continue;
        };
        for task in tasks(text) {
            if task_matches(root, &docs.workspace, path, &task, plan)? {
                records.push(task_output_record(root, path, &task, tree));
            }
        }
    }
    if !records.is_empty() {
        println!("{}", task_table(&records, heading));
    }
    Ok(())
}

pub(crate) fn run_task_action(root: &Path, action: &TaskAction) -> Result<(), String> {
    match action {
        TaskAction::Done(config) => {
            let done = Local::now()
                .fixed_offset()
                .to_rfc3339_opts(SecondsFormat::Secs, true);
            for target in &config.targets {
                complete_task_target(root, target, &done)?;
            }
            Ok(())
        }
    }
}

pub(crate) fn complete_task_target(root: &Path, target: &str, done: &str) -> Result<(), String> {
    let task_ref = parse_task_target(root, target)?;
    let text = std::fs::read_to_string(&task_ref.path)
        .map_err(|err| format!("cannot read {}: {err}", task_ref.path.display()))?;
    let edits = task_done_edits_by_id(&text, &task_ref.id, done).map_err(task_edit_error)?;
    let updated = apply_text_edits(text, edits).map_err(edit_error)?;
    std::fs::write(&task_ref.path, updated)
        .map_err(|err| format!("cannot write {}: {err}", task_ref.path.display()))
}

fn task_edit_error(err: TaskEditError) -> String {
    match err {
        TaskEditError::TaskIdNotFound { id } => format!("task id not found: {id}"),
        TaskEditError::TaskAlreadyDone { id } => format!("task is already done: {id}"),
        TaskEditError::TaskCanceled { id } => format!("task is canceled: {id}"),
        TaskEditError::CannotBuildEdit { id } => format!("cannot build done edit for task: {id}"),
    }
}

fn edit_error(err: EditError) -> String {
    match err {
        EditError::OverlappingEdits => "task edits overlap".to_string(),
        EditError::EditRangeOutsideDocument => {
            "task edit range is outside the document".to_string()
        }
    }
}

fn parse_task_target(root: &Path, target: &str) -> Result<TaskRef, String> {
    let (path, id) = target
        .split_once('#')
        .ok_or_else(|| format!("task target must be written as path.dj#task-id: {target}"))?;
    if path.is_empty() || id.is_empty() {
        return Err(format!(
            "task target must include both path and id: {target}"
        ));
    }
    let path = Path::new(path);
    let path = if path.is_absolute() {
        normalize(path)
    } else {
        normalize(&root.join(path))
    };
    if !path.starts_with(root) {
        return Err(format!("task target escapes root: {target}"));
    }
    if !is_djot_file(&path) {
        return Err(format!("task target is not a Djot file: {target}"));
    }
    Ok(TaskRef {
        path,
        id: id.to_string(),
    })
}

pub(crate) fn task_matches(
    root: &Path,
    workspace: &Workspace,
    path: &Path,
    task: &Task,
    plan: Option<&QueryPlan>,
) -> Result<bool, String> {
    let Some(plan) = plan else {
        return Ok(true);
    };
    let depends_on = workspace
        .task_dependencies(path, task)
        .into_iter()
        .map(|dependency| display_task_ref(root, &dependency.target))
        .collect();
    let directly_blocking = task
        .id
        .as_deref()
        .map(|id| {
            workspace
                .directly_blocking_tasks(path, id)
                .into_iter()
                .map(|task_ref| display_task_ref(root, &task_ref))
                .collect()
        })
        .unwrap_or_default();
    let blocked = workspace.is_task_blocked(path, task);
    let actionable = task_is_actionable(task, blocked, plan.now);
    plan.matches_task(TaskRecord {
        root,
        path,
        id: task.id.as_deref(),
        title: &task.title,
        created: task.created.as_deref(),
        done: task.done.as_deref(),
        canceled: task.canceled.as_deref(),
        due: task.due.as_deref(),
        wait: task.wait.as_deref(),
        recur: task.recur.as_deref(),
        prev: task.prev.as_deref(),
        depends_on,
        directly_blocking,
        blocked,
        actionable,
    })
}

fn task_is_actionable(task: &Task, blocked: bool, now: DateTime<FixedOffset>) -> bool {
    task.done.is_none()
        && task.canceled.is_none()
        && !blocked
        && task
            .wait
            .as_deref()
            .and_then(|wait| DateTime::parse_from_rfc3339(wait).ok())
            .is_none_or(|wait| wait <= now)
}

fn display_task_ref(root: &Path, task_ref: &TaskRef) -> String {
    format!("{}#{}", display_path(root, &task_ref.path), task_ref.id)
}

pub(crate) fn task_output_record(
    root: &Path,
    path: &Path,
    task: &Task,
    tree: bool,
) -> TaskOutputRecord {
    let status = if task.canceled.is_some() {
        "x"
    } else if task.done.is_some() {
        "o"
    } else {
        "-"
    };
    let source = match task.id.as_deref() {
        Some(id) => format!("{}#{id}", display_path(root, path)),
        None => display_path(root, path),
    };
    TaskOutputRecord {
        status: status.to_string(),
        title: task_title(task, tree),
        source,
    }
}

fn task_title(task: &Task, tree: bool) -> String {
    if !tree || task.depth == 0 {
        return task.title.clone();
    }
    format!("{}> {}", "  ".repeat(task.depth - 1), task.title)
}

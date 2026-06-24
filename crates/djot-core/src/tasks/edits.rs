use std::collections::{HashMap, HashSet};
use std::ops::Range;

use chrono::{DateTime, FixedOffset, SecondsFormat};
use jotdown::{Container, Event, Parser};

use crate::{analyze, Anchor, TextEdit};

use super::attributes::{
    anchor_attribute, escape_attribute_value, filter_recurring_instance_attributes, leading_indent,
    line_bounds,
};
use super::model::{Task, TaskEditError, TaskStatus, TaskStatusEdit};
use super::recurrence::next_recur_due;

pub fn task_list_item_conversion_edit(
    text: &str,
    offset: usize,
    created: &str,
) -> Option<TextEdit> {
    let analysis = analyze(text);
    let task = analysis
        .native_task_list_items
        .iter()
        .filter(|task| !task.checked && task.range.start <= offset && offset <= task.range.end)
        .max_by_key(|task| task.range.start)?;
    let (line_start, line_end) = line_bounds(text, task.range.start)?;
    let line = text.get(line_start..line_end)?;
    let content = line.strip_suffix('\r').unwrap_or(line);
    let indent = leading_indent(content);
    let rest = &content[indent.len()..];
    let title = rest.strip_prefix("- [ ] ")?.trim();
    if title.is_empty() {
        return None;
    }

    let range_end = trim_trailing_line_ending(text, task.range.end, task.range.start)?;
    let task_indent = format!("{indent}  ");
    let body = native_task_list_item_body(text, line_end, range_end, title, &task_indent)?;

    Some(TextEdit {
        range: line_start..range_end,
        new_text: format!("{indent}- {{created=\"{created}\"}}\n{task_indent}::: task\n{body}"),
    })
}

fn native_task_list_item_body(
    text: &str,
    first_line_end: usize,
    range_end: usize,
    title: &str,
    task_indent: &str,
) -> Option<String> {
    let continuation = text.get(first_line_end..range_end)?;
    let mut body = String::new();
    body.push_str(task_indent);
    body.push_str(title);
    body.push_str(continuation);
    if !body.ends_with('\n') {
        body.push('\n');
    }
    body.push_str(task_indent);
    body.push_str(":::");
    Some(body)
}

fn trim_trailing_line_ending(text: &str, end: usize, min: usize) -> Option<usize> {
    if end > text.len() {
        return None;
    }
    let bytes = text.as_bytes();
    let mut end = end;
    if end > min && bytes.get(end - 1) == Some(&b'\n') {
        end -= 1;
        if end > min && bytes.get(end - 1) == Some(&b'\r') {
            end -= 1;
        }
    }
    Some(end)
}

pub fn task_status_edits_at(
    text: &str,
    offset: usize,
    status: TaskStatus,
    timestamp: &str,
) -> Option<TaskStatusEdit> {
    let analysis = analyze(text);
    let task = analysis
        .tasks
        .iter()
        .filter(|task| {
            task.done.is_none()
                && task.canceled.is_none()
                && task.range.start <= offset
                && offset <= task.range.end
        })
        .max_by_key(|task| task.range.start)?;
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
                "{separator}{list_indent}- {next_id_attribute}\n{indent}{{created=\"{timestamp}\" due=\"{next_due_text}\"{next_wait_attribute} recur=\"{recur}\" prev=\"#{current_id_text}\"}}\n{div}",
                separator = context.separator,
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

struct ListTaskContext<'a> {
    list_indent: &'a str,
    insert: usize,
    separator: &'static str,
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
    let task_end_line = line_bounds(text, task_end_line_offset).map(|(_, end)| end)?;
    if last_nonblank_line_end(text, task_end_line, list_end)? != task_end_line {
        return None;
    }
    if has_indented_content_after(text, list_end, list_indent) {
        return None;
    }
    if count_task_fences(text.get(list_start..list_end)?) != 1 {
        return None;
    }
    let (insert, separator) = if text.as_bytes().get(task_end_line) == Some(&b'\n') {
        (task_end_line + 1, "")
    } else {
        (task_end_line, "\n")
    };

    Some(ListTaskContext {
        list_indent,
        insert,
        separator,
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
            if indent.len() <= list_indent.len() {
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

fn last_nonblank_line_end(text: &str, start: usize, end: usize) -> Option<usize> {
    let mut line_start = start;
    let mut last_nonblank = None;
    while line_start < end {
        let (_, line_end) = line_bounds(text, line_start)?;
        let line = text
            .get(line_start..line_end)?
            .strip_suffix('\r')
            .unwrap_or(text.get(line_start..line_end)?);
        if !line.trim().is_empty() {
            last_nonblank = Some(line_end);
        }
        let Some(next) = next_line_start(text, line_end) else {
            break;
        };
        line_start = next;
    }
    last_nonblank.or(Some(start))
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

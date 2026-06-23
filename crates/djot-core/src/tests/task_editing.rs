use chrono::DateTime;

use crate::*;

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
    let edits = task_done_edits_by_id(text, "write-parser", "2026-06-22T09:00:00+08:00").unwrap();
    let updated = apply_text_edits(text.to_string(), edits).unwrap();

    assert_eq!(
        updated,
        "{#write-parser}\n{done=\"2026-06-22T09:00:00+08:00\"}\n::: task\nWrite parser.\n:::\n"
    );
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

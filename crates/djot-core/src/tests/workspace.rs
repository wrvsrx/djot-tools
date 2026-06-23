use std::path::PathBuf;

use crate::*;

use super::common::workspace_fixture;

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
fn workspace_fixture_covers_diagnostics_and_edit_plans() {
    let fixture = workspace_fixture();
    let ws = fixture.workspace;

    let diagnostics = ws.diagnostics_for(&fixture.index);
    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic.kind
            == DiagnosticKind::UnresolvedPath {
                path: "missing.dj".to_string(),
            }
    }));
    assert!(diagnostics
        .iter()
        .any(|diagnostic| { diagnostic.kind == DiagnosticKind::TaskBlocked { count: 1 } }));

    let mut anchor_edits = ws
        .anchor_rename_edits(&fixture.topic, "topic", "renamed")
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
    anchor_edits.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        anchor_edits,
        vec![
            (
                fixture.index.clone(),
                "topic".to_string(),
                "renamed".to_string()
            ),
            (
                fixture.topic.clone(),
                "topic".to_string(),
                "renamed".to_string()
            ),
        ]
    );

    let plan = ws.path_rename_edit_plan(&fixture.topic, &fixture.renamed);
    assert_eq!(
        plan.first(),
        Some(&WorkspaceEdit::RenameFile(FileRenameEdit {
            old_path: fixture.topic.clone(),
            new_path: fixture.renamed,
        }))
    );
    assert!(plan.iter().any(|edit| match edit {
        WorkspaceEdit::Text(edit) => {
            let text = &ws.get(&edit.path).unwrap().text;
            edit.path == fixture.index
                && &text[edit.edit.range.clone()] == "topic.dj"
                && edit.edit.new_text == "sub/renamed.dj"
        }
        WorkspaceEdit::RenameFile(_) => false,
    }));

    let edits = task_done_edits_by_id(fixture.index_text, "open", "2026-06-19T09:00:00Z").unwrap();
    let updated = apply_text_edits(fixture.index_text.to_string(), edits).unwrap();
    assert!(updated.contains("{done=\"2026-06-19T09:00:00Z\"}"));
}

#[test]
fn larger_workspace_fixture_covers_paths_duplicates_cycles_and_rename_edits() {
    let root = PathBuf::from("/notes");
    let index = root.join("index.dj");
    let project = root.join("Project Plan.dj");
    let nested = root.join("nested/Work File.dj");
    let renamed = root.join("archive/Project Plan.dj");
    let index_text = "# Index\n\n[review](Project Plan.dj#review) [topic](nested/Work File.dj#topic)\n\n{#publish depends=\"Project%20Plan.dj#review\"}\n::: task\nPublish.\n:::\n";
    let project_text = "{#review depends=\"nested/Work%20File.dj#draft\"}\n::: task\nReview.\n:::\n\n{id=\"review\"}\nDuplicate review anchor.\n";
    let nested_text =
        "{#topic}\n# Topic\n\n{#draft depends=\"../Project%20Plan.dj#review\"}\n::: task\nDraft.\n:::\n";
    let mut ws = Workspace::new();
    ws.insert(index.clone(), index_text.to_string());
    ws.insert(project.clone(), project_text.to_string());
    ws.insert(nested.clone(), nested_text.to_string());

    let index_diagnostics = ws.diagnostics_for(&index);
    assert!(index_diagnostics
        .iter()
        .any(|diagnostic| diagnostic.kind == DiagnosticKind::TaskBlocked { count: 1 }));

    let project_diagnostics = ws.diagnostics_for(&project);
    assert!(project_diagnostics.iter().any(|diagnostic| {
        diagnostic.kind
            == DiagnosticKind::DuplicateAnchor {
                id: "review".to_string(),
                first_range: 2..8,
            }
    }));
    assert!(project_diagnostics.iter().any(|diagnostic| {
        diagnostic.kind
            == DiagnosticKind::TaskDependencyCycle {
                id: "review".to_string(),
            }
    }));

    let nested_diagnostics = ws.diagnostics_for(&nested);
    assert!(nested_diagnostics.iter().any(|diagnostic| {
        diagnostic.kind
            == DiagnosticKind::TaskDependencyCycle {
                id: "draft".to_string(),
            }
    }));

    let publish = ws.task_by_id(&index, "publish").unwrap();
    assert_eq!(
        ws.open_task_dependencies(&index, &publish)
            .into_iter()
            .map(|dependency| dependency.target)
            .collect::<Vec<_>>(),
        vec![TaskRef {
            path: project.clone(),
            id: "review".to_string(),
        }]
    );

    let mut anchor_edits = ws
        .anchor_rename_edits(&project, "review", "review-done")
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
    anchor_edits.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    assert_eq!(
        anchor_edits,
        vec![
            (
                project.clone(),
                "review".to_string(),
                "review-done".to_string()
            ),
            (
                index.clone(),
                "review".to_string(),
                "review-done".to_string()
            ),
            (
                index.clone(),
                "review".to_string(),
                "review-done".to_string()
            ),
            (
                nested.clone(),
                "review".to_string(),
                "review-done".to_string()
            ),
        ]
    );

    let plan = ws.path_rename_edit_plan(&project, &renamed);
    assert_eq!(
        plan.first(),
        Some(&WorkspaceEdit::RenameFile(FileRenameEdit {
            old_path: project.clone(),
            new_path: renamed.clone(),
        }))
    );
    let mut path_edits = plan
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
    path_edits.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        path_edits,
        vec![
            (
                index.clone(),
                "Project Plan.dj".to_string(),
                "archive/Project Plan.dj".to_string()
            ),
            (
                index,
                "Project%20Plan.dj".to_string(),
                "archive/Project Plan.dj".to_string()
            ),
            (
                nested,
                "../Project%20Plan.dj".to_string(),
                "../archive/Project Plan.dj".to_string()
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
    let doc = "{#a depends=\"#b\"}\n::: task\nA.\n:::\n\n{#b depends=\"#a\"}\n::: task\nB.\n:::\n";
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

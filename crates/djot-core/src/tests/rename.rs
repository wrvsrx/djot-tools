use std::path::PathBuf;

use crate::*;

#[test]
fn workspace_resolves_rename_target_from_anchor_or_reference() {
    let a = PathBuf::from("/notes/a.dj");
    let b = PathBuf::from("/notes/b.dj");
    let doc_a = "# A\n\nsee [to B](b.dj#topic)\n";
    let doc_b = "{#topic}\nTopic\n";
    let mut ws = Workspace::new();
    ws.insert(a.clone(), doc_a.to_string());
    ws.insert(b.clone(), doc_b.to_string());

    let from_anchor = ws
        .rename_target_at(&b, doc_b.find("topic").unwrap())
        .expect("rename target from anchor");
    assert_eq!(from_anchor.path, b);
    assert_eq!(from_anchor.id, "topic");
    assert_eq!(&doc_b[from_anchor.range], "topic");

    let from_reference = ws
        .rename_target_at(&a, doc_a.find("topic").unwrap())
        .expect("rename target from reference");
    assert_eq!(from_reference.path, PathBuf::from("/notes/b.dj"));
    assert_eq!(from_reference.id, "topic");
    assert_eq!(&doc_a[from_reference.range], "topic");
    assert_eq!(
        ws.rename_target_at(&a, doc_a.find("b.dj").unwrap()),
        Err(RenameTargetError::NotRenameable)
    );
}

#[test]
fn workspace_renames_anchor_only_from_rename_range() {
    let path = PathBuf::from("/notes/tasks.dj");
    let doc = "{#topic}\n::: task\nTask title.\n:::\n\n- {#list-task}\n  ::: task\n  List task title.\n  :::\n";
    let mut ws = Workspace::new();
    ws.insert(path.clone(), doc.to_string());

    let from_anchor = ws
        .rename_target_at(&path, doc.find("topic").unwrap())
        .expect("rename target from explicit anchor");
    assert_eq!(from_anchor.id, "topic");
    assert_eq!(&doc[from_anchor.range], "topic");

    let from_list_anchor = ws
        .rename_target_at(&path, doc.find("list-task").unwrap())
        .expect("rename target from list item anchor");
    assert_eq!(from_list_anchor.id, "list-task");
    assert_eq!(&doc[from_list_anchor.range], "list-task");

    assert_eq!(
        ws.rename_target_at(&path, doc.find("Task title").unwrap()),
        Err(RenameTargetError::NotRenameable)
    );
    assert_eq!(
        ws.rename_target_at(&path, doc.find("List task title").unwrap()),
        Err(RenameTargetError::NotRenameable)
    );
}

#[test]
fn workspace_collects_anchor_rename_edits() {
    let a = PathBuf::from("/notes/a.dj");
    let b = PathBuf::from("/notes/b.dj");
    let doc_a =
        "# A\n\n[local](#A) [other](b.dj#topic) [file](b.dj)\n\n{prev=\"b.dj#topic\"}\n::: task\nNext.\n:::\n";
    let doc_b = "{#topic}\nTopic\n\n[back](../notes/a.dj#A)\n";
    let mut ws = Workspace::new();
    ws.insert(a.clone(), doc_a.to_string());
    ws.insert(b.clone(), doc_b.to_string());

    let mut document_edits = ws
        .anchor_rename_edits(&b, "topic", "renamed")
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
    document_edits.sort_by(|a, b| a.0.cmp(&b.0));

    assert_eq!(
        document_edits,
        vec![
            (
                PathBuf::from("/notes/a.dj"),
                "topic".to_string(),
                "renamed".to_string()
            ),
            (
                PathBuf::from("/notes/a.dj"),
                "topic".to_string(),
                "renamed".to_string()
            ),
            (
                PathBuf::from("/notes/b.dj"),
                "topic".to_string(),
                "renamed".to_string()
            )
        ]
    );
}

#[test]
fn workspace_rejects_rename_for_implicit_heading_anchor() {
    let a = PathBuf::from("/notes/a.dj");
    let b = PathBuf::from("/notes/b.dj");
    let doc_a = "# A\n\nsee [to B](b.dj#Topic)\n";
    let doc_b = "# Topic\n";
    let mut ws = Workspace::new();
    ws.insert(a.clone(), doc_a.to_string());
    ws.insert(b.clone(), doc_b.to_string());

    assert_eq!(
        ws.rename_target_at(&b, doc_b.find("Topic").unwrap()),
        Err(RenameTargetError::ImplicitHeadingAnchor)
    );
    assert_eq!(
        ws.rename_target_at(&a, doc_a.find("Topic").unwrap()),
        Err(RenameTargetError::ImplicitHeadingAnchor)
    );
    assert!(ws.anchor_rename_edits(&b, "Topic", "Renamed").is_empty());
}

#[test]
fn workspace_resolves_path_rename_target_from_link_path() {
    let a = PathBuf::from("/notes/a.dj");
    let b = PathBuf::from("/notes/b.dj");
    let doc_a = "# A\n\nsee [to B](b.dj#topic)\n";
    let mut ws = Workspace::new();
    ws.insert(a.clone(), doc_a.to_string());
    ws.insert(b.clone(), "{#topic}\nTopic\n".to_string());

    let target = ws
        .path_rename_target_at(&a, doc_a.find("b.dj").unwrap())
        .expect("path rename target");

    assert_eq!(target.old_path, b);
    assert_eq!(&doc_a[target.range], "b.dj");
    assert_eq!(
        ws.path_rename_target_at(&a, doc_a.find("topic").unwrap()),
        Err(PathRenameError::NotRenameable)
    );
}

#[test]
fn workspace_collects_path_rename_edit_plan_with_relative_replacements() {
    let a = PathBuf::from("/notes/a.dj");
    let b = PathBuf::from("/notes/b.dj");
    let c = PathBuf::from("/notes/sub/c.dj");
    let renamed = PathBuf::from("/notes/renamed.dj");
    let doc_a = "# A\n\n[topic](b.dj#topic)\n";
    let doc_c = "# C\n\n[topic](../b.dj)\n";
    let mut ws = Workspace::new();
    ws.insert(a.clone(), doc_a.to_string());
    ws.insert(b.clone(), "{#topic}\nTopic\n".to_string());
    ws.insert(c.clone(), doc_c.to_string());

    let plan = ws.path_rename_edit_plan(&b, &renamed);
    assert_eq!(
        plan.first(),
        Some(&WorkspaceEdit::RenameFile(FileRenameEdit {
            old_path: b,
            new_path: renamed,
        }))
    );

    let mut text_edits = plan
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
    text_edits.sort_by(|a, b| a.0.cmp(&b.0));

    assert_eq!(
        text_edits,
        vec![
            (
                PathBuf::from("/notes/a.dj"),
                "b.dj".to_string(),
                "renamed.dj".to_string()
            ),
            (
                PathBuf::from("/notes/sub/c.dj"),
                "../b.dj".to_string(),
                "../renamed.dj".to_string()
            ),
        ]
    );
}

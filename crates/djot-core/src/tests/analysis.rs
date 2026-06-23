use std::path::Path;

use jotdown::{Container, Event, Parser};

use crate::*;

#[test]
fn outline_nests_by_section_level() {
    let text = "# A\n\ntext\n\n## B\n\n### C\n\n# D\n";
    let roots = heading_outline(text);
    assert_eq!(
        roots.iter().map(|h| h.name.as_str()).collect::<Vec<_>>(),
        ["A", "D"]
    );
    let a = &roots[0];
    assert_eq!(a.level, 1);
    assert_eq!(
        a.children
            .iter()
            .map(|h| h.name.as_str())
            .collect::<Vec<_>>(),
        ["B"]
    );
    assert_eq!(a.children[0].children[0].name, "C");
    // Parent section range encloses its children.
    assert!(a.range.end >= a.children[0].range.end);
}

#[test]
fn index_collects_anchors_and_references() {
    let text = "# My Heading\n\n[a](#My-Heading) [b][] [u](https://x.y) [f](o.dj#s)\n\n## b\n";
    let index = build_index(text);
    assert!(index.anchors.contains_key("My-Heading"));
    assert!(index.anchors.contains_key("b"));

    let targets: Vec<_> = index.references.iter().map(|r| &r.target).collect();
    assert!(targets.contains(&&RefTarget::Internal {
        id: "My-Heading".into()
    }));
    assert!(targets.contains(&&RefTarget::Url("https://x.y".into())));
    assert!(targets.contains(&&RefTarget::External {
        path: "o.dj".into(),
        id: Some("s".into()),
    }));
}

#[test]
fn analysis_collects_shared_document_semantics() {
    let text = "{.metadata}\n``` toml\ntitle = \"x\"\n```\n\n# Topic\n\n{#task-a recur=\"P1Q\"}\n::: task\nTask A.\n:::\n\n[topic](#Topic)\n";
    let analysis = analyze(text);

    assert_eq!(analysis.metadata.as_deref(), Some("title = \"x\"\n"));
    assert!(analysis.index.anchors.contains_key("Topic"));
    assert_eq!(analysis.index.references.len(), 1);
    assert_eq!(analysis.tasks.len(), 1);
    assert_eq!(analysis.tasks[0].id.as_deref(), Some("task-a"));
    assert!(analysis.diagnostics.iter().any(|diagnostic| {
        diagnostic.kind
            == DiagnosticKind::InvalidTaskRecur {
                recur: "P1Q".into(),
            }
    }));
    assert!(analysis
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.kind == DiagnosticKind::MissingTaskDueForRecur));
}

#[test]
fn index_tracks_anchor_rename_ranges() {
    let text = "# My Heading\n\n{#custom}\nparagraph\n\n{prev=\"#quoted\" id=\"quoted\"}\nquoted paragraph\n\n{id=bare}\nbare paragraph\n\n{id=\"学习-anki\"}\nunicode paragraph\n";
    let index = build_index(text);

    let heading = &index.anchors["My-Heading"];
    assert_eq!(&text[heading.rename_range.clone()], "My Heading");
    assert!(!heading.explicit);

    let explicit = &index.anchors["custom"];
    assert_eq!(&text[explicit.rename_range.clone()], "custom");
    assert!(explicit.explicit);

    let quoted = &index.anchors["quoted"];
    assert_eq!(&text[quoted.rename_range.clone()], "quoted");
    assert!(quoted.explicit);

    let bare = &index.anchors["bare"];
    assert_eq!(&text[bare.rename_range.clone()], "bare");
    assert!(bare.explicit);

    let unicode = &index.anchors["学习-anki"];
    assert_eq!(&text[unicode.rename_range.clone()], "学习-anki");
    assert!(unicode.explicit);
}

#[test]
fn index_tracks_reference_target_id_ranges() {
    let text = "[internal](#Topic) [external](other.dj#Section) [file](other.dj) [implicit][]";
    let index = build_index(text);

    let ranges = index
        .references
        .iter()
        .filter_map(|reference| {
            reference
                .target_id_range
                .clone()
                .map(|range| text[range].to_string())
        })
        .collect::<Vec<_>>();

    assert_eq!(ranges, ["Topic", "Section"]);
}

#[test]
fn index_tracks_reference_target_path_ranges() {
    let text = "[internal](#Topic) [external](other.dj#Section) [file](notes/other.dj) [url](https://example.com)";
    let index = build_index(text);

    let ranges = index
        .references
        .iter()
        .filter_map(|reference| {
            reference
                .target_path_range
                .clone()
                .map(|range| text[range].to_string())
        })
        .collect::<Vec<_>>();

    assert_eq!(ranges, ["other.dj", "notes/other.dj"]);
}

#[test]
fn index_tracks_task_prev_references() {
    let text = "{prev=\"#old-task\"}\n::: task\nNext task.\n:::\n\n{prev=\"other.dj#previous\"}\n::: task\nCross-file next task.\n:::\n\n{prev=\"other.dj\"}\n::: task\nFile-only prev is not a reference.\n:::\n";
    let index = build_index(text);

    let refs = index
        .references
        .iter()
        .map(|reference| {
            (
                text[reference.source.clone()].to_string(),
                reference
                    .target_path_range
                    .clone()
                    .map(|range| text[range].to_string()),
                reference
                    .target_id_range
                    .clone()
                    .map(|range| text[range].to_string()),
                reference.target.clone(),
                reference.kind,
            )
        })
        .collect::<Vec<_>>();

    assert_eq!(
        refs,
        vec![
            (
                "#old-task".to_string(),
                None,
                Some("old-task".to_string()),
                RefTarget::Internal {
                    id: "old-task".to_string()
                },
                ReferenceKind::TaskPrev,
            ),
            (
                "other.dj#previous".to_string(),
                Some("other.dj".to_string()),
                Some("previous".to_string()),
                RefTarget::External {
                    path: "other.dj".to_string(),
                    id: Some("previous".to_string()),
                },
                ReferenceKind::TaskPrev,
            ),
        ]
    );
}

#[test]
fn metadata_block_extracts_leading_toml() {
    let text = "{.metadata}\n``` toml\ntitle = \"x\"\n```\n\n# H\n";
    assert_eq!(metadata_block(text).as_deref(), Some("title = \"x\"\n"));
    // A plain code block is not metadata.
    assert_eq!(metadata_block("``` toml\ntitle = \"x\"\n```\n"), None);
}

#[test]
fn metadata_insertion_edit_adds_leading_metadata_block() {
    let text = "\n\n# Heading\n";
    let edit = metadata_insertion_edit(
        text,
        1,
        Path::new("/notes/my \"note\".dj"),
        "2026-06-22T09:00:00+08:00",
    )
    .unwrap();

    assert_eq!(edit.range, 0..0);
    assert_eq!(
        edit.new_text,
        "{.metadata}\n``` toml\ntitle = \"my \\\"note\\\"\"\ncreated = \"2026-06-22T09:00:00+08:00\"\n```\n\n"
    );
    assert!(metadata_insertion_edit("# Heading\n", 2, Path::new("x.dj"), "now").is_none());
}

#[test]
fn parse_dst_classifies_destinations() {
    assert_eq!(parse_dst("#sec"), RefTarget::Internal { id: "sec".into() });
    assert_eq!(
        parse_dst("mailto:a@b.c"),
        RefTarget::Url("mailto:a@b.c".into())
    );
    assert_eq!(
        parse_dst("other.dj"),
        RefTarget::External {
            path: "other.dj".into(),
            id: None
        }
    );
}

#[test]
fn jotdown_cursor_link_parsing_shapes() {
    for (marked, expected_str) in [
        ("[|", Some("[")),
        ("[foo|", Some("[foo")),
        ("[foo|]", Some("[foo]")),
        ("[foo](|", Some("[foo](")),
        ("[foo](|)", None),
        ("[|]", Some("[]")),
    ] {
        let (text, cursor) = strip_cursor_marker(marked);
        assert_eq!(
            str_event_touching_cursor(&text, cursor).as_deref(),
            expected_str,
            "unexpected Str event at cursor for {marked:?}"
        );
    }

    let (text, cursor) = strip_cursor_marker("[foo](|)");
    assert!(
        Parser::new(&text).into_offset_iter().any(|(event, span)| {
            span.start <= cursor
                && cursor <= span.end
                && matches!(event, Event::End(Container::Link(_, _)))
        }),
        "cursor in a complete empty destination is in the link end syntax span"
    );
}

fn strip_cursor_marker(marked: &str) -> (String, usize) {
    let cursor = marked.find('|').expect("cursor marker");
    (marked.replace('|', ""), cursor)
}

fn str_event_touching_cursor(text: &str, cursor: usize) -> Option<String> {
    Parser::new(text)
        .into_offset_iter()
        .find_map(|(event, span)| match event {
            Event::Str(s) if span.start <= cursor && cursor <= span.end => Some(s.to_string()),
            _ => None,
        })
}

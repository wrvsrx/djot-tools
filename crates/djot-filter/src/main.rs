use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;

use chrono::DateTime;
use clap::{Args, Parser, Subcommand};
use djot_core::Workspace;

mod interactive;
mod query;
mod render;
#[path = "tasks.rs"]
mod task_ops;

pub(crate) use interactive::{
    create_file_from_query, editor_command, editor_paths, handle_interactive_action,
    highlight_djot_preview, run_interactive, FilterItem,
};
pub(crate) use query::{retain_query_matches, QueryPlan};
pub(crate) use render::print_paths;
pub(crate) use task_ops::{
    complete_task_target, print_tasks, run_task_action, task_matches, task_output_record,
    TaskOutputRecord,
};

fn main() -> ExitCode {
    let config = Config::parse();

    let root = default_root(&config);

    match &config.command {
        CommandMode::Note(note) => {
            let docs = match load_docs(&root) {
                Ok(docs) => docs,
                Err(err) => {
                    eprintln!("djot-filter: {err}");
                    return ExitCode::FAILURE;
                }
            };
            let mut paths = docs.paths.clone();
            if let Some(query) = &config.query {
                let plan = match QueryPlan::compile(query) {
                    Ok(plan) => plan,
                    Err(err) => {
                        eprintln!("djot-filter: {err}");
                        return ExitCode::FAILURE;
                    }
                };
                match retain_query_matches(&root, &docs, &mut paths, &plan) {
                    Ok(()) => {}
                    Err(err) => {
                        eprintln!("djot-filter: {err}");
                        return ExitCode::FAILURE;
                    }
                }
            }

            paths.sort();
            if note.interactive {
                match run_interactive(&root, &paths, &docs.texts) {
                    Ok(action) => {
                        if let Err(err) = handle_interactive_action(&root, action) {
                            eprintln!("djot-filter: {err}");
                            return ExitCode::FAILURE;
                        }
                    }
                    Err(err) => {
                        eprintln!("djot-filter: {err}");
                        return ExitCode::FAILURE;
                    }
                }
            } else {
                print_paths(paths.into_iter().map(|path| display_path(&root, &path)));
            }
        }
        CommandMode::Task(task) => {
            if let Some(action) = &task.action {
                if config.query.is_some() {
                    eprintln!("djot-filter: task actions do not support --query yet; pass explicit TARGET values");
                    return ExitCode::FAILURE;
                }
                if let Err(err) = run_task_action(&root, action) {
                    eprintln!("djot-filter: {err}");
                    return ExitCode::FAILURE;
                }
                return ExitCode::SUCCESS;
            }

            let docs = match load_docs(&root) {
                Ok(docs) => docs,
                Err(err) => {
                    eprintln!("djot-filter: {err}");
                    return ExitCode::FAILURE;
                }
            };
            let plan = match config.query.as_deref() {
                Some(query) => match QueryPlan::compile(query) {
                    Ok(plan) => Some(plan),
                    Err(err) => {
                        eprintln!("djot-filter: {err}");
                        return ExitCode::FAILURE;
                    }
                },
                None => None,
            };
            if let Err(err) = print_tasks(&root, &docs, plan.as_ref(), !task.flat) {
                eprintln!("djot-filter: {err}");
                return ExitCode::FAILURE;
            }
        }
    }

    ExitCode::SUCCESS
}

#[derive(Debug, Parser)]
#[command(
    name = "djot-filter",
    about = "Filter .dj/.djot files under a directory"
)]
struct Config {
    /// Directory to scan recursively. Defaults to the current directory.
    #[arg(long, global = true, value_name = "DIR")]
    root: Option<PathBuf>,

    /// Keep records whose CEL predicate evaluates to true.
    #[arg(long, global = true, value_name = "EXPR")]
    query: Option<String>,

    #[command(subcommand)]
    command: CommandMode,
}

#[derive(Debug, Subcommand)]
enum CommandMode {
    /// Filter note files under the scanned directory.
    Note(NoteConfig),

    /// Print tasks found in scanned Djot files.
    Task(TaskConfig),
}

#[derive(Debug, Args)]
struct NoteConfig {
    /// Re-filter results interactively with skim.
    #[arg(short, long)]
    interactive: bool,
}

#[derive(Debug, Args)]
struct TaskConfig {
    /// Print task titles without nested task tree markers.
    #[arg(long)]
    flat: bool,

    #[command(subcommand)]
    action: Option<TaskAction>,
}

#[derive(Debug, Subcommand)]
enum TaskAction {
    /// Mark task targets done. Recurring tasks advance to the next instance by default.
    Done(TaskDoneConfig),
}

#[derive(Debug, Args)]
struct TaskDoneConfig {
    /// Task targets to complete, written as path.dj#task-id.
    #[arg(value_name = "TARGET", required = true)]
    targets: Vec<String>,
}

pub(crate) struct LoadedDocs {
    workspace: Workspace,
    paths: Vec<PathBuf>,
    texts: HashMap<PathBuf, String>,
}
pub(crate) fn load_docs(root: &Path) -> Result<LoadedDocs, String> {
    let mut workspace = Workspace::new();
    let mut paths = Vec::new();
    let mut texts = HashMap::new();

    for path in djot_files(root)? {
        let text = std::fs::read_to_string(&path)
            .map_err(|err| format!("cannot read {}: {err}", path.display()))?;
        let path = normalize(&path);
        workspace.insert(path.clone(), text.clone());
        texts.insert(path.clone(), text);
        paths.push(path);
    }

    Ok(LoadedDocs {
        workspace,
        paths,
        texts,
    })
}

fn djot_files(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut out = Vec::new();
    collect_djot_files(root, &mut out)?;
    out.sort();
    Ok(out)
}

fn collect_djot_files(path: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = std::fs::read_dir(path)
        .map_err(|err| format!("cannot read directory {}: {err}", path.display()))?;
    for entry in entries {
        let entry = entry.map_err(|err| format!("cannot read directory entry: {err}"))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|err| format!("cannot stat {}: {err}", path.display()))?;
        if file_type.is_dir() {
            collect_djot_files(&path, out)?;
        } else if file_type.is_file() && is_djot_file(&path) {
            out.push(path);
        }
    }
    Ok(())
}
pub(crate) fn absolute_path(path: &Path) -> PathBuf {
    if path.is_absolute() {
        normalize(path)
    } else {
        normalize(
            &std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(path),
        )
    }
}

fn default_root(config: &Config) -> PathBuf {
    absolute_path(config.root.as_deref().unwrap_or_else(|| Path::new(".")))
}

pub(crate) fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

pub(crate) fn is_djot_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext == "dj" || ext == "djot")
}

pub(crate) fn display_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use djot_core::tasks;
    use skim::SkimItem;

    #[test]
    fn root_defaults_to_current_directory() {
        let config = Config {
            root: None,
            query: None,
            command: CommandMode::Note(NoteConfig { interactive: false }),
        };
        assert_eq!(default_root(&config), absolute_path(Path::new(".")));
    }

    #[test]
    fn note_subcommand_accepts_root_query_and_interactive_after_subcommand() {
        let config = Config::parse_from([
            "djot-filter",
            "note",
            "--root",
            "notes",
            "--query",
            "title.matches('topic')",
            "--interactive",
        ]);

        assert!(matches!(
            config.command,
            CommandMode::Note(NoteConfig { interactive: true })
        ));
        assert_eq!(config.root.as_deref(), Some(Path::new("notes")));
        assert_eq!(config.query.as_deref(), Some("title.matches('topic')"));
    }

    #[test]
    fn task_subcommand_accepts_root_and_query_after_subcommand() {
        let config = Config::parse_from([
            "djot-filter",
            "task",
            "--root",
            "notes",
            "--query",
            "done == null",
        ]);

        assert!(matches!(
            config.command,
            CommandMode::Task(TaskConfig {
                flat: false,
                action: None,
            })
        ));
        assert_eq!(config.root.as_deref(), Some(Path::new("notes")));
        assert_eq!(config.query.as_deref(), Some("done == null"));
    }

    #[test]
    fn task_subcommand_accepts_flat_flag() {
        let config = Config::parse_from(["djot-filter", "task", "--flat"]);

        assert!(matches!(
            config.command,
            CommandMode::Task(TaskConfig {
                flat: true,
                action: None,
            })
        ));
    }

    #[test]
    fn task_done_subcommand_accepts_targets() {
        let config = Config::parse_from(["djot-filter", "task", "done", "tasks.dj#write-parser"]);

        let CommandMode::Task(TaskConfig {
            action: Some(TaskAction::Done(done)),
            ..
        }) = config.command
        else {
            panic!("expected task done config");
        };
        assert_eq!(done.targets, vec!["tasks.dj#write-parser"]);
    }

    #[test]
    fn default_notes_command_is_removed() {
        let err = Config::try_parse_from(["djot-filter", "--query", "true"]).unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::MissingSubcommand);
    }

    #[test]
    fn plural_subcommands_are_rejected() {
        let notes_err = Config::try_parse_from(["djot-filter", "notes"]).unwrap_err();
        assert_eq!(notes_err.kind(), clap::error::ErrorKind::InvalidSubcommand);

        let tasks_err = Config::try_parse_from(["djot-filter", "tasks"]).unwrap_err();
        assert_eq!(tasks_err.kind(), clap::error::ErrorKind::InvalidSubcommand);
    }

    #[test]
    fn cel_query_matches_path_and_title() {
        let root = unique_test_dir("djot-filter-query-title-test");
        std::fs::create_dir_all(root.join("docs")).unwrap();
        std::fs::write(
            root.join("docs/semantics.dj"),
            "{.metadata}\n``` toml\ntitle = \"Semantics Guide\"\n```\n\n# H\n",
        )
        .unwrap();
        std::fs::write(root.join("notes.dj"), "# Notes\n").unwrap();

        let docs = load_docs(&root).unwrap();
        let mut paths = docs.paths.clone();
        let plan =
            QueryPlan::compile("path.startsWith('docs/') && title.matches('Semantics')").unwrap();
        retain_query_matches(&root, &docs, &mut paths, &plan).unwrap();

        assert_eq!(paths, vec![normalize(&root.join("docs/semantics.dj"))]);

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn cel_query_matches_direct_and_transitive_reverse_references() {
        let root = unique_test_dir("djot-filter-query-reference-test");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("index.dj"), "[topic](topic.dj)\n").unwrap();
        std::fs::write(root.join("topic.dj"), "[leaf](leaf.dj)\n").unwrap();
        std::fs::write(root.join("leaf.dj"), "# Leaf\n").unwrap();

        let docs = load_docs(&root).unwrap();

        let mut direct_paths = docs.paths.clone();
        let direct = QueryPlan::compile("'index.dj' in directly_referenced_by").unwrap();
        retain_query_matches(&root, &docs, &mut direct_paths, &direct).unwrap();
        direct_paths.sort();
        assert_eq!(direct_paths, vec![normalize(&root.join("topic.dj"))]);

        let mut transitive_paths = docs.paths.clone();
        let transitive = QueryPlan::compile("'index.dj' in transitively_referenced_by").unwrap();
        retain_query_matches(&root, &docs, &mut transitive_paths, &transitive).unwrap();
        transitive_paths.sort();
        assert_eq!(
            transitive_paths,
            vec![
                normalize(&root.join("leaf.dj")),
                normalize(&root.join("topic.dj")),
            ]
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn task_query_matches_title_created_and_done() {
        let root = unique_test_dir("djot-filter-task-query-test");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("tasks.dj"),
            "{#open-task}\n{created=\"2026-06-18T09:00:00+08:00\" due=\"2026-06-20T09:00:00+08:00\" wait=\"2026-06-19T09:00:00+08:00\" recur=\"P1W\" prev=\"#previous-task\"}\n::: task\nOpen task\n:::\n\n{created=\"2026-06-19T09:00:00+08:00\" done=\"2026-06-19T21:30:00+08:00\"}\n::: task\nDone task\n:::\n\n{created=\"2026-06-20T09:00:00+08:00\" canceled=\"2026-06-20T21:30:00+08:00\" wait=\"2026-06-23T09:00:00+08:00\"}\n::: task\nCanceled task\n:::\n",
        )
        .unwrap();

        let docs = load_docs(&root).unwrap();
        let path = normalize(&root.join("tasks.dj"));
        let text = docs.texts.get(&path).unwrap();
        let found = tasks(text);
        let open = QueryPlan::compile("done == null").unwrap();
        let created =
            QueryPlan::compile("created == timestamp('2026-06-18T09:00:00+08:00')").unwrap();
        let done = QueryPlan::compile("done != null && title.matches('Done')").unwrap();
        let canceled = QueryPlan::compile("canceled != null && title.matches('Canceled')").unwrap();
        let recurring = QueryPlan::compile(
            "due == timestamp('2026-06-20T09:00:00+08:00') && wait == timestamp('2026-06-19T09:00:00+08:00') && recur == 'P1W' && prev == '#previous-task'",
        )
        .unwrap();
        let actionable = QueryPlan::compile_at(
            "done == null && canceled == null && (wait == null || wait <= now)",
            DateTime::parse_from_rfc3339("2026-06-19T10:00:00+08:00").unwrap(),
        )
        .unwrap();
        let waiting = QueryPlan::compile_at(
            "done == null && canceled == null && wait != null && wait > now",
            DateTime::parse_from_rfc3339("2026-06-18T10:00:00+08:00").unwrap(),
        )
        .unwrap();
        let source = QueryPlan::compile("path == 'tasks.dj' && id == 'open-task'").unwrap();

        assert!(task_matches(&root, &docs.workspace, &path, &found[0], Some(&open)).unwrap());
        assert!(!task_matches(&root, &docs.workspace, &path, &found[1], Some(&open)).unwrap());
        assert!(task_matches(&root, &docs.workspace, &path, &found[0], Some(&created)).unwrap());
        assert!(!task_matches(&root, &docs.workspace, &path, &found[1], Some(&created)).unwrap());
        assert!(task_matches(&root, &docs.workspace, &path, &found[1], Some(&done)).unwrap());
        assert!(task_matches(&root, &docs.workspace, &path, &found[2], Some(&canceled)).unwrap());
        assert!(task_matches(&root, &docs.workspace, &path, &found[0], Some(&recurring)).unwrap());
        assert!(!task_matches(&root, &docs.workspace, &path, &found[1], Some(&recurring)).unwrap());
        assert!(task_matches(&root, &docs.workspace, &path, &found[0], Some(&actionable)).unwrap());
        assert!(
            !task_matches(&root, &docs.workspace, &path, &found[1], Some(&actionable)).unwrap()
        );
        assert!(task_matches(&root, &docs.workspace, &path, &found[0], Some(&waiting)).unwrap());
        assert!(!task_matches(&root, &docs.workspace, &path, &found[1], Some(&waiting)).unwrap());
        assert!(task_matches(&root, &docs.workspace, &path, &found[0], Some(&source)).unwrap());
        assert!(!task_matches(&root, &docs.workspace, &path, &found[1], Some(&source)).unwrap());
        let open_row = task_output_record(&root, &path, &found[0], false);
        let done_row = task_output_record(&root, &path, &found[1], false);
        let canceled_row = task_output_record(&root, &path, &found[2], false);
        assert_eq!(
            open_row,
            TaskOutputRecord {
                status: "-".to_string(),
                title: "Open task".to_string(),
                source: "tasks.dj#open-task".to_string(),
            }
        );
        assert_eq!(
            done_row,
            TaskOutputRecord {
                status: "o".to_string(),
                title: "Done task".to_string(),
                source: "tasks.dj".to_string(),
            }
        );
        assert_eq!(
            canceled_row,
            TaskOutputRecord {
                status: "x".to_string(),
                title: "Canceled task".to_string(),
                source: "tasks.dj".to_string(),
            }
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn task_output_prefixes_nested_titles_by_default() {
        let root = unique_test_dir("djot-filter-task-tree-test");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("tasks.dj"),
            "::: task\nParent\n\n::: task\nChild\n\n::: task\nGrandchild\n:::\n:::\n:::\n",
        )
        .unwrap();

        let docs = load_docs(&root).unwrap();
        let path = normalize(&root.join("tasks.dj"));
        let text = docs.texts.get(&path).unwrap();
        let found = tasks(text);

        assert_eq!(
            found
                .iter()
                .map(|task| task_output_record(&root, &path, task, true).title)
                .collect::<Vec<_>>(),
            vec!["Parent", "> Child", "  > Grandchild"]
        );
        assert_eq!(
            found
                .iter()
                .map(|task| task_output_record(&root, &path, task, false).title)
                .collect::<Vec<_>>(),
            vec!["Parent", "Child", "Grandchild"]
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn task_done_target_marks_task_done() {
        let root = unique_test_dir("djot-filter-task-done-test");
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("tasks.dj");
        std::fs::write(&path, "{#write-parser}\n::: task\nWrite parser.\n:::\n").unwrap();

        complete_task_target(&root, "tasks.dj#write-parser", "2026-06-22T09:00:00+08:00").unwrap();

        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "{#write-parser}\n{done=\"2026-06-22T09:00:00+08:00\"}\n::: task\nWrite parser.\n:::\n"
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn task_done_target_advances_recurring_task() {
        let root = unique_test_dir("djot-filter-task-done-recurring-test");
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("tasks.dj");
        std::fs::write(
            &path,
            "{#weekly-review}\n{project=\"ops\" due=\"2026-06-21T17:00:00+08:00\" wait=\"2026-06-21T09:00:00+08:00\" recur=\"P1W\"}\n::: task\nWeekly review.\n:::\n",
        )
        .unwrap();

        complete_task_target(&root, "tasks.dj#weekly-review", "2026-06-22T09:00:00+08:00").unwrap();

        let updated = std::fs::read_to_string(&path).unwrap();
        assert!(updated.contains("{done=\"2026-06-22T09:00:00+08:00\"}\n::: task"));
        assert!(updated.contains("{#Weekly-review-2026-06-28}\n"));
        assert!(updated.contains("{project=\"ops\"}\n"));
        assert!(updated.contains(
            "{created=\"2026-06-22T09:00:00+08:00\" due=\"2026-06-28T17:00:00+08:00\" wait=\"2026-06-28T09:00:00+08:00\" recur=\"P1W\" prev=\"#weekly-review\"}"
        ));
        assert_eq!(updated.matches("::: task").count(), 2);

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn task_query_matches_dependency_fields() {
        let root = unique_test_dir("djot-filter-task-dependency-test");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("tasks.dj"),
            "{#draft}\n::: task\nDraft.\n:::\n\n{#done done=\"2026-06-21T09:00:00+08:00\"}\n::: task\nDone.\n:::\n\n{#review depends=\"#draft #done\"}\n::: task\nReview.\n:::\n\n{#waiting wait=\"2026-06-23T09:00:00+08:00\"}\n::: task\nWaiting.\n:::\n",
        )
        .unwrap();

        let docs = load_docs(&root).unwrap();
        let path = normalize(&root.join("tasks.dj"));
        let text = docs.texts.get(&path).unwrap();
        let found = tasks(text);
        let depends_on =
            QueryPlan::compile("'tasks.dj#draft' in depends_on && 'tasks.dj#done' in depends_on")
                .unwrap();
        let directly_blocking =
            QueryPlan::compile("'tasks.dj#review' in directly_blocking").unwrap();
        let blocked = QueryPlan::compile("blocked").unwrap();
        let actionable = QueryPlan::compile_at(
            "actionable",
            DateTime::parse_from_rfc3339("2026-06-22T09:00:00+08:00").unwrap(),
        )
        .unwrap();

        let draft = found
            .iter()
            .find(|task| task.id.as_deref() == Some("draft"))
            .unwrap();
        let review = found
            .iter()
            .find(|task| task.id.as_deref() == Some("review"))
            .unwrap();
        let waiting = found
            .iter()
            .find(|task| task.id.as_deref() == Some("waiting"))
            .unwrap();

        assert!(task_matches(&root, &docs.workspace, &path, review, Some(&depends_on)).unwrap());
        assert!(task_matches(
            &root,
            &docs.workspace,
            &path,
            draft,
            Some(&directly_blocking)
        )
        .unwrap());
        assert!(task_matches(&root, &docs.workspace, &path, review, Some(&blocked)).unwrap());
        assert!(!task_matches(&root, &docs.workspace, &path, review, Some(&actionable)).unwrap());
        assert!(task_matches(&root, &docs.workspace, &path, draft, Some(&actionable)).unwrap());
        assert!(!task_matches(&root, &docs.workspace, &path, waiting, Some(&actionable)).unwrap());

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn filter_item_searches_full_text_but_outputs_path() {
        let item = FilterItem::new(
            "notes/topic.dj".to_string(),
            "# Topic\nbody text\n".to_string(),
        );

        assert_eq!(item.output(), "notes/topic.dj");
        assert!(item.text().contains("notes/topic.dj"));
        assert!(item.text().contains("body text"));

        let preview = FilterItem::preview_text("notes/topic.dj", "# Topic\nbody text\n");
        assert!(preview.contains("notes/topic.dj"));
        assert!(preview.contains("# Topic"));
        assert!(preview.contains("\x1b["));
    }

    #[test]
    fn preview_highlights_djot_lines_with_ansi() {
        let preview = highlight_djot_preview(
            "{.metadata}\n``` toml\ntitle = \"T\"\n```\n# Heading\nSee [next](next.dj)\n",
        );

        assert!(preview.contains("\x1b[35m{.metadata}\x1b[0m"));
        assert!(preview.contains("\x1b[36m``` toml\x1b[0m"));
        assert!(preview.contains("\x1b[1;34m# Heading\x1b[0m"));
        assert!(preview.contains("\x1b[32m[next](next.dj)\x1b[0m"));
    }

    #[test]
    fn editor_command_supports_arguments() {
        let (program, args) = editor_command("nvim -p").unwrap();
        assert_eq!(program, "nvim");
        assert_eq!(args, vec!["-p"]);

        let (program, args) = editor_command("'code editor' --wait").unwrap();
        assert_eq!(program, "code editor");
        assert_eq!(args, vec!["--wait"]);
    }

    #[test]
    fn editor_paths_are_root_relative_and_keep_spaces() {
        let root = normalize(Path::new("/tmp/djot-filter-root"));
        let paths = editor_paths(&root, &["other file.dj".to_string()]);
        assert_eq!(paths, vec![root.join("other file.dj")]);
    }

    #[test]
    fn create_file_from_query_creates_root_relative_file() {
        let root = unique_test_dir("djot-filter-create-test");
        std::fs::create_dir_all(&root).unwrap();

        let path = create_file_from_query(&root, "notes/other file.dj").unwrap();
        assert_eq!(path, root.join("notes/other file.dj"));
        assert!(path.exists());

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn create_file_from_query_adds_default_dj_extension() {
        let root = unique_test_dir("djot-filter-create-extension-test");
        std::fs::create_dir_all(&root).unwrap();

        let plain = create_file_from_query(&root, "topic").unwrap();
        assert_eq!(plain, root.join("topic.dj"));

        let nested = create_file_from_query(&root, "notes/topic").unwrap();
        assert_eq!(nested, root.join("notes/topic.dj"));

        let djot = create_file_from_query(&root, "notes/full.djot").unwrap();
        assert_eq!(djot, root.join("notes/full.djot"));

        let other = create_file_from_query(&root, "notes/raw.txt").unwrap();
        assert_eq!(other, root.join("notes/raw.dj"));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn create_file_from_query_rejects_empty_or_escaping_path() {
        let root = unique_test_dir("djot-filter-create-reject-test");
        std::fs::create_dir_all(&root).unwrap();

        assert!(create_file_from_query(&root, "  ").is_err());
        assert!(create_file_from_query(&root, "../outside.dj").is_err());

        let _ = std::fs::remove_dir_all(root);
    }

    fn unique_test_dir(name: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!("{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        path
    }
}

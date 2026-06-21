use std::borrow::Cow;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::OpenOptions;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, ExitCode};
use std::sync::Arc;

use cel::{Context, ExecutionError, Program, Value};
use clap::{Args, Parser, Subcommand};
use djot_core::{metadata_block, resolve_target, tasks, Task, Workspace};
use skim::prelude::*;

fn main() -> ExitCode {
    let config = Config::parse();

    let root = default_root(&config);
    let docs = match load_docs(&root) {
        Ok(docs) => docs,
        Err(err) => {
            eprintln!("djot-filter: {err}");
            return ExitCode::FAILURE;
        }
    };

    match &config.command {
        CommandMode::Notes(notes) => {
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
            if notes.interactive {
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
        CommandMode::Tasks => {
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
            if let Err(err) = print_tasks(&root, &docs, plan.as_ref()) {
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
    Notes(NotesConfig),

    /// Print tasks found in scanned Djot files.
    Tasks,
}

#[derive(Debug, Args)]
struct NotesConfig {
    /// Re-filter results interactively with skim.
    #[arg(short, long)]
    interactive: bool,
}

struct LoadedDocs {
    workspace: Workspace,
    paths: Vec<PathBuf>,
    texts: HashMap<PathBuf, String>,
}

struct QueryPlan {
    program: Program,
}

struct DocumentRecord<'a> {
    root: &'a Path,
    path: &'a Path,
    text: &'a str,
    reverse_references: &'a ReverseReferences,
}

struct TaskRecord<'a> {
    path: &'a Path,
    title: &'a str,
    created: Option<&'a str>,
    done: Option<&'a str>,
    due: Option<&'a str>,
    repeat: Option<&'a str>,
    prev: Option<&'a str>,
}

impl QueryPlan {
    fn compile(source: &str) -> Result<Self, String> {
        let program =
            Program::compile(source).map_err(|err| format!("invalid CEL query: {err}"))?;
        Ok(Self { program })
    }

    fn matches(&self, record: DocumentRecord<'_>) -> Result<bool, String> {
        let mut context = Context::default();
        context.add_variable_from_value("path", display_path(record.root, record.path));
        context.add_variable_from_value("title", document_title(record.text).unwrap_or_default());
        context.add_variable_from_value(
            "directly_referenced_by",
            record
                .reverse_references
                .direct(record.path)
                .into_iter()
                .map(|path| display_path(record.root, &path))
                .collect::<Vec<_>>(),
        );
        context.add_variable_from_value(
            "transitively_referenced_by",
            record
                .reverse_references
                .transitive(record.path)
                .into_iter()
                .map(|path| display_path(record.root, &path))
                .collect::<Vec<_>>(),
        );

        match self.program.execute(&context) {
            Ok(Value::Bool(value)) => Ok(value),
            Ok(value) => Err(format!(
                "CEL query must return bool, got {}",
                value_type_name(&value)
            )),
            Err(ExecutionError::NoSuchKey(_)) => Ok(false),
            Err(err) => Err(format!(
                "cannot evaluate CEL query for {}: {err}",
                record.path.display()
            )),
        }
    }

    fn matches_task(&self, record: TaskRecord<'_>) -> Result<bool, String> {
        let mut context = Context::default();
        context.add_variable_from_value("title", record.title.to_string());
        context.add_variable_from_value("created", record.created.map(str::to_string));
        context.add_variable_from_value("done", record.done.map(str::to_string));
        context.add_variable_from_value("due", record.due.map(str::to_string));
        context.add_variable_from_value("repeat", record.repeat.map(str::to_string));
        context.add_variable_from_value("prev", record.prev.map(str::to_string));

        match self.program.execute(&context) {
            Ok(Value::Bool(value)) => Ok(value),
            Ok(value) => Err(format!(
                "CEL query must return bool, got {}",
                value_type_name(&value)
            )),
            Err(ExecutionError::NoSuchKey(_)) => Ok(false),
            Err(err) => Err(format!(
                "cannot evaluate CEL query for task in {}: {err}",
                record.path.display()
            )),
        }
    }
}

fn retain_query_matches(
    root: &Path,
    docs: &LoadedDocs,
    paths: &mut Vec<PathBuf>,
    plan: &QueryPlan,
) -> Result<(), String> {
    let reverse_references = ReverseReferences::build(&docs.workspace);
    let mut retained = Vec::new();

    for path in paths.drain(..) {
        let Some(text) = docs.texts.get(&path) else {
            continue;
        };
        let record = DocumentRecord {
            root,
            path: &path,
            text,
            reverse_references: &reverse_references,
        };
        if plan.matches(record)? {
            retained.push(path);
        }
    }

    *paths = retained;
    Ok(())
}

struct ReverseReferences {
    direct: HashMap<PathBuf, HashSet<PathBuf>>,
}

impl ReverseReferences {
    fn build(workspace: &Workspace) -> Self {
        let mut direct: HashMap<PathBuf, HashSet<PathBuf>> = HashMap::new();
        for (source, entry) in workspace.documents() {
            for reference in &entry.index.references {
                let Some(target) = resolve_target(source, &reference.target) else {
                    continue;
                };
                if workspace.contains(&target.path) {
                    direct
                        .entry(target.path)
                        .or_default()
                        .insert(source.to_path_buf());
                }
            }
        }
        Self { direct }
    }

    fn direct(&self, path: &Path) -> Vec<PathBuf> {
        sorted_paths(self.direct.get(path).into_iter().flatten().cloned())
    }

    fn transitive(&self, path: &Path) -> Vec<PathBuf> {
        let mut out = HashSet::new();
        let mut queue = VecDeque::new();

        for source in self.direct(path) {
            queue.push_back(source);
        }

        while let Some(source) = queue.pop_front() {
            if source == path || !out.insert(source.clone()) {
                continue;
            }
            for next in self.direct(&source) {
                queue.push_back(next);
            }
        }

        sorted_paths(out)
    }
}

fn sorted_paths(paths: impl IntoIterator<Item = PathBuf>) -> Vec<PathBuf> {
    let mut paths = paths.into_iter().collect::<Vec<_>>();
    paths.sort();
    paths
}

enum InteractiveAction {
    Open(Vec<String>),
    Create(String),
}

#[derive(Clone)]
struct FilterItem {
    path: String,
    searchable: String,
    display: String,
    preview: String,
}

impl FilterItem {
    fn new(path: String, text: String) -> Self {
        let searchable = format!("{path}\n{text}");
        let preview = Self::preview_text(&path, &text);
        let display = ansi_bold(&path);
        Self {
            path,
            searchable,
            display,
            preview,
        }
    }

    fn preview_text(path: &str, text: &str) -> String {
        format!(
            "{}\n{}\n{}",
            ansi_bold(path),
            ansi_dim(&"-".repeat(path.len())),
            highlight_djot_preview(text)
        )
    }
}

impl SkimItem for FilterItem {
    fn text(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.searchable)
    }

    fn display<'a>(&'a self, _context: DisplayContext<'a>) -> AnsiString<'a> {
        AnsiString::parse(&self.display)
    }

    fn preview(&self, _context: PreviewContext) -> ItemPreview {
        ItemPreview::AnsiText(self.preview.clone())
    }

    fn output(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.path)
    }
}

fn load_docs(root: &Path) -> Result<LoadedDocs, String> {
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

fn document_title(text: &str) -> Option<String> {
    let metadata = metadata_block(text)?;
    let value: toml::Value = toml::from_str(&metadata).ok()?;
    value
        .get("title")
        .and_then(|title| title.as_str())
        .map(str::to_string)
}

fn value_type_name(value: &Value) -> &'static str {
    match value {
        Value::List(_) => "list",
        Value::Map(_) => "map",
        Value::Function(_, _) => "function",
        Value::Int(_) => "int",
        Value::UInt(_) => "uint",
        Value::Float(_) => "float",
        Value::String(_) => "string",
        Value::Bytes(_) => "bytes",
        Value::Bool(_) => "bool",
        Value::Duration(_) => "duration",
        Value::Timestamp(_) => "timestamp",
        Value::Opaque(_) => "opaque",
        Value::Null => "null",
    }
}

fn highlight_djot_preview(text: &str) -> String {
    let mut in_code_block = false;
    let mut lines = Vec::new();

    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            in_code_block = !in_code_block;
            lines.push(ansi_color(line, "36"));
        } else if in_code_block {
            lines.push(ansi_color(line, "90"));
        } else if trimmed.starts_with("{.metadata}") {
            lines.push(ansi_color(line, "35"));
        } else if trimmed.starts_with('#') {
            lines.push(ansi_color(line, "1;34"));
        } else {
            lines.push(highlight_links(line));
        }
    }

    lines.join("\n")
}

fn highlight_links(line: &str) -> String {
    let mut out = String::new();
    let mut rest = line;
    while let Some(open) = rest.find('[') {
        let before = &rest[..open];
        out.push_str(before);
        let Some(close_rel) = rest[open..].find(']') else {
            out.push_str(&rest[open..]);
            return out;
        };
        let close = open + close_rel;
        let after_close = &rest[close + 1..];
        if after_close.starts_with('(') {
            let Some(dst_close) = after_close.find(')') else {
                out.push_str(&rest[open..]);
                return out;
            };
            let link_end = close + 1 + dst_close + 1;
            out.push_str(&ansi_color(&rest[open..link_end], "32"));
            rest = &rest[link_end..];
        } else {
            out.push_str(&rest[open..=close]);
            rest = after_close;
        }
    }
    out.push_str(rest);
    out
}

fn ansi_bold(text: &str) -> String {
    ansi_color(text, "1")
}

fn ansi_dim(text: &str) -> String {
    ansi_color(text, "2")
}

fn ansi_color(text: &str, code: &str) -> String {
    format!("\x1b[{code}m{text}\x1b[0m")
}

fn run_interactive(
    root: &Path,
    paths: &[PathBuf],
    texts: &HashMap<PathBuf, String>,
) -> Result<InteractiveAction, String> {
    let options = SkimOptionsBuilder::default()
        .height(Some("100%"))
        .multi(true)
        .preview(Some(""))
        .bind(vec!["ctrl-n:accept"])
        .build()
        .map_err(|err| err.to_string())?;
    let (sender, receiver): (SkimItemSender, SkimItemReceiver) = unbounded();

    for path in paths {
        let Some(text) = texts.get(path) else {
            continue;
        };
        let item = FilterItem::new(display_path(root, path), text.clone());
        sender
            .send(Arc::new(item))
            .map_err(|err| format!("cannot send item to skim: {err}"))?;
    }
    drop(sender);

    let output = Skim::run_with(&options, Some(receiver));
    let Some(output) = output else {
        return Ok(InteractiveAction::Open(Vec::new()));
    };
    if output.final_key == Key::Ctrl('n') {
        return Ok(InteractiveAction::Create(output.query));
    }

    Ok(InteractiveAction::Open(
        output
            .selected_items
            .into_iter()
            .map(|item| item.output().into_owned())
            .collect(),
    ))
}

fn handle_interactive_action(root: &Path, action: InteractiveAction) -> Result<(), String> {
    match action {
        InteractiveAction::Open(selected) => open_in_editor(root, &selected),
        InteractiveAction::Create(name) => {
            let path = create_file_from_query(root, &name)?;
            open_paths_in_editor(&[path])
        }
    }
}

fn open_in_editor(root: &Path, selected: &[String]) -> Result<(), String> {
    if selected.is_empty() {
        return Ok(());
    }

    let paths = editor_paths(root, selected);
    open_paths_in_editor(&paths)
}

fn open_paths_in_editor(paths: &[PathBuf]) -> Result<(), String> {
    if paths.is_empty() {
        return Ok(());
    }

    let editor = std::env::var("EDITOR")
        .map_err(|_| "--interactive selected files, but EDITOR is not set".to_string())?;
    let (program, args) = editor_command(&editor)?;
    let status = Command::new(&program)
        .args(args)
        .args(paths)
        .status()
        .map_err(|err| format!("cannot run editor `{program}`: {err}"))?;

    if !status.success() {
        return Err(format!("editor `{program}` exited with {status}"));
    }

    Ok(())
}

fn create_file_from_query(root: &Path, query: &str) -> Result<PathBuf, String> {
    let name = query.trim();
    if name.is_empty() {
        return Err("cannot create a file from an empty query".to_string());
    }

    let path = normalize(&root.join(with_default_extension(name)));
    if !path.starts_with(root) {
        return Err(format!("new file path escapes root: {name}"));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| format!("cannot create directory {}: {err}", parent.display()))?;
    }
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|err| format!("cannot create {}: {err}", path.display()))?;

    Ok(path)
}

fn with_default_extension(name: &str) -> PathBuf {
    let path = Path::new(name);
    if is_djot_file(path) {
        path.to_path_buf()
    } else {
        path.with_extension("dj")
    }
}

fn editor_command(editor: &str) -> Result<(String, Vec<String>), String> {
    let mut parts =
        shlex::split(editor).ok_or_else(|| format!("cannot parse EDITOR={editor:?}"))?;
    if parts.is_empty() {
        return Err("EDITOR is empty".to_string());
    }
    let program = parts.remove(0);
    Ok((program, parts))
}

fn editor_paths(root: &Path, selected: &[String]) -> Vec<PathBuf> {
    selected
        .iter()
        .map(|path| {
            let path = Path::new(path);
            if path.is_absolute() {
                normalize(path)
            } else {
                normalize(&root.join(path))
            }
        })
        .collect()
}

fn print_tasks(root: &Path, docs: &LoadedDocs, plan: Option<&QueryPlan>) -> Result<(), String> {
    for path in &docs.paths {
        let Some(text) = docs.texts.get(path) else {
            continue;
        };
        for task in tasks(text) {
            if task_matches(root, path, &task, plan)? {
                print_task(&task);
            }
        }
    }
    Ok(())
}

fn task_matches(
    _root: &Path,
    path: &Path,
    task: &Task,
    plan: Option<&QueryPlan>,
) -> Result<bool, String> {
    let Some(plan) = plan else {
        return Ok(true);
    };
    plan.matches_task(TaskRecord {
        path,
        title: &task.title,
        created: task.created.as_deref(),
        done: task.done.as_deref(),
        due: task.due.as_deref(),
        repeat: task.repeat.as_deref(),
        prev: task.prev.as_deref(),
    })
}

fn print_task(task: &Task) {
    println!("{}", task_line(task));
}

fn task_line(task: &Task) -> String {
    let marker = if task.done.is_some() { "o" } else { "-" };
    format!("{marker} {}", task.title)
}

fn print_paths(paths: impl IntoIterator<Item = String>) {
    for path in paths {
        println!("{path}");
    }
}

fn absolute_path(path: &Path) -> PathBuf {
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

fn normalize(path: &Path) -> PathBuf {
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

fn is_djot_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext == "dj" || ext == "djot")
}

fn display_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_defaults_to_current_directory() {
        let config = Config {
            root: None,
            query: None,
            command: CommandMode::Notes(NotesConfig { interactive: false }),
        };
        assert_eq!(default_root(&config), absolute_path(Path::new(".")));
    }

    #[test]
    fn notes_subcommand_accepts_root_query_and_interactive_after_subcommand() {
        let config = Config::parse_from([
            "djot-filter",
            "notes",
            "--root",
            "notes",
            "--query",
            "title.matches('topic')",
            "--interactive",
        ]);

        assert!(matches!(
            config.command,
            CommandMode::Notes(NotesConfig { interactive: true })
        ));
        assert_eq!(config.root.as_deref(), Some(Path::new("notes")));
        assert_eq!(config.query.as_deref(), Some("title.matches('topic')"));
    }

    #[test]
    fn tasks_subcommand_accepts_root_and_query_after_subcommand() {
        let config = Config::parse_from([
            "djot-filter",
            "tasks",
            "--root",
            "notes",
            "--query",
            "done == null",
        ]);

        assert!(matches!(config.command, CommandMode::Tasks));
        assert_eq!(config.root.as_deref(), Some(Path::new("notes")));
        assert_eq!(config.query.as_deref(), Some("done == null"));
    }

    #[test]
    fn default_notes_command_is_removed() {
        let err = Config::try_parse_from(["djot-filter", "--query", "true"]).unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::MissingSubcommand);
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
            "{created=\"2026-06-18T09:00:00+08:00\" due=\"2026-06-20T09:00:00+08:00\" repeat=\"P1W\" prev=\"#previous-task\"}\n::: task\nOpen task\n:::\n\n{created=\"2026-06-19T09:00:00+08:00\" done=\"2026-06-19T21:30:00+08:00\"}\n::: task\nDone task\n:::\n",
        )
        .unwrap();

        let docs = load_docs(&root).unwrap();
        let path = normalize(&root.join("tasks.dj"));
        let text = docs.texts.get(&path).unwrap();
        let found = tasks(text);
        let open = QueryPlan::compile("done == null").unwrap();
        let created = QueryPlan::compile("created == '2026-06-18T09:00:00+08:00'").unwrap();
        let done = QueryPlan::compile("done != null && title.matches('Done')").unwrap();
        let recurring = QueryPlan::compile(
            "due == '2026-06-20T09:00:00+08:00' && repeat == 'P1W' && prev == '#previous-task'",
        )
        .unwrap();

        assert!(task_matches(&root, &path, &found[0], Some(&open)).unwrap());
        assert!(!task_matches(&root, &path, &found[1], Some(&open)).unwrap());
        assert!(task_matches(&root, &path, &found[0], Some(&created)).unwrap());
        assert!(!task_matches(&root, &path, &found[1], Some(&created)).unwrap());
        assert!(task_matches(&root, &path, &found[1], Some(&done)).unwrap());
        assert!(task_matches(&root, &path, &found[0], Some(&recurring)).unwrap());
        assert!(!task_matches(&root, &path, &found[1], Some(&recurring)).unwrap());
        assert_eq!(task_line(&found[0]), "- Open task");
        assert_eq!(task_line(&found[1]), "o Done task");

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

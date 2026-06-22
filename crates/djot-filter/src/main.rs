use std::borrow::Cow;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::OpenOptions;
use std::ops::Range as ByteRange;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, ExitCode};
use std::sync::Arc;

use cel::{Context, ExecutionError, Program, Value};
use chrono::{DateTime, Datelike, Duration, FixedOffset, Local, SecondsFormat, TimeZone, Timelike};
use clap::{Args, Parser, Subcommand};
use comfy_table::{presets::NOTHING, ContentArrangement, Table};
use djot_core::{
    build_index, metadata_block, parse_repeat_rule, resolve_target, tasks, RepeatRule, Task,
    TaskRef, Workspace,
};
use jotdown::{Container, Event, Parser as DjotParser};
use skim::prelude::*;

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

struct LoadedDocs {
    workspace: Workspace,
    paths: Vec<PathBuf>,
    texts: HashMap<PathBuf, String>,
}

struct QueryPlan {
    program: Program,
    now: DateTime<FixedOffset>,
}

struct DocumentRecord<'a> {
    root: &'a Path,
    path: &'a Path,
    text: &'a str,
    reverse_references: &'a ReverseReferences,
}

struct TaskRecord<'a> {
    root: &'a Path,
    path: &'a Path,
    id: Option<&'a str>,
    title: &'a str,
    created: Option<&'a str>,
    done: Option<&'a str>,
    canceled: Option<&'a str>,
    due: Option<&'a str>,
    wait: Option<&'a str>,
    recur: Option<&'a str>,
    prev: Option<&'a str>,
    depends_on: Vec<String>,
    directly_blocking: Vec<String>,
    blocked: bool,
    actionable: bool,
}

#[derive(Debug, PartialEq, Eq)]
struct TaskOutputRecord {
    status: String,
    title: String,
    source: String,
}

impl QueryPlan {
    fn compile(source: &str) -> Result<Self, String> {
        Self::compile_at(source, Local::now().fixed_offset())
    }

    fn compile_at(source: &str, now: DateTime<FixedOffset>) -> Result<Self, String> {
        let program =
            Program::compile(source).map_err(|err| format!("invalid CEL query: {err}"))?;
        Ok(Self { program, now })
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
        context.add_variable_from_value("path", display_path(record.root, record.path));
        context.add_variable_from_value("id", record.id.map(str::to_string));
        context.add_variable_from_value("title", record.title.to_string());
        context.add_variable_from_value("created", datetime_value(record.created));
        context.add_variable_from_value("done", datetime_value(record.done));
        context.add_variable_from_value("canceled", datetime_value(record.canceled));
        context.add_variable_from_value("due", datetime_value(record.due));
        context.add_variable_from_value("wait", datetime_value(record.wait));
        context.add_variable_from_value("now", Value::Timestamp(self.now));
        context.add_variable_from_value("recur", record.recur.map(str::to_string));
        context.add_variable_from_value("prev", record.prev.map(str::to_string));
        context.add_variable_from_value("depends_on", record.depends_on);
        context.add_variable_from_value("directly_blocking", record.directly_blocking);
        context.add_variable_from_value("blocked", record.blocked);
        context.add_variable_from_value("actionable", record.actionable);

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
            for reference in &entry.analysis.index.references {
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

fn datetime_value(value: Option<&str>) -> Value {
    value
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map_or(Value::Null, Value::Timestamp)
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

fn print_tasks(
    root: &Path,
    docs: &LoadedDocs,
    plan: Option<&QueryPlan>,
    tree: bool,
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
        println!("{}", task_table(&records));
    }
    Ok(())
}

fn run_task_action(root: &Path, action: &TaskAction) -> Result<(), String> {
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

fn complete_task_target(root: &Path, target: &str, done: &str) -> Result<(), String> {
    let task_ref = parse_task_target(root, target)?;
    let text = std::fs::read_to_string(&task_ref.path)
        .map_err(|err| format!("cannot read {}: {err}", task_ref.path.display()))?;
    let edits = task_done_edits(&text, &task_ref.id, done)?;
    let updated = apply_task_text_edits(text, edits)?;
    std::fs::write(&task_ref.path, updated)
        .map_err(|err| format!("cannot write {}: {err}", task_ref.path.display()))
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

fn task_done_edits(text: &str, id: &str, done: &str) -> Result<Vec<TaskTextEdit>, String> {
    let task = tasks(text)
        .into_iter()
        .find(|task| task.id.as_deref() == Some(id))
        .ok_or_else(|| format!("task id not found: {id}"))?;
    if task.done.is_some() {
        return Err(format!("task is already done: {id}"));
    }
    if task.canceled.is_some() {
        return Err(format!("task is canceled: {id}"));
    }

    if let Some(edits) = recurring_task_done_edits(text, &task, done) {
        return Ok(edits);
    }
    task_completion_edits(text, &task, done)
        .ok_or_else(|| format!("cannot build done edit for task: {id}"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TaskTextEdit {
    range: ByteRange<usize>,
    new_text: String,
}

fn apply_task_text_edits(mut text: String, mut edits: Vec<TaskTextEdit>) -> Result<String, String> {
    edits.sort_by_key(|edit| edit.range.start);
    for pair in edits.windows(2) {
        if pair[0].range.end > pair[1].range.start {
            return Err("task edits overlap".to_string());
        }
    }
    for edit in edits.into_iter().rev() {
        if edit.range.start > edit.range.end || edit.range.end > text.len() {
            return Err("task edit range is outside the document".to_string());
        }
        text.replace_range(edit.range, &edit.new_text);
    }
    Ok(text)
}

fn task_completion_edits(text: &str, task: &Task, done: &str) -> Option<Vec<TaskTextEdit>> {
    let opening = task_opening_fence(text, &task.range)?;
    Some(vec![TaskTextEdit {
        range: opening.attribute_insert.clone(),
        new_text: format!(
            "{}{{done=\"{done}\"}}\n{}",
            opening.attribute_prefix, opening.fence_prefix
        ),
    }])
}

fn recurring_task_done_edits(text: &str, task: &Task, done: &str) -> Option<Vec<TaskTextEdit>> {
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

    let anchors = build_index(text).anchors;
    let mut reserved = HashSet::new();
    let current_id = task.id.clone()?;
    let next_id = task_instance_id(&task.title, next_due, &anchors, &reserved)?;
    reserved.insert(next_id.clone());
    let next_insert = line_bounds(text, task.range.end)?.1;
    let recur = escape_attribute_value(recur);
    let next_due_text = next_due.to_rfc3339_opts(SecondsFormat::Secs, true);
    let next_wait_text = next_wait.map(|wait| wait.to_rfc3339_opts(SecondsFormat::Secs, true));
    let next_wait_attribute = next_wait_text
        .as_deref()
        .map(|wait| format!(" wait=\"{}\"", escape_attribute_value(wait)))
        .unwrap_or_default();
    let current_id_text = escape_attribute_value(&current_id);
    let next_id_attribute = anchor_attribute(&next_id);
    let div = inherited_task_source(text.get(task.range.clone())?, indent);
    let list_item = single_task_list_item_context(text, opening.line_start, task.range.end, indent);

    let done_text = format!(
        "{}{{done=\"{done}\"}}\n{}",
        opening.attribute_prefix, opening.fence_prefix
    );

    let next_edit = match list_item {
        Some(context) => TaskTextEdit {
            range: context.insert..context.insert,
            new_text: format!(
                "\n{list_indent}- {next_id_attribute}\n{indent}{{created=\"{done}\" due=\"{next_due_text}\"{next_wait_attribute} recur=\"{recur}\" prev=\"#{current_id_text}\"}}\n{div}",
                list_indent = context.list_indent,
            ),
        },
        None => TaskTextEdit {
            range: next_insert..next_insert,
            new_text: format!(
                "\n\n{indent}{next_id_attribute}\n{indent}{{created=\"{done}\" due=\"{next_due_text}\"{next_wait_attribute} recur=\"{recur}\" prev=\"#{current_id_text}\"}}\n{div}"
            ),
        },
    };

    Some(vec![
        TaskTextEdit {
            range: opening.attribute_insert,
            new_text: done_text,
        },
        next_edit,
    ])
}

fn task_matches(
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

fn task_output_record(root: &Path, path: &Path, task: &Task, tree: bool) -> TaskOutputRecord {
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

fn task_table(records: &[TaskOutputRecord]) -> String {
    let mut table = Table::new();
    table
        .load_preset(NOTHING)
        .set_content_arrangement(ContentArrangement::Dynamic);
    for record in records {
        table.add_row([&record.status, &record.title, &record.source]);
    }
    table.to_string()
}

fn print_paths(paths: impl IntoIterator<Item = String>) {
    for path in paths {
        println!("{path}");
    }
}

struct ListTaskContext<'a> {
    list_indent: &'a str,
    insert: usize,
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
    if list_end != line_bounds(text, task_end_line_offset).map(|(_, end)| end)? {
        return None;
    }
    if has_indented_content_after(text, list_end, list_indent) {
        return None;
    }
    if count_task_fences(text.get(list_start..list_end)?) != 1 {
        return None;
    }

    Some(ListTaskContext {
        list_indent,
        insert: list_end,
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

    let mut line_start = previous_line_start(text, task_line_start)?;

    loop {
        let (_, line_end) = line_bounds(text, line_start)?;
        let line = text
            .get(line_start..line_end)?
            .strip_suffix('\r')
            .unwrap_or(text.get(line_start..line_end)?);
        let indent = leading_indent(line);
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            return None;
        }
        if indent == list_indent && trimmed.starts_with("- ") {
            return Some(line_start);
        }
        if indent != task_indent || !trimmed.starts_with('{') {
            return None;
        }
        line_start = previous_line_start(text, line_start)?;
    }
}

fn list_item_end(text: &str, list_start: usize, list_indent: &str) -> Option<usize> {
    let (_, mut line_end) = line_bounds(text, list_start)?;
    let mut next_start = next_line_start(text, line_end)?;

    while next_start < text.len() {
        let (_, next_end) = line_bounds(text, next_start)?;
        let line = text
            .get(next_start..next_end)?
            .strip_suffix('\r')
            .unwrap_or(text.get(next_start..next_end)?);
        let indent = leading_indent(line);
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            break;
        }
        if indent == list_indent && trimmed.starts_with("- ") {
            break;
        }
        if indent.len() <= list_indent.len() {
            break;
        }
        line_end = next_end;
        let Some(start) = next_line_start(text, next_end) else {
            break;
        };
        next_start = start;
    }

    Some(line_end)
}

fn has_indented_content_after(text: &str, line_end: usize, list_indent: &str) -> bool {
    let Some(mut line_start) = next_line_start(text, line_end) else {
        return false;
    };

    while line_start < text.len() {
        let Some((_, next_end)) = line_bounds(text, line_start) else {
            return false;
        };
        let Some(line) = text.get(line_start..next_end) else {
            return false;
        };
        let line = line.strip_suffix('\r').unwrap_or(line);
        let trimmed = line.trim_start();
        if !trimmed.is_empty() {
            let indent = leading_indent(line);
            return indent.len() > list_indent.len();
        }
        let Some(start) = next_line_start(text, next_end) else {
            return false;
        };
        line_start = start;
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

fn filter_recurring_instance_attributes(source: &str) -> String {
    let mut out = String::new();
    for line in source.split_inclusive('\n') {
        match filter_recurring_attribute_line(line) {
            AttributeLineFilter::Keep(line) => out.push_str(line),
            AttributeLineFilter::Replace(line) => out.push_str(&line),
            AttributeLineFilter::Drop => {}
        }
    }
    out
}

enum AttributeLineFilter<'a> {
    Keep(&'a str),
    Replace(String),
    Drop,
}

fn filter_recurring_attribute_line(line: &str) -> AttributeLineFilter<'_> {
    let line_without_newline = line.trim_end_matches(['\r', '\n']);
    let newline = &line[line_without_newline.len()..];
    let indent = leading_indent(line_without_newline);
    let content = &line_without_newline[indent.len()..];
    let Some(inner) = content.strip_prefix('{').and_then(|s| s.strip_suffix('}')) else {
        return AttributeLineFilter::Keep(line);
    };

    let Some(tokens) = attribute_tokens(inner) else {
        return AttributeLineFilter::Keep(line);
    };
    if tokens.is_empty() {
        return AttributeLineFilter::Keep(line);
    }

    let kept = tokens
        .iter()
        .filter(|token| !is_recurring_instance_attribute(token))
        .collect::<Vec<_>>();
    if kept.len() == tokens.len() {
        return AttributeLineFilter::Keep(line);
    }
    if kept.is_empty() {
        return AttributeLineFilter::Drop;
    }

    let mut replacement = String::new();
    replacement.push_str(indent);
    replacement.push('{');
    for (idx, token) in kept.iter().enumerate() {
        if idx > 0 {
            replacement.push(' ');
        }
        replacement.push_str(token);
    }
    replacement.push('}');
    replacement.push_str(newline);
    AttributeLineFilter::Replace(replacement)
}

fn attribute_tokens(inner: &str) -> Option<Vec<&str>> {
    let mut tokens = Vec::new();
    let mut start = None;
    let mut quote = None;
    let mut escaped = false;

    for (idx, ch) in inner.char_indices() {
        if start.is_none() {
            if ch.is_whitespace() {
                continue;
            }
            start = Some(idx);
        }

        if let Some(quoted) = quote {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == quoted {
                quote = None;
            }
            continue;
        }

        if ch == '"' || ch == '\'' {
            quote = Some(ch);
        } else if ch.is_whitespace() {
            if let Some(token_start) = start.take() {
                tokens.push(inner[token_start..idx].trim());
            }
        }
    }

    if quote.is_some() {
        return None;
    }
    if let Some(token_start) = start {
        tokens.push(inner[token_start..].trim());
    }

    Some(
        tokens
            .into_iter()
            .filter(|token| !token.is_empty())
            .collect(),
    )
}

fn is_recurring_instance_attribute(token: &str) -> bool {
    if token.starts_with('#') {
        return true;
    }
    let key = token.split_once('=').map_or(token, |(key, _)| key);
    matches!(
        key,
        "created" | "done" | "canceled" | "due" | "wait" | "recur" | "prev"
    )
}

fn next_recur_due(due: DateTime<FixedOffset>, recur: &str) -> Option<DateTime<FixedOffset>> {
    let rule = parse_repeat_rule(recur)?;
    match rule {
        RepeatRule::Days(days) => Some(due + Duration::days(days)),
        RepeatRule::Weeks(weeks) => Some(due + Duration::weeks(weeks)),
        RepeatRule::Months(months) => add_months(due, months),
        RepeatRule::Years(years) => add_months(due, years.checked_mul(12)?),
    }
}

fn add_months(due: DateTime<FixedOffset>, months: i32) -> Option<DateTime<FixedOffset>> {
    let month0 = due.month0() as i32 + months;
    let year = due.year() + month0.div_euclid(12);
    let month0 = month0.rem_euclid(12);
    let month = (month0 + 1) as u32;
    let day = due.day().min(last_day_of_month(year, month)?);
    due.timezone()
        .with_ymd_and_hms(year, month, day, due.hour(), due.minute(), due.second())
        .single()
}

fn last_day_of_month(year: i32, month: u32) -> Option<u32> {
    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    let first_next = chrono::NaiveDate::from_ymd_opt(next_year, next_month, 1)?;
    Some((first_next - Duration::days(1)).day())
}

fn task_instance_id(
    title: &str,
    due: DateTime<FixedOffset>,
    anchors: &HashMap<String, djot_core::Anchor>,
    reserved: &HashSet<String>,
) -> Option<String> {
    let base = djot_heading_id(title)?;
    let date = due.format("%Y-%m-%d");
    let candidate = format!("{base}-{date}");
    Some(unique_anchor_id(candidate, anchors, reserved))
}

fn djot_heading_id(title: &str) -> Option<String> {
    let source = format!("# {}\n", title.trim());
    DjotParser::new(&source).find_map(|event| match event {
        Event::Start(Container::Heading { id, .. }, _) => Some(id.into_owned()),
        _ => None,
    })
}

fn unique_anchor_id(
    candidate: String,
    anchors: &HashMap<String, djot_core::Anchor>,
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

fn leading_indent(line: &str) -> &str {
    let indent_len = line
        .char_indices()
        .find(|(_, c)| *c != ' ' && *c != '\t')
        .map(|(i, _)| i)
        .unwrap_or(line.len());
    &line[..indent_len]
}

fn escape_attribute_value(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn anchor_attribute(id: &str) -> String {
    if is_shorthand_anchor_id(id) {
        format!("{{#{id}}}")
    } else {
        format!("{{id=\"{}\"}}", escape_attribute_value(id))
    }
}

fn is_shorthand_anchor_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'_' | b'-'))
}

struct TaskOpeningFence {
    line_start: usize,
    attribute_insert: ByteRange<usize>,
    attribute_prefix: String,
    fence_prefix: String,
    task_indent: String,
}

fn task_opening_fence(text: &str, range: &ByteRange<usize>) -> Option<TaskOpeningFence> {
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
        fence_prefix: format!("{indent}  "),
        task_indent: format!("{indent}  "),
    })
}

fn line_bounds(text: &str, offset: usize) -> Option<(usize, usize)> {
    if offset > text.len() {
        return None;
    }
    let start = text[..offset].rfind('\n').map_or(0, |i| i + 1);
    let end = text[offset..].find('\n').map_or(text.len(), |i| offset + i);
    Some((start, end))
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

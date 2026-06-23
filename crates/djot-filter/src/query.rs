use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

use cel::{Context, ExecutionError, Program, Value};
use chrono::{DateTime, FixedOffset, Local};
use djot_core::{metadata_block, resolve_target, Workspace};

use crate::{display_path, LoadedDocs};

pub(crate) struct QueryPlan {
    program: Program,
    pub(crate) now: DateTime<FixedOffset>,
}

struct DocumentRecord<'a> {
    pub(crate) root: &'a Path,
    pub(crate) path: &'a Path,
    text: &'a str,
    reverse_references: &'a ReverseReferences,
}

pub(crate) struct TaskRecord<'a> {
    pub(crate) root: &'a Path,
    pub(crate) path: &'a Path,
    pub(crate) id: Option<&'a str>,
    pub(crate) title: &'a str,
    pub(crate) created: Option<&'a str>,
    pub(crate) done: Option<&'a str>,
    pub(crate) canceled: Option<&'a str>,
    pub(crate) due: Option<&'a str>,
    pub(crate) wait: Option<&'a str>,
    pub(crate) recur: Option<&'a str>,
    pub(crate) prev: Option<&'a str>,
    pub(crate) depends_on: Vec<String>,
    pub(crate) directly_blocking: Vec<String>,
    pub(crate) blocked: bool,
    pub(crate) actionable: bool,
}

impl QueryPlan {
    pub(crate) fn compile(source: &str) -> Result<Self, String> {
        Self::compile_at(source, Local::now().fixed_offset())
    }

    pub(crate) fn compile_at(source: &str, now: DateTime<FixedOffset>) -> Result<Self, String> {
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

    pub(crate) fn matches_task(&self, record: TaskRecord<'_>) -> Result<bool, String> {
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

pub(crate) fn retain_query_matches(
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

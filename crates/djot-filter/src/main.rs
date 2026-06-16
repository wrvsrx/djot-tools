use std::borrow::Cow;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::OpenOptions;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, ExitCode};
use std::sync::Arc;

use clap::Parser;
use djot_core::{metadata_block, resolve_target, Workspace};
use regex::Regex;
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

    let mut paths = docs.paths.clone();
    if !config.referenced_by.is_empty() {
        let seeds = config
            .referenced_by
            .iter()
            .map(|path| referenced_by_path(&root, path))
            .collect::<Vec<_>>();
        let referenced = referenced_files(&docs.workspace, &seeds, config.direct);
        paths.retain(|path| referenced.contains(path));
    }

    if !config.metadata_filters.is_empty() {
        paths.retain(|path| {
            docs.texts
                .get(path)
                .is_some_and(|text| metadata_matches(text, &config.metadata_filters))
        });
    }

    paths.sort();
    if config.interactive {
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

    ExitCode::SUCCESS
}

#[derive(Debug, Parser)]
#[command(
    name = "djot-filter",
    about = "Filter .dj/.djot files under a directory"
)]
struct Config {
    /// Directory to scan recursively. Defaults to the current directory.
    #[arg(long, value_name = "DIR")]
    root: Option<PathBuf>,

    /// Keep files directly or indirectly referenced by FILE. May be repeated;
    /// the result is the union.
    #[arg(long = "referenced-by", value_name = "FILE")]
    referenced_by: Vec<PathBuf>,

    /// With --referenced-by, keep only directly referenced files.
    #[arg(long)]
    direct: bool,

    /// Re-filter results interactively with skim.
    #[arg(short, long)]
    interactive: bool,

    /// Keep files whose string metadata KEY matches REGEX. May be repeated; all
    /// metadata filters must match.
    #[arg(long = "metadata", value_name = "KEY=REGEX", value_parser = parse_metadata_filter)]
    metadata_filters: Vec<MetadataFilter>,
}

#[derive(Debug, Clone)]
struct MetadataFilter {
    key: String,
    regex: Regex,
}

fn parse_metadata_filter(spec: &str) -> Result<MetadataFilter, String> {
    let Some((key, pattern)) = spec.split_once('=') else {
        return Err(format!("metadata filter `{spec}` must be KEY=REGEX"));
    };
    if key.is_empty() {
        return Err("metadata key cannot be empty".to_string());
    }
    let regex =
        Regex::new(pattern).map_err(|err| format!("invalid regex for metadata `{key}`: {err}"))?;
    Ok(MetadataFilter {
        key: key.to_string(),
        regex,
    })
}

struct LoadedDocs {
    workspace: Workspace,
    paths: Vec<PathBuf>,
    texts: HashMap<PathBuf, String>,
}

enum InteractiveAction {
    Open(Vec<String>),
    Create(String),
}

#[derive(Clone)]
struct FilterItem {
    path: String,
    searchable: String,
    preview: String,
}

impl FilterItem {
    fn new(path: String, text: String) -> Self {
        let searchable = format!("{path}\n{text}");
        let preview = Self::preview_text(&path, &text);
        Self {
            path,
            searchable,
            preview,
        }
    }

    fn preview_text(path: &str, text: &str) -> String {
        format!("{path}\n{}\n{text}", "-".repeat(path.len()))
    }
}

impl SkimItem for FilterItem {
    fn text(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.searchable)
    }

    fn display<'a>(&'a self, _context: DisplayContext<'a>) -> AnsiString<'a> {
        AnsiString::parse(&self.path)
    }

    fn preview(&self, _context: PreviewContext) -> ItemPreview {
        ItemPreview::Text(self.preview.clone())
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

fn referenced_files(
    workspace: &Workspace,
    seeds: &[PathBuf],
    direct_only: bool,
) -> HashSet<PathBuf> {
    let mut out = HashSet::new();
    let mut queue = VecDeque::new();
    let seeds = seeds.iter().cloned().collect::<HashSet<_>>();

    for seed in &seeds {
        queue.push_back(seed.clone());
    }

    while let Some(source) = queue.pop_front() {
        let Some(entry) = workspace.get(&source) else {
            continue;
        };

        for reference in &entry.index.references {
            let Some(target) = resolve_target(&source, &reference.target) else {
                continue;
            };
            if !workspace.contains(&target.path) {
                continue;
            }
            if !out.insert(target.path.clone()) {
                continue;
            }
            if !direct_only && !seeds.contains(&target.path) {
                queue.push_back(target.path);
            }
        }
    }

    for seed in seeds {
        out.remove(&seed);
    }
    out
}

fn metadata_matches(text: &str, filters: &[MetadataFilter]) -> bool {
    let Some(metadata) = metadata_block(text) else {
        return false;
    };
    let Ok(table) = toml::from_str::<toml::Table>(&metadata) else {
        return false;
    };

    filters.iter().all(|filter| {
        metadata_string(&table, &filter.key).is_some_and(|value| filter.regex.is_match(value))
    })
}

fn metadata_string<'a>(table: &'a toml::Table, key: &str) -> Option<&'a str> {
    let mut parts = key.split('.');
    let first = parts.next()?;
    let mut value = table.get(first)?;
    for part in parts {
        value = value.as_table()?.get(part)?;
    }
    value.as_str()
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

fn referenced_by_path(root: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        normalize(path)
    } else {
        normalize(&root.join(path))
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
    fn metadata_filter_requires_string_value_matching_regex() {
        let filters = vec![parse_metadata_filter("title=Guide$").unwrap()];
        let text = "{.metadata}\n``` toml\ntitle = \"Usage Guide\"\ndraft = true\n```\n";
        assert!(metadata_matches(text, &filters));

        let filters = vec![parse_metadata_filter("draft=true").unwrap()];
        assert!(!metadata_matches(text, &filters));
    }

    #[test]
    fn referenced_files_can_be_direct_or_transitive() {
        let root = unique_test_dir("djot-filter-reference-test");
        std::fs::create_dir_all(&root).unwrap();
        let a = root.join("a.dj");
        let b = root.join("b.dj");
        let c = root.join("c.dj");
        std::fs::write(&a, "[b](b.dj)\n").unwrap();
        std::fs::write(&b, "[c](c.dj)\n").unwrap();
        std::fs::write(&c, "# C\n").unwrap();

        let docs = load_docs(&root).unwrap();
        let direct = referenced_files(&docs.workspace, &[normalize(&a)], true);
        assert!(direct.contains(&normalize(&b)));
        assert!(!direct.contains(&normalize(&c)));

        let transitive = referenced_files(&docs.workspace, &[normalize(&a)], false);
        assert!(transitive.contains(&normalize(&b)));
        assert!(transitive.contains(&normalize(&c)));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn metadata_key_supports_dotted_tables() {
        let filters = vec![parse_metadata_filter("book.title=Guide").unwrap()];
        let text = "{.metadata}\n``` toml\n[book]\ntitle = \"Guide\"\n```\n";
        assert!(metadata_matches(text, &filters));
    }

    #[test]
    fn referenced_by_relative_paths_are_root_relative() {
        let root = normalize(Path::new("/tmp/djot-filter-root"));
        assert_eq!(
            referenced_by_path(&root, Path::new("index.dj")),
            root.join("index.dj")
        );
        assert_eq!(
            referenced_by_path(&root, Path::new("nested/../index.dj")),
            root.join("index.dj")
        );
    }

    #[test]
    fn root_defaults_to_current_directory() {
        let config = Config {
            root: None,
            referenced_by: Vec::new(),
            direct: false,
            interactive: false,
            metadata_filters: Vec::new(),
        };
        assert_eq!(default_root(&config), absolute_path(Path::new(".")));
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

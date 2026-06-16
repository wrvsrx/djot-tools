use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;

use clap::Parser;
use djot_core::{metadata_block, resolve_target, Workspace};
use regex::Regex;

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
    for path in paths {
        println!("{}", display_path(&root, &path));
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
    root: Option<PathBuf>,

    /// Keep files directly or indirectly referenced by FILE. May be repeated;
    /// the result is the union.
    #[arg(long = "referenced-by", value_name = "FILE")]
    referenced_by: Vec<PathBuf>,

    /// With --referenced-by, keep only directly referenced files.
    #[arg(long)]
    direct: bool,

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
            metadata_filters: Vec::new(),
        };
        assert_eq!(default_root(&config), absolute_path(Path::new(".")));
    }

    fn unique_test_dir(name: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!("{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        path
    }
}

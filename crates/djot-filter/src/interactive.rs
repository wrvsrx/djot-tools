use std::borrow::Cow;
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use skim::prelude::*;

use crate::{display_path, is_djot_file, normalize};

pub(crate) enum InteractiveAction {
    Open(Vec<String>),
    Create(String),
}

#[derive(Clone)]
pub(crate) struct FilterItem {
    path: String,
    searchable: String,
    display: String,
    preview: String,
}

impl FilterItem {
    pub(crate) fn new(path: String, text: String) -> Self {
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

    pub(crate) fn preview_text(path: &str, text: &str) -> String {
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
pub(crate) fn highlight_djot_preview(text: &str) -> String {
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

pub(crate) fn run_interactive(
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

pub(crate) fn handle_interactive_action(
    root: &Path,
    action: InteractiveAction,
) -> Result<(), String> {
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

pub(crate) fn create_file_from_query(root: &Path, query: &str) -> Result<PathBuf, String> {
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

pub(crate) fn editor_command(editor: &str) -> Result<(String, Vec<String>), String> {
    let mut parts =
        shlex::split(editor).ok_or_else(|| format!("cannot parse EDITOR={editor:?}"))?;
    if parts.is_empty() {
        return Err("EDITOR is empty".to_string());
    }
    let program = parts.remove(0);
    Ok((program, parts))
}

pub(crate) fn editor_paths(root: &Path, selected: &[String]) -> Vec<PathBuf> {
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

use std::ops::Range;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextEdit {
    pub range: Range<usize>,
    pub new_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentTextEdit {
    pub path: PathBuf,
    pub edit: TextEdit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRenameEdit {
    pub old_path: PathBuf,
    pub new_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceEdit {
    Text(DocumentTextEdit),
    RenameFile(FileRenameEdit),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditError {
    OverlappingEdits,
    EditRangeOutsideDocument,
}

pub fn apply_text_edits(mut text: String, mut edits: Vec<TextEdit>) -> Result<String, EditError> {
    edits.sort_by_key(|edit| edit.range.start);
    for pair in edits.windows(2) {
        if pair[0].range.end > pair[1].range.start {
            return Err(EditError::OverlappingEdits);
        }
    }
    for edit in edits.into_iter().rev() {
        if edit.range.start > edit.range.end || edit.range.end > text.len() {
            return Err(EditError::EditRangeOutsideDocument);
        }
        text.replace_range(edit.range, &edit.new_text);
    }
    Ok(text)
}

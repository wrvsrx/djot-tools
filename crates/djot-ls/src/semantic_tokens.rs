use djot_core::{NativeTaskListItem, Task};
use lsp_types::{
    SemanticToken, SemanticTokenModifier, SemanticTokenType, SemanticTokens,
    SemanticTokensFullOptions, SemanticTokensLegend, SemanticTokensOptions,
    SemanticTokensServerCapabilities, WorkDoneProgressOptions,
};

use crate::position::offset_to_position;

const TASK_TOKEN_TYPE_INDEX: u32 = 0;
const COMPLETED_MODIFIER_BITSET: u32 = 1;

pub(crate) fn semantic_tokens_provider() -> SemanticTokensServerCapabilities {
    SemanticTokensOptions {
        work_done_progress_options: WorkDoneProgressOptions::default(),
        legend: SemanticTokensLegend {
            token_types: vec![SemanticTokenType::new("task")],
            token_modifiers: vec![SemanticTokenModifier::new("completed")],
        },
        range: None,
        full: Some(SemanticTokensFullOptions::Bool(true)),
    }
    .into()
}

pub(crate) fn task_semantic_tokens(
    text: &str,
    tasks: &[Task],
    native_task_list_items: &[NativeTaskListItem],
) -> SemanticTokens {
    let mut absolute = tasks
        .iter()
        .filter(|task| task.done.is_some() || task.canceled.is_some())
        .filter_map(|task| task.title_range.as_ref())
        .chain(
            native_task_list_items
                .iter()
                .filter(|item| item.checked)
                .filter_map(|item| item.title_range.as_ref()),
        )
        .filter_map(|range| {
            let start = offset_to_position(text, range.start);
            let end = offset_to_position(text, range.end);
            if start.line != end.line || start.character == end.character {
                return None;
            }
            Some((start.line, start.character, end.character - start.character))
        })
        .collect::<Vec<_>>();
    absolute.sort_unstable();

    let mut previous_line = 0;
    let mut previous_start = 0;
    let data = absolute
        .into_iter()
        .map(|(line, start, length)| {
            let delta_line = line - previous_line;
            let delta_start = if delta_line == 0 {
                start - previous_start
            } else {
                start
            };
            previous_line = line;
            previous_start = start;
            SemanticToken {
                delta_line,
                delta_start,
                length,
                token_type: TASK_TOKEN_TYPE_INDEX,
                token_modifiers_bitset: COMPLETED_MODIFIER_BITSET,
            }
        })
        .collect();

    SemanticTokens {
        result_id: None,
        data,
    }
}

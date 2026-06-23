use std::collections::HashSet;
use std::ops::Range;

use djot_core::{analyze, heading_outline, metadata_block, Anchor, Heading, Task};
use lsp_types::{DocumentSymbol, SymbolKind};

use crate::position::byte_range_to_lsp;

pub(crate) fn document_title(text: &str) -> Option<String> {
    let metadata = metadata_block(text)?;
    let value: toml::Value = toml::from_str(&metadata).ok()?;
    value
        .get("title")
        .and_then(|title| title.as_str())
        .map(str::to_string)
}

pub(crate) fn document_symbols(text: &str) -> Vec<DocumentSymbol> {
    let headings = heading_outline(text);
    let heading_ranges = collect_heading_selection_ranges(&headings);
    let analysis = analyze(text);
    let task_anchor_ids = analysis
        .tasks
        .iter()
        .filter_map(|task| task.id.clone())
        .collect::<HashSet<_>>();
    let mut roots = headings
        .iter()
        .map(|heading| heading_symbol(text, heading))
        .collect::<Vec<_>>();

    let mut anchors = analysis
        .index
        .anchors
        .iter()
        .filter(|(id, anchor)| {
            anchor.explicit
                && !heading_ranges.contains(&anchor.range)
                && !task_anchor_ids.contains(id.as_str())
        })
        .map(|(id, anchor)| anchor_symbol(text, id, anchor))
        .collect::<Vec<_>>();
    anchors.sort_by_key(|symbol| symbol.range.start);
    for symbol in anchors {
        insert_symbol(&mut roots, symbol);
    }

    for task in analysis.tasks.iter().map(|task| task_symbol(text, task)) {
        insert_symbol(&mut roots, task);
    }

    for task in analysis.native_task_list_items.iter().map(|task| {
        source_symbol(
            text,
            task.title.trim(),
            if task.checked {
                Some("task list item done".to_string())
            } else {
                Some("task list item".to_string())
            },
            SymbolKind::EVENT,
            task.range.clone(),
            task.title_range
                .clone()
                .unwrap_or_else(|| task.range.clone()),
        )
    }) {
        insert_symbol(&mut roots, task);
    }

    roots.sort_by_key(|symbol| symbol.range.start);
    roots.into_iter().map(SourceSymbol::into_lsp).collect()
}

fn heading_symbol(text: &str, heading: &Heading) -> SourceSymbol {
    let children = heading
        .children
        .iter()
        .map(|child| heading_symbol(text, child))
        .collect();
    source_symbol_with_children(
        text,
        if heading.name.is_empty() {
            format!("H{}", heading.level)
        } else {
            heading.name.clone()
        },
        Some(format!("H{}", heading.level)),
        SymbolKind::STRING,
        heading.range.clone(),
        heading.selection_range.clone(),
        children,
    )
}

fn anchor_symbol(text: &str, id: &str, anchor: &Anchor) -> SourceSymbol {
    source_symbol(
        text,
        &format!("#{id}"),
        Some("anchor".to_string()),
        SymbolKind::KEY,
        anchor.range.clone(),
        anchor.rename_range.clone(),
    )
}

fn task_symbol(text: &str, task: &Task) -> SourceSymbol {
    let detail = if task.canceled.is_some() {
        "task canceled"
    } else if task.done.is_some() {
        "task done"
    } else {
        "task"
    };
    let fallback = task.id.as_deref().unwrap_or("Task");
    let name = non_empty_name(task.title.trim(), fallback);
    source_symbol(
        text,
        name,
        Some(detail.to_string()),
        SymbolKind::EVENT,
        task.range.clone(),
        task.title_range
            .clone()
            .unwrap_or_else(|| task.range.clone()),
    )
}

fn source_symbol(
    text: &str,
    name: &str,
    detail: Option<String>,
    kind: SymbolKind,
    range: Range<usize>,
    selection_range: Range<usize>,
) -> SourceSymbol {
    source_symbol_with_children(
        text,
        non_empty_name(name, "Task").to_string(),
        detail,
        kind,
        range,
        selection_range,
        Vec::new(),
    )
}

fn source_symbol_with_children(
    text: &str,
    name: String,
    detail: Option<String>,
    kind: SymbolKind,
    range: Range<usize>,
    selection_range: Range<usize>,
    children: Vec<SourceSymbol>,
) -> SourceSymbol {
    #[allow(deprecated)]
    let symbol = DocumentSymbol {
        name,
        detail,
        kind,
        tags: None,
        deprecated: None,
        range: byte_range_to_lsp(text, &range),
        selection_range: byte_range_to_lsp(text, &selection_range),
        children: None,
    };
    SourceSymbol {
        range,
        symbol,
        children,
    }
}

fn non_empty_name<'a>(name: &'a str, fallback: &'a str) -> &'a str {
    if name.is_empty() {
        fallback
    } else {
        name
    }
}

fn insert_symbol(nodes: &mut Vec<SourceSymbol>, symbol: SourceSymbol) {
    for node in nodes.iter_mut() {
        if node.range.start <= symbol.range.start
            && symbol.range.end <= node.range.end
            && node.range != symbol.range
        {
            insert_symbol(&mut node.children, symbol);
            return;
        }
    }
    nodes.push(symbol);
}

fn collect_heading_selection_ranges(headings: &[Heading]) -> HashSet<Range<usize>> {
    let mut ranges = HashSet::new();
    for heading in headings {
        ranges.insert(heading.selection_range.clone());
        ranges.extend(collect_heading_selection_ranges(&heading.children));
    }
    ranges
}

struct SourceSymbol {
    range: Range<usize>,
    symbol: DocumentSymbol,
    children: Vec<SourceSymbol>,
}

impl SourceSymbol {
    fn into_lsp(mut self) -> DocumentSymbol {
        self.children.sort_by_key(|child| child.range.start);
        let children = self
            .children
            .into_iter()
            .map(SourceSymbol::into_lsp)
            .collect::<Vec<_>>();
        self.symbol.children = if children.is_empty() {
            None
        } else {
            Some(children)
        };
        self.symbol
    }
}

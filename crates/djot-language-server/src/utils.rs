// fn offset_to_position(offset: usize, rope: &Rope) -> Option<Position> {
//     let line = rope.try_char_to_line(offset).ok()?;
//     let first_char_of_line = rope.try_line_to_char(line).ok()?;
//     let column = offset - first_char_of_line;
//     Some(Position::new(line as u32, column as u32))
// }

pub fn treesitter_range_to_lsp_range(r: tree_sitter::Range) -> tower_lsp::lsp_types::Range {
    tower_lsp::lsp_types::Range {
        start: tower_lsp::lsp_types::Position {
            line: r.start_point.row as u32,
            character: r.start_point.column as u32,
        },
        end: tower_lsp::lsp_types::Position {
            line: r.end_point.row as u32,
            character: r.end_point.column as u32,
        },
    }
}

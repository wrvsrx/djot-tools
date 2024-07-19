use tower_lsp::lsp_types::Position;
pub fn byte_offset_to_position(byte_offset: usize, rope: &ropey::Rope) -> Option<Position> {
    let char_offset = rope.try_byte_to_char(byte_offset).ok()?;
    let line = rope.try_byte_to_line(char_offset).ok()?;
    let first_char_of_line = rope.try_line_to_char(line).ok()?;
    let column = char_offset - first_char_of_line;
    Some(Position::new(line as u32, column as u32))
}

pub fn byte_range_to_lsp_range(
    r: &std::ops::Range<usize>,
    rope: &ropey::Rope,
) -> tower_lsp::lsp_types::Range {
    tower_lsp::lsp_types::Range {
        start: byte_offset_to_position(r.start, rope).unwrap(),
        end: byte_offset_to_position(r.end, rope).unwrap(),
    }
}

//! TUI text editing math and geometry.

/// Returns the number of visual rows needed to display `char_len` characters wrapped at `wrap_width`.
pub fn config_editor_wrap_rows(char_len: usize, wrap_width: usize) -> usize {
    if char_len == 0 {
        1
    } else {
        char_len.div_ceil(wrap_width.max(1))
    }
}

/// Returns the cursor index at the start of the visual line containing `cursor`.
pub fn config_editor_line_start(text: &str, cursor: usize) -> usize {
    let cursor = cursor.min(text.len());
    text[..cursor].rfind('\n').map_or(0, |index| index + 1)
}

/// Returns the cursor index at the end of the visual line containing `cursor`.
pub fn config_editor_line_end(text: &str, cursor: usize) -> usize {
    let cursor = cursor.min(text.len());
    text[cursor..]
        .find('\n')
        .map_or(text.len(), |index| cursor + index)
}

/// Returns the cursor index of the previous character boundary before `cursor`.
pub fn prev_char_boundary(text: &str, cursor: usize) -> usize {
    if cursor == 0 {
        return 0;
    }
    let mut prev = cursor - 1;
    while prev > 0 && !text.is_char_boundary(prev) {
        prev -= 1;
    }
    prev
}

/// Returns the cursor index of the next character boundary after `cursor`.
pub fn next_char_boundary(text: &str, cursor: usize) -> usize {
    if cursor >= text.len() {
        return text.len();
    }
    let mut next = cursor + 1;
    while next < text.len() && !text.is_char_boundary(next) {
        next += 1;
    }
    next.min(text.len())
}

/// Returns the cursor index of the previous word boundary before `cursor`.
pub fn prev_word_boundary(text: &str, cursor: usize) -> usize {
    if cursor == 0 {
        return 0;
    }
    let mut pos = cursor.min(text.len());

    // First, skip any whitespace/newlines to the left
    while pos > 0 {
        let prev = prev_char_boundary(text, pos);
        let ch = text[prev..pos].chars().next().unwrap_or('\n');
        if !ch.is_whitespace() {
            break;
        }
        pos = prev;
    }

    if pos == 0 {
        return 0;
    }

    // Now determine the type of character we are on
    let prev = prev_char_boundary(text, pos);
    let first_ch = text[prev..pos].chars().next().unwrap_or('\n');
    let is_word_char = first_ch.is_alphanumeric() || first_ch == '_';

    // Skip characters of the same type
    while pos > 0 {
        let prev = prev_char_boundary(text, pos);
        let ch = text[prev..pos].chars().next().unwrap_or('\n');
        if ch.is_whitespace() {
            break;
        }
        let ch_is_word = ch.is_alphanumeric() || ch == '_';
        if ch_is_word != is_word_char {
            break;
        }
        pos = prev;
    }

    pos
}

/// Returns the cursor index of the next word boundary after `cursor`.
pub fn next_word_boundary(text: &str, cursor: usize) -> usize {
    let mut pos = cursor.min(text.len());
    if pos >= text.len() {
        return text.len();
    }

    // Determine the type of character at the cursor
    let first_ch = text[pos..].chars().next().unwrap_or('\n');

    if first_ch.is_whitespace() {
        // Skip whitespace/newlines
        while pos < text.len() {
            let next = next_char_boundary(text, pos);
            let ch = text[pos..next].chars().next().unwrap_or('\n');
            if !ch.is_whitespace() {
                break;
            }
            pos = next;
        }
    } else {
        let is_word_char = first_ch.is_alphanumeric() || first_ch == '_';
        // Skip characters of the same type
        while pos < text.len() {
            let next = next_char_boundary(text, pos);
            let ch = text[pos..next].chars().next().unwrap_or('\n');
            if ch.is_whitespace() {
                break;
            }
            let ch_is_word = ch.is_alphanumeric() || ch == '_';
            if ch_is_word != is_word_char {
                break;
            }
            pos = next;
        }
        // Then, skip trailing whitespace
        while pos < text.len() {
            let next = next_char_boundary(text, pos);
            let ch = text[pos..next].chars().next().unwrap_or('\n');
            if !ch.is_whitespace() {
                break;
            }
            pos = next;
        }
    }

    pos
}

/// Converts a visual character column back to a string byte index.
pub fn byte_index_for_char_column(text: &str, char_col: usize) -> usize {
    if char_col == 0 {
        return 0;
    }
    text.char_indices()
        .nth(char_col)
        .map_or(text.len(), |(index, _)| index)
}

/// Returns the visual position `(row, column)` for a given byte `cursor` wrapped at `wrap_width`.
pub fn cursor_visual_position(text: &str, cursor: usize, wrap_width: usize) -> (usize, usize) {
    let wrap_width = wrap_width.max(1);
    let cursor = cursor.min(text.len());
    let before = &text[..cursor];
    let line_idx = before.chars().filter(|&c| c == '\n').count();
    let line_start = config_editor_line_start(text, cursor);
    let line_end = config_editor_line_end(text, cursor);
    let line = &text[line_start..line_end];
    let line_col = text[line_start..cursor].chars().count();
    let line_len = line.chars().count();

    let rows_before = text
        .split('\n')
        .take(line_idx)
        .map(|segment| config_editor_wrap_rows(segment.chars().count(), wrap_width))
        .sum::<usize>();

    if line_col == line_len && line_len > 0 && line_len.is_multiple_of(wrap_width) {
        (
            rows_before + (line_col / wrap_width).saturating_sub(1),
            wrap_width - 1,
        )
    } else {
        (rows_before + (line_col / wrap_width), line_col % wrap_width)
    }
}

/// Computes the string byte index corresponding to a `target_row` and `target_col` wrapped at `wrap_width`.
pub fn cursor_from_visual_position(
    text: &str,
    target_row: usize,
    target_col: usize,
    wrap_width: usize,
) -> usize {
    let wrap_width = wrap_width.max(1);
    let mut row_base = 0_usize;
    let mut line_start = 0_usize;

    for line in text.split('\n') {
        let line_len = line.chars().count();
        let row_count = config_editor_wrap_rows(line_len, wrap_width);
        if target_row < row_base + row_count {
            let local_row = target_row - row_base;
            let target_char_col = (local_row * wrap_width + target_col).min(line_len);
            return line_start + byte_index_for_char_column(line, target_char_col);
        }
        row_base += row_count;
        line_start += line.len() + 1;
    }

    text.len()
}

/// Expand stored tab characters only for display. The buffer remains byte-for-
/// byte unchanged while cursor geometry and wrapping use terminal cell widths.
pub fn expand_tabs_for_editor_view(text: &str, cursor: usize, tab_width: usize) -> (String, usize) {
    let tab_width = tab_width.max(1);
    let cursor = cursor.min(text.len());
    let mut display = String::with_capacity(text.len());
    let mut display_cursor = 0;
    let mut column = 0_usize;
    for (byte_index, character) in text.char_indices() {
        if byte_index == cursor {
            display_cursor = display.len();
        }
        match character {
            '\t' => {
                let spaces = tab_width - (column % tab_width);
                display.extend(std::iter::repeat_n(' ', spaces));
                column += spaces;
            }
            '\n' => {
                display.push(character);
                column = 0;
            }
            _ => {
                display.push(character);
                column += 1;
            }
        }
    }
    if cursor == text.len() {
        display_cursor = display.len();
    }
    (display, display_cursor)
}

/// Returns the 0-indexed line number corresponding to the given byte cursor in the text.
pub fn project_editor_line_at(text: &str, cursor: usize) -> usize {
    text[..cursor.min(text.len())]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_visual_position_handles_wrap_boundary_at_line_end() {
        let (row, col) = cursor_visual_position("abcd", 4, 4);
        assert_eq!((row, col), (0, 3));
    }
}

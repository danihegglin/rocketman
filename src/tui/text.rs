//! Minimal single- and multi-line text editing for the terminal UI.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// What a key did to an input.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Edit {
    /// The text changed.
    Changed,
    /// Only the cursor moved.
    Moved,
    /// The key isn't an editing key; the caller may handle it.
    Ignored,
}

fn byte_index(s: &str, char_ix: usize) -> usize {
    s.char_indices().nth(char_ix).map_or(s.len(), |(i, _)| i)
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// A single line of text with a cursor (counted in chars).
#[derive(Clone, Debug, Default)]
pub struct LineInput {
    value: String,
    cursor: usize,
}

impl LineInput {
    pub fn new(value: impl Into<String>) -> Self {
        let mut input = Self::default();
        input.set(value);
        input
    }

    pub fn value(&self) -> &str {
        &self.value
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn len(&self) -> usize {
        self.value.chars().count()
    }

    pub fn is_empty(&self) -> bool {
        self.value.is_empty()
    }

    /// Replaces the text and puts the cursor at the end.
    pub fn set(&mut self, value: impl Into<String>) {
        self.value = value.into();
        self.cursor = self.len();
    }

    pub fn set_cursor(&mut self, cursor: usize) {
        self.cursor = cursor.min(self.len());
    }

    pub fn insert_str(&mut self, text: &str) {
        let at = byte_index(&self.value, self.cursor);
        // A single-line field: line breaks from a paste become spaces.
        let text: String = text
            .chars()
            .filter(|c| *c != '\r')
            .map(|c| if c == '\n' || c == '\t' { ' ' } else { c })
            .collect();
        self.value.insert_str(at, &text);
        self.cursor += text.chars().count();
    }

    fn word_left(&self) -> usize {
        let chars: Vec<char> = self.value.chars().collect();
        let mut i = self.cursor;
        while i > 0 && !is_word(chars[i - 1]) {
            i -= 1;
        }
        while i > 0 && is_word(chars[i - 1]) {
            i -= 1;
        }
        i
    }

    fn word_right(&self) -> usize {
        let chars: Vec<char> = self.value.chars().collect();
        let mut i = self.cursor;
        while i < chars.len() && !is_word(chars[i]) {
            i += 1;
        }
        while i < chars.len() && is_word(chars[i]) {
            i += 1;
        }
        i
    }

    fn delete_range(&mut self, from: usize, to: usize) {
        let (a, b) = (byte_index(&self.value, from), byte_index(&self.value, to));
        self.value.replace_range(a..b, "");
        self.cursor = from;
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Edit {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Char(c) if !ctrl && !alt => {
                self.insert_str(c.encode_utf8(&mut [0; 4]));
                Edit::Changed
            }
            KeyCode::Char('a') if ctrl => {
                self.cursor = 0;
                Edit::Moved
            }
            KeyCode::Char('e') if ctrl => {
                self.cursor = self.len();
                Edit::Moved
            }
            KeyCode::Char('u') if ctrl => {
                self.delete_range(0, self.cursor);
                Edit::Changed
            }
            KeyCode::Char('k') if ctrl => {
                self.delete_range(self.cursor, self.len());
                Edit::Changed
            }
            KeyCode::Char('w') if ctrl => {
                self.delete_range(self.word_left(), self.cursor);
                Edit::Changed
            }
            KeyCode::Backspace if ctrl || alt => {
                self.delete_range(self.word_left(), self.cursor);
                Edit::Changed
            }
            KeyCode::Backspace if self.cursor > 0 => {
                self.delete_range(self.cursor - 1, self.cursor);
                Edit::Changed
            }
            KeyCode::Delete if self.cursor < self.len() => {
                self.delete_range(self.cursor, self.cursor + 1);
                Edit::Changed
            }
            KeyCode::Backspace | KeyCode::Delete => Edit::Moved,
            KeyCode::Left if ctrl || alt => {
                self.cursor = self.word_left();
                Edit::Moved
            }
            KeyCode::Right if ctrl || alt => {
                self.cursor = self.word_right();
                Edit::Moved
            }
            KeyCode::Left if self.cursor > 0 => {
                self.cursor -= 1;
                Edit::Moved
            }
            KeyCode::Right if self.cursor < self.len() => {
                self.cursor += 1;
                Edit::Moved
            }
            KeyCode::Home => {
                self.cursor = 0;
                Edit::Moved
            }
            KeyCode::End => {
                self.cursor = self.len();
                Edit::Moved
            }
            _ => Edit::Ignored,
        }
    }

    /// The part of the text that fits in `width` columns with the cursor
    /// visible, and the cursor's column within it.
    pub fn view(&self, width: usize) -> (String, usize) {
        let width = width.max(1);
        let start = (self.cursor + 1).saturating_sub(width);
        let text: String = self.value.chars().skip(start).take(width).collect();
        (text, self.cursor - start)
    }
}

/// Multi-line text with a cursor and a scroll position.
#[derive(Clone, Debug)]
pub struct TextArea {
    lines: Vec<String>,
    row: usize,
    col: usize,
    /// First visible line and column, kept up to date by [`TextArea::scroll_to_cursor`].
    pub scroll: (usize, usize),
}

impl Default for TextArea {
    fn default() -> Self {
        Self {
            lines: vec![String::new()],
            row: 0,
            col: 0,
            scroll: (0, 0),
        }
    }
}

impl TextArea {
    pub fn set(&mut self, text: &str) {
        self.lines = text.split('\n').map(|l| l.trim_end_matches('\r').to_string()).collect();
        self.row = 0;
        self.col = 0;
        self.scroll = (0, 0);
    }

    pub fn text(&self) -> String {
        self.lines.join("\n")
    }

    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    pub fn cursor(&self) -> (usize, usize) {
        (self.row, self.col)
    }

    fn line_len(&self, row: usize) -> usize {
        self.lines[row].chars().count()
    }

    pub fn insert_str(&mut self, text: &str) {
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        let line = &mut self.lines[self.row];
        let at = byte_index(line, self.col);
        let tail = line.split_off(at);
        let mut parts = text.split('\n');
        let first = parts.next().unwrap_or("");
        line.push_str(first);
        self.col += first.chars().count();
        for part in parts {
            self.row += 1;
            self.lines.insert(self.row, part.to_string());
            self.col = part.chars().count();
        }
        self.lines[self.row].push_str(&tail);
    }

    fn newline(&mut self) {
        // Keep the current line's indentation, which is what you want in JSON.
        let indent: String = self.lines[self.row]
            .chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .take(self.col)
            .collect();
        self.insert_str(&format!("\n{indent}"));
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Edit {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Enter if !ctrl && !alt => {
                self.newline();
                Edit::Changed
            }
            KeyCode::Char(c) if !ctrl && !alt => {
                self.insert_str(c.encode_utf8(&mut [0; 4]));
                Edit::Changed
            }
            KeyCode::Backspace => {
                if self.col > 0 {
                    let line = &mut self.lines[self.row];
                    let (a, b) = (byte_index(line, self.col - 1), byte_index(line, self.col));
                    line.replace_range(a..b, "");
                    self.col -= 1;
                } else if self.row > 0 {
                    let line = self.lines.remove(self.row);
                    self.row -= 1;
                    self.col = self.line_len(self.row);
                    self.lines[self.row].push_str(&line);
                } else {
                    return Edit::Moved;
                }
                Edit::Changed
            }
            KeyCode::Delete => {
                if self.col < self.line_len(self.row) {
                    let line = &mut self.lines[self.row];
                    let (a, b) = (byte_index(line, self.col), byte_index(line, self.col + 1));
                    line.replace_range(a..b, "");
                } else if self.row + 1 < self.lines.len() {
                    let next = self.lines.remove(self.row + 1);
                    self.lines[self.row].push_str(&next);
                } else {
                    return Edit::Moved;
                }
                Edit::Changed
            }
            KeyCode::Char('k') if ctrl => {
                let line = &mut self.lines[self.row];
                let at = byte_index(line, self.col);
                line.truncate(at);
                Edit::Changed
            }
            KeyCode::Left => {
                if self.col > 0 {
                    self.col -= 1;
                } else if self.row > 0 {
                    self.row -= 1;
                    self.col = self.line_len(self.row);
                }
                Edit::Moved
            }
            KeyCode::Right => {
                if self.col < self.line_len(self.row) {
                    self.col += 1;
                } else if self.row + 1 < self.lines.len() {
                    self.row += 1;
                    self.col = 0;
                }
                Edit::Moved
            }
            KeyCode::Up if self.row > 0 => {
                self.row -= 1;
                self.col = self.col.min(self.line_len(self.row));
                Edit::Moved
            }
            KeyCode::Down if self.row + 1 < self.lines.len() => {
                self.row += 1;
                self.col = self.col.min(self.line_len(self.row));
                Edit::Moved
            }
            KeyCode::PageUp => {
                self.row = self.row.saturating_sub(10);
                self.col = self.col.min(self.line_len(self.row));
                Edit::Moved
            }
            KeyCode::PageDown => {
                self.row = (self.row + 10).min(self.lines.len() - 1);
                self.col = self.col.min(self.line_len(self.row));
                Edit::Moved
            }
            KeyCode::Home => {
                self.col = 0;
                Edit::Moved
            }
            KeyCode::Char('a') if ctrl => {
                self.col = 0;
                Edit::Moved
            }
            KeyCode::End => {
                self.col = self.line_len(self.row);
                Edit::Moved
            }
            KeyCode::Char('e') if ctrl => {
                self.col = self.line_len(self.row);
                Edit::Moved
            }
            _ => Edit::Ignored,
        }
    }

    /// Adjusts the scroll position so the cursor is inside a `width` x `height` view.
    pub fn scroll_to_cursor(&mut self, width: usize, height: usize) {
        let (mut top, mut left) = self.scroll;
        let (width, height) = (width.max(1), height.max(1));
        if self.row < top {
            top = self.row;
        } else if self.row >= top + height {
            top = self.row + 1 - height;
        }
        if self.col < left {
            left = self.col;
        } else if self.col >= left + width {
            left = self.col + 1 - width;
        }
        self.scroll = (top, left);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::KeyCode::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn line_editing() {
        let mut input = LineInput::new("héllo");
        assert_eq!(input.cursor(), 5);
        input.handle_key(key(Left));
        input.handle_key(key(Backspace));
        assert_eq!(input.value(), "hélo");
        input.insert_str("a\nb");
        assert_eq!(input.value(), "héla bo");
        assert_eq!(input.handle_key(key(F(2))), Edit::Ignored);
        let (text, cursor) = LineInput::new("abcdef").view(3);
        assert_eq!((text.as_str(), cursor), ("ef", 2));
    }

    #[test]
    fn text_area_editing() {
        let mut area = TextArea::default();
        area.insert_str("{\n  \"a\": 1");
        assert_eq!(area.cursor(), (1, 8));
        area.handle_key(key(Enter));
        assert_eq!(area.cursor(), (2, 2));
        area.insert_str("}");
        assert_eq!(area.text(), "{\n  \"a\": 1\n  }");
        area.handle_key(key(Home));
        area.handle_key(key(Backspace));
        assert_eq!(area.text(), "{\n  \"a\": 1  }");
        area.scroll_to_cursor(4, 1);
        assert_eq!(area.scroll, (1, 5));
    }
}

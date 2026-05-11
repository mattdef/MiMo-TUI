#[derive(Debug, Default, Clone)]
pub struct InputBuffer {
    text: String,
    cursor: usize,
}

impl InputBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from(text: impl Into<String>) -> Self {
        let text = text.into();
        let cursor = text.chars().count();
        Self { text, cursor }
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
    }

    pub fn set_text(&mut self, text: impl Into<String>) {
        *self = Self::from(text);
    }

    pub fn trim(&self) -> &str {
        self.text.trim()
    }

    pub fn char_len(&self) -> usize {
        self.text.chars().count()
    }

    pub fn insert_char(&mut self, ch: char) {
        let byte_index = self.byte_index(self.cursor);
        self.text.insert(byte_index, ch);
        self.cursor += 1;
    }

    pub fn insert_str(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let byte_index = self.byte_index(self.cursor);
        self.text.insert_str(byte_index, text);
        self.cursor += text.chars().count();
    }

    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let end = self.byte_index(self.cursor);
        let start = self.byte_index(self.cursor - 1);
        self.text.replace_range(start..end, "");
        self.cursor -= 1;
    }

    pub fn delete(&mut self) {
        if self.cursor >= self.char_len() {
            return;
        }
        let start = self.byte_index(self.cursor);
        let end = self.byte_index(self.cursor + 1);
        self.text.replace_range(start..end, "");
    }

    pub fn move_left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn move_right(&mut self) {
        self.cursor = (self.cursor + 1).min(self.char_len());
    }

    pub fn move_to_line_start(&mut self) {
        self.cursor = self.current_line_start();
    }

    pub fn move_to_line_end(&mut self) {
        self.cursor = self.current_line_end();
    }

    pub fn replace_char_range(&mut self, start: usize, end: usize, replacement: &str) {
        let start = start.min(self.char_len());
        let end = end.min(self.char_len()).max(start);
        let start_byte = self.byte_index(start);
        let end_byte = self.byte_index(end);
        self.text.replace_range(start_byte..end_byte, replacement);
        self.cursor = start + replacement.chars().count();
    }

    pub fn current_token_bounds(&self) -> (usize, usize) {
        let chars = self.text.chars().collect::<Vec<_>>();
        let mut start = self.cursor.min(chars.len());
        while start > 0 && !chars[start - 1].is_whitespace() {
            start -= 1;
        }

        let mut end = self.cursor.min(chars.len());
        while end < chars.len() && !chars[end].is_whitespace() {
            end += 1;
        }

        (start, end)
    }

    pub fn line_count(&self) -> usize {
        self.text.chars().filter(|ch| *ch == '\n').count() + 1
    }

    pub fn cursor_line_col(&self) -> (usize, usize) {
        let line_start = self.current_line_start();
        let current_line = self
            .text
            .chars()
            .take(self.cursor)
            .filter(|ch| *ch == '\n')
            .count();
        (current_line, self.cursor.saturating_sub(line_start))
    }

    fn current_line_start(&self) -> usize {
        let mut line_start = 0;
        for (index, ch) in self.text.chars().take(self.cursor).enumerate() {
            if ch == '\n' {
                line_start = index + 1;
            }
        }
        line_start
    }

    fn current_line_end(&self) -> usize {
        let mut end = self.char_len();
        for (index, ch) in self.text.chars().enumerate().skip(self.cursor) {
            if ch == '\n' {
                end = index;
                break;
            }
        }
        end
    }

    fn byte_index(&self, char_index: usize) -> usize {
        self.text
            .char_indices()
            .nth(char_index)
            .map(|(index, _)| index)
            .unwrap_or(self.text.len())
    }
}

#[cfg(test)]
mod tests {
    use super::InputBuffer;

    #[test]
    fn supports_basic_editing() {
        let mut buffer = InputBuffer::new();
        buffer.insert_str("hello");
        buffer.move_left();
        buffer.insert_char('X');
        buffer.delete();
        buffer.backspace();
        assert_eq!(buffer.as_str(), "hell");
        assert_eq!(buffer.char_len(), 4);
    }

    #[test]
    fn computes_line_navigation() {
        let mut buffer = InputBuffer::from("one\ntwo\nthree");
        buffer.set_text("one\ntwo\nthree");
        for _ in 0..8 {
            buffer.move_left();
        }
        assert_eq!(buffer.cursor_line_col(), (1, 1));
        buffer.move_to_line_start();
        assert_eq!(buffer.cursor_line_col(), (1, 0));
        buffer.move_to_line_end();
        assert_eq!(buffer.cursor_line_col(), (1, 3));
    }

    #[test]
    fn replaces_char_ranges() {
        let mut buffer = InputBuffer::from("alpha beta");
        buffer.replace_char_range(6, 10, "gamma");
        assert_eq!(buffer.as_str(), "alpha gamma");
    }

    #[test]
    fn counts_trailing_empty_line() {
        let buffer = InputBuffer::from("one\n");
        assert_eq!(buffer.line_count(), 2);
        assert_eq!(buffer.cursor_line_col(), (1, 0));
    }
}

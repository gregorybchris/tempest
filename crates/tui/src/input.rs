//! A one-line text input: the filter, the palette's query and the fields of modals.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use unicode_width::UnicodeWidthChar;

/// Text being typed, and where the cursor is in it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Input {
    chars: Vec<char>,
    /// The cursor, as a count of chars before it.
    cursor: usize,
}

impl Input {
    /// An input holding `text`, with the cursor at its end.
    pub fn new(text: &str) -> Self {
        let chars: Vec<char> = text.chars().collect();
        let cursor = chars.len();
        Self { chars, cursor }
    }

    /// The text.
    pub fn text(&self) -> String {
        self.chars.iter().collect()
    }

    /// Whether nothing is typed.
    pub fn is_empty(&self) -> bool {
        self.chars.is_empty()
    }

    /// Types `text` at the cursor. Line breaks become spaces, since the input is one line.
    pub fn insert(&mut self, text: &str) {
        for c in text.chars() {
            let c = if c == '\n' || c == '\r' || c == '\t' {
                ' '
            } else {
                c
            };
            if !c.is_control() {
                self.chars.insert(self.cursor, c);
                self.cursor += 1;
            }
        }
    }

    /// Edits the text for `key`, returning whether the key was an editing key.
    pub fn key(&mut self, key: KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('u') if ctrl => {
                self.chars.drain(..self.cursor);
                self.cursor = 0;
            }
            KeyCode::Char('w') if ctrl => {
                let start = self.word_start();
                self.chars.drain(start..self.cursor);
                self.cursor = start;
            }
            KeyCode::Char('a') if ctrl => self.cursor = 0,
            KeyCode::Char('e') if ctrl => self.cursor = self.chars.len(),
            KeyCode::Char(c) if !ctrl => self.insert(&c.to_string()),
            KeyCode::Backspace if self.cursor > 0 => {
                self.cursor -= 1;
                self.chars.remove(self.cursor);
            }
            KeyCode::Delete if self.cursor < self.chars.len() => {
                self.chars.remove(self.cursor);
            }
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => self.cursor = (self.cursor + 1).min(self.chars.len()),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.chars.len(),
            KeyCode::Backspace | KeyCode::Delete => {}
            _ => return false,
        }
        true
    }

    /// Where the word before the cursor starts.
    fn word_start(&self) -> usize {
        let before = &self.chars[..self.cursor];
        let end = before
            .iter()
            .rposition(|c| !c.is_whitespace())
            .map_or(0, |i| i + 1);
        before[..end]
            .iter()
            .rposition(|c| c.is_whitespace())
            .map_or(0, |i| i + 1)
    }

    /// The part of the text that fits in `width` columns with the cursor in view, and the
    /// cursor's column within it.
    pub fn view(&self, width: usize) -> (String, usize) {
        let width = width.max(1);
        let char_width = |c: &char| c.width().unwrap_or(0);
        // Start as far left as keeps the cursor, and a column for it, in view.
        let mut start = self.cursor;
        let mut used = 1;
        while start > 0 && used + char_width(&self.chars[start - 1]) <= width {
            start -= 1;
            used += char_width(&self.chars[start]);
        }
        let mut shown = String::new();
        let mut columns = 0;
        let mut cursor_column = 0;
        for (index, c) in self.chars.iter().enumerate().skip(start) {
            if index == self.cursor {
                cursor_column = columns;
            }
            if columns + char_width(c) > width {
                break;
            }
            columns += char_width(c);
            shown.push(*c);
        }
        if self.cursor == self.chars.len() {
            cursor_column = columns;
        }
        (shown, cursor_column.min(width - 1))
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn press(input: &mut Input, code: KeyCode, modifiers: KeyModifiers) {
        input.key(KeyEvent::new(code, modifiers));
    }

    #[test]
    fn typing_and_editing_at_the_cursor() {
        let mut input = Input::new("ensemble");
        press(&mut input, KeyCode::Home, KeyModifiers::NONE);
        input.insert("the ");
        assert_eq!(input.text(), "the ensemble");
        press(&mut input, KeyCode::Backspace, KeyModifiers::NONE);
        press(&mut input, KeyCode::Delete, KeyModifiers::NONE);
        assert_eq!(input.text(), "thensemble");
        press(&mut input, KeyCode::Char('e'), KeyModifiers::CONTROL);
        input.insert(" 2026\nfinal");
        assert_eq!(input.text(), "thensemble 2026 final");
        press(&mut input, KeyCode::Char('w'), KeyModifiers::CONTROL);
        assert_eq!(input.text(), "thensemble 2026 ");
        press(&mut input, KeyCode::Char('u'), KeyModifiers::CONTROL);
        assert!(input.is_empty());
        assert!(!input.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
    }

    #[test]
    fn long_text_scrolls_to_keep_the_cursor_in_view() {
        let mut input = Input::new("tempest:abcdefghij");
        assert_eq!(input.view(8), ("bcdefghij".chars().skip(2).collect(), 7));
        press(&mut input, KeyCode::Home, KeyModifiers::NONE);
        assert_eq!(input.view(8), ("tempest:".to_owned(), 0));
        assert_eq!(Input::new("").view(8), (String::new(), 0));
    }

    proptest! {
        #[test]
        fn the_view_fits_and_shows_the_cursor(text in "\\PC{0,40}", width in 1usize..30, left in 0usize..45) {
            let mut input = Input::new(&text);
            for _ in 0..left {
                press(&mut input, KeyCode::Left, KeyModifiers::NONE);
            }
            let (shown, cursor) = input.view(width);
            prop_assert!(unicode_width::UnicodeWidthStr::width(shown.as_str()) <= width);
            prop_assert!(cursor < width);
        }
    }
}

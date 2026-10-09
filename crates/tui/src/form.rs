//! A form of labeled fields: text, a choice among options, or days of the week. The Share
//! area and the modals for a transfer's limits, a new category and the schedule use it.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::input::Input;

/// The days of the week, Monday first, as the days field shows them.
pub const DAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

/// A field's value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    /// Typed text.
    Text(Input),
    /// One of several options, changed with ← and →.
    Choice {
        /// The options.
        options: Vec<String>,
        /// The chosen one.
        chosen: usize,
    },
    /// Days of the week, each on or off.
    Days {
        /// Whether each day, Monday first, is on.
        on: [bool; 7],
        /// The day the cursor is on.
        cursor: usize,
    },
}

/// A labeled field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    /// Its label.
    pub label: &'static str,
    /// What shows in an empty text field.
    pub placeholder: String,
    /// Its value.
    pub value: Value,
}

impl Field {
    /// A text field holding `text`.
    pub fn text(label: &'static str, text: &str, placeholder: impl Into<String>) -> Self {
        Self {
            label,
            placeholder: placeholder.into(),
            value: Value::Text(Input::new(text)),
        }
    }

    /// A choice among `options`, `chosen` first.
    pub fn choice(label: &'static str, options: Vec<String>, chosen: usize) -> Self {
        Self {
            label,
            placeholder: String::new(),
            value: Value::Choice { options, chosen },
        }
    }

    /// Days of the week, those in `on` switched on.
    pub fn days(label: &'static str, on: [bool; 7]) -> Self {
        Self {
            label,
            placeholder: String::new(),
            value: Value::Days { on, cursor: 0 },
        }
    }
}

/// Fields, one of which has the keys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Form {
    /// The fields, top to bottom.
    pub fields: Vec<Field>,
    /// The field with the keys.
    pub focus: usize,
    /// Why the form was refused, if it was.
    pub error: Option<String>,
}

/// What a key did to a form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// It changed, or nothing happened.
    Edited,
    /// `enter`: the form is to be sent.
    Submit,
}

impl Form {
    /// A form of `fields`, the first focused.
    pub fn new(fields: Vec<Field>) -> Self {
        Self {
            fields,
            focus: 0,
            error: None,
        }
    }

    /// The text of the field labeled `label`, trimmed, or empty.
    pub fn text(&self, label: &str) -> String {
        match self.value(label) {
            Some(Value::Text(input)) => input.text().trim().to_owned(),
            _ => String::new(),
        }
    }

    /// The chosen option's index in the field labeled `label`.
    pub fn chosen(&self, label: &str) -> usize {
        match self.value(label) {
            Some(Value::Choice { chosen, .. }) => *chosen,
            _ => 0,
        }
    }

    /// The days switched on in the field labeled `label`.
    pub fn days(&self, label: &str) -> [bool; 7] {
        match self.value(label) {
            Some(Value::Days { on, .. }) => *on,
            _ => [false; 7],
        }
    }

    fn value(&self, label: &str) -> Option<&Value> {
        self.fields
            .iter()
            .find(|field| field.label == label)
            .map(|field| &field.value)
    }

    /// The focused field.
    pub fn focused(&self) -> &Field {
        &self.fields[self.focus]
    }

    /// Handles `key`: `tab`, `↓`, `shift-tab` and `↑` move between fields and `enter` sends;
    /// other keys edit the focused field.
    pub fn key(&mut self, key: KeyEvent) -> Outcome {
        let plain = !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        let count = self.fields.len();
        match key.code {
            KeyCode::Enter if plain => return Outcome::Submit,
            KeyCode::Tab | KeyCode::Down if plain => self.focus = (self.focus + 1) % count,
            KeyCode::BackTab | KeyCode::Up => self.focus = (self.focus + count - 1) % count,
            _ => match &mut self.fields[self.focus].value {
                Value::Text(input) => {
                    input.key(key);
                }
                Value::Choice { options, chosen } => {
                    let count = options.len().max(1);
                    match key.code {
                        KeyCode::Left | KeyCode::Char('h') => {
                            *chosen = (*chosen + count - 1) % count;
                        }
                        KeyCode::Right | KeyCode::Char('l') | KeyCode::Char(' ') => {
                            *chosen = (*chosen + 1) % count;
                        }
                        _ => {}
                    }
                }
                Value::Days { on, cursor } => match key.code {
                    KeyCode::Left | KeyCode::Char('h') => *cursor = (*cursor + 6) % 7,
                    KeyCode::Right | KeyCode::Char('l') => *cursor = (*cursor + 1) % 7,
                    KeyCode::Char(' ') | KeyCode::Char('x') => on[*cursor] = !on[*cursor],
                    _ => {}
                },
            },
        }
        Outcome::Edited
    }

    /// Types pasted text into the focused field, if it takes text.
    pub fn paste(&mut self, text: &str) {
        if let Value::Text(input) = &mut self.fields[self.focus].value {
            input.insert(text);
        }
    }

    /// The keys that work in the focused field, for the hint bar, before `enter` and `esc`.
    pub fn hints(&self) -> &'static [(&'static str, &'static str)] {
        match self.focused().value {
            Value::Text(_) => &[("tab", "next")],
            Value::Choice { .. } => &[("←/→", "change"), ("tab", "next")],
            Value::Days { .. } => &[("←/→", "day"), ("space", "on/off"), ("tab", "next")],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(form: &mut Form, code: KeyCode) -> Outcome {
        form.key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn form() -> Form {
        Form::new(vec![
            Field::text("Name", "", "anything"),
            Field::choice("Action", vec!["stop".to_owned(), "remove".to_owned()], 0),
            Field::days("Days", [false; 7]),
        ])
    }

    #[test]
    fn keys_move_between_fields_and_edit_the_focused_one() {
        let mut form = form();
        for c in "ada".chars() {
            press(&mut form, KeyCode::Char(c));
        }
        form.paste(" lovelace");
        assert_eq!(form.text("Name"), "ada lovelace");
        press(&mut form, KeyCode::Tab);
        press(&mut form, KeyCode::Right);
        assert_eq!(form.chosen("Action"), 1);
        press(&mut form, KeyCode::Right);
        assert_eq!(form.chosen("Action"), 0);
        press(&mut form, KeyCode::Left);
        assert_eq!(form.chosen("Action"), 1);
        form.paste("ignored");
        press(&mut form, KeyCode::Down);
        press(&mut form, KeyCode::Char(' '));
        press(&mut form, KeyCode::Left);
        press(&mut form, KeyCode::Char(' '));
        let mut expected = [false; 7];
        expected[0] = true;
        expected[6] = true;
        assert_eq!(form.days("Days"), expected);
        assert_eq!(form.hints()[0].0, "←/→");
        press(&mut form, KeyCode::Tab);
        assert_eq!(form.focus, 0);
        press(&mut form, KeyCode::BackTab);
        assert_eq!(form.focus, 2);
        assert_eq!(press(&mut form, KeyCode::Enter), Outcome::Submit);
    }
}

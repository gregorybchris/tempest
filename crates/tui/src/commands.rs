//! Every command and the keys bound to it. The key handling, the hint bar, the help overlay and
//! the `:` palette all read this one table.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::Area;

/// Something the user can do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[allow(missing_docs, reason = "`Command::label` describes each")]
pub enum Command {
    Down,
    Up,
    First,
    Last,
    PageDown,
    PageUp,

    Add,
    StartStop,
    ForceStart,
    Remove,
    Rename,
    Tags,
    Category,
    Recheck,
    MoveStorage,
    Open,

    QueueUp,
    QueueDown,
    QueueTop,
    QueueBottom,

    Filter,
    StatusFilter,
    Sort,
    Reverse,
    ClearFilters,

    Palette,
    GoTransfers,
    GoShare,
    GoCatalog,
    GoSettings,
    NextArea,
    PreviousArea,
    Theme,
    Help,
    Quit,
}

/// A group of commands, as help lists them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    /// Moving the cursor in the list.
    Move,
    /// Acting on the selected transfer, or adding one.
    Transfer,
    /// Reordering the queue.
    Queue,
    /// Filtering and sorting.
    Find,
    /// Areas, theme, help: what works everywhere.
    App,
}

impl Section {
    /// Its heading.
    pub fn title(self) -> &'static str {
        match self {
            Self::Move => "Move",
            Self::Transfer => "Transfer",
            Self::Queue => "Queue",
            Self::Find => "Find",
            Self::App => "App",
        }
    }

    /// Whether its commands work in `area`.
    pub fn works_in(self, area: Area) -> bool {
        self == Self::App || area == Area::Transfers
    }
}

impl Command {
    /// Every command, in the order help and the palette list them.
    pub const ALL: [Self; 35] = [
        Self::Down,
        Self::Up,
        Self::First,
        Self::Last,
        Self::PageDown,
        Self::PageUp,
        Self::Add,
        Self::StartStop,
        Self::ForceStart,
        Self::Remove,
        Self::Rename,
        Self::Tags,
        Self::Category,
        Self::Recheck,
        Self::MoveStorage,
        Self::Open,
        Self::QueueUp,
        Self::QueueDown,
        Self::QueueTop,
        Self::QueueBottom,
        Self::Filter,
        Self::StatusFilter,
        Self::Sort,
        Self::Reverse,
        Self::ClearFilters,
        Self::Palette,
        Self::GoTransfers,
        Self::GoShare,
        Self::GoCatalog,
        Self::GoSettings,
        Self::NextArea,
        Self::PreviousArea,
        Self::Theme,
        Self::Help,
        Self::Quit,
    ];

    /// The keys bound to it, the first being the one shown.
    pub fn keys(self) -> &'static [KeyCode] {
        use KeyCode::*;
        match self {
            Self::Down => &[Char('j'), KeyCode::Down],
            Self::Up => &[Char('k'), KeyCode::Up],
            Self::First => &[Char('g'), Home],
            Self::Last => &[Char('G'), End],
            Self::PageDown => &[KeyCode::PageDown],
            Self::PageUp => &[KeyCode::PageUp],
            Self::Add => &[Char('a')],
            Self::StartStop => &[Char(' ')],
            Self::ForceStart => &[Char('F')],
            Self::Remove => &[Char('x'), Delete],
            Self::Rename => &[Char('r')],
            Self::Tags => &[Char('t')],
            Self::Category => &[Char('C')],
            Self::Recheck => &[Char('R')],
            Self::MoveStorage => &[Char('m')],
            Self::Open => &[Char('o')],
            Self::QueueUp => &[Char('K')],
            Self::QueueDown => &[Char('J')],
            Self::QueueTop => &[Char('<')],
            Self::QueueBottom => &[Char('>')],
            Self::Filter => &[Char('/')],
            Self::StatusFilter => &[Char('f')],
            Self::Sort => &[Char('s')],
            Self::Reverse => &[Char('S')],
            Self::ClearFilters => &[Esc],
            Self::Palette => &[Char(':')],
            Self::GoTransfers => &[Char('1')],
            Self::GoShare => &[Char('2')],
            Self::GoCatalog => &[Char('3')],
            Self::GoSettings => &[Char('4')],
            Self::NextArea => &[Char(']')],
            Self::PreviousArea => &[Char('[')],
            Self::Theme => &[Char('T')],
            Self::Help => &[Char('?')],
            Self::Quit => &[Char('q')],
        }
    }

    /// What it does, for help and the palette.
    pub fn label(self) -> &'static str {
        match self {
            Self::Down => "Move down",
            Self::Up => "Move up",
            Self::First => "Go to the first",
            Self::Last => "Go to the last",
            Self::PageDown => "Page down",
            Self::PageUp => "Page up",
            Self::Add => "Add a link",
            Self::StartStop => "Start or stop",
            Self::ForceStart => "Force start",
            Self::Remove => "Remove",
            Self::Rename => "Rename",
            Self::Tags => "Edit tags",
            Self::Category => "Set the category",
            Self::Recheck => "Recheck its data",
            Self::MoveStorage => "Move its files",
            Self::Open => "Open its folder",
            Self::QueueUp => "Up the queue",
            Self::QueueDown => "Down the queue",
            Self::QueueTop => "Top of the queue",
            Self::QueueBottom => "Bottom of the queue",
            Self::Filter => "Filter by name or #tag",
            Self::StatusFilter => "Filter by status",
            Self::Sort => "Sort by…",
            Self::Reverse => "Reverse the order",
            Self::ClearFilters => "Clear the filters",
            Self::Palette => "Commands",
            Self::GoTransfers => "Go to Transfers",
            Self::GoShare => "Go to Share",
            Self::GoCatalog => "Go to Catalog",
            Self::GoSettings => "Go to Settings",
            Self::NextArea => "Next area",
            Self::PreviousArea => "Previous area",
            Self::Theme => "Next theme",
            Self::Help => "Help",
            Self::Quit => "Quit",
        }
    }

    /// The keys and a word or two, as the hint bar shows it, if it ever does.
    pub fn hint(self) -> Option<(&'static str, &'static str)> {
        Some(match self {
            Self::Down => ("j/k", "move"),
            Self::StartStop => ("space", "start/stop"),
            Self::Add => ("a", "add"),
            Self::Remove => ("x", "remove"),
            Self::Filter => ("/", "filter"),
            Self::Sort => ("s", "sort"),
            Self::ClearFilters => ("esc", "clear filters"),
            Self::Palette => (":", "command"),
            Self::GoTransfers => ("1-4", "areas"),
            Self::Theme => ("T", "theme"),
            Self::Quit => ("q", "quit"),
            Self::Help => ("?", "help"),
            _ => return None,
        })
    }

    /// Its line in help, as keys and what they do. Pairs of commands share a line, which the
    /// first of them gives; the second gives none.
    pub fn help_line(self) -> Option<(String, &'static str)> {
        let pair = |keys: &str, label| Some((keys.to_owned(), label));
        match self {
            Self::Down => pair("j/k", "Move down/up"),
            Self::First => pair("g/G", "First/last"),
            Self::PageDown => pair("pgdn/pgup", "Page down/up"),
            Self::QueueUp => pair("K/J", "Up/down the queue"),
            Self::QueueTop => pair("</>", "Top/bottom of the queue"),
            Self::GoTransfers => pair("1-4", "Go to an area"),
            Self::NextArea => pair("]/[", "Next/previous area"),
            Self::Up
            | Self::Last
            | Self::PageUp
            | Self::QueueDown
            | Self::QueueBottom
            | Self::GoShare
            | Self::GoCatalog
            | Self::GoSettings
            | Self::PreviousArea => None,
            _ => Some((self.key_label(), self.label())),
        }
    }

    /// Its group.
    pub fn section(self) -> Section {
        match self {
            Self::Down | Self::Up | Self::First | Self::Last | Self::PageDown | Self::PageUp => {
                Section::Move
            }
            Self::Add
            | Self::StartStop
            | Self::ForceStart
            | Self::Remove
            | Self::Rename
            | Self::Tags
            | Self::Category
            | Self::Recheck
            | Self::MoveStorage
            | Self::Open => Section::Transfer,
            Self::QueueUp | Self::QueueDown | Self::QueueTop | Self::QueueBottom => Section::Queue,
            Self::Filter | Self::StatusFilter | Self::Sort | Self::Reverse | Self::ClearFilters => {
                Section::Find
            }
            _ => Section::App,
        }
    }

    /// Whether the palette offers it. Moving the cursor is for keys alone.
    pub fn in_palette(self) -> bool {
        self.section() != Section::Move && self != Self::Palette
    }

    /// Its keys as help shows them: "x del".
    pub fn key_label(self) -> String {
        self.keys()
            .iter()
            .map(|code| key_name(*code))
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// The command `key` runs in `area`, if any.
    pub fn for_key(area: Area, key: KeyEvent) -> Option<Self> {
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return None;
        }
        Self::ALL
            .into_iter()
            .find(|command| command.section().works_in(area) && command.keys().contains(&key.code))
    }
}

/// A key's name, as help shows it.
fn key_name(code: KeyCode) -> String {
    match code {
        KeyCode::Char(' ') => "space".to_owned(),
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Up => "↑".to_owned(),
        KeyCode::Down => "↓".to_owned(),
        KeyCode::Home => "home".to_owned(),
        KeyCode::End => "end".to_owned(),
        KeyCode::PageUp => "pgup".to_owned(),
        KeyCode::PageDown => "pgdn".to_owned(),
        KeyCode::Delete => "del".to_owned(),
        KeyCode::Esc => "esc".to_owned(),
        other => other.to_string().to_lowercase(),
    }
}

/// How well `query` matches `label`, lower being better, or `None` if it doesn't: the label
/// starting with it, then a word starting with it, then containing it, then containing its
/// letters in order.
pub fn match_score(query: &str, label: &str) -> Option<usize> {
    let query = query.trim().to_lowercase();
    let label = label.to_lowercase();
    if query.is_empty() || label.starts_with(&query) {
        return Some(0);
    }
    if label
        .split(|c: char| !c.is_alphanumeric())
        .any(|word| word.starts_with(&query))
    {
        return Some(1);
    }
    if label.contains(&query) {
        return Some(2);
    }
    // Letters in order, scored by how spread out they are.
    let mut letters = label.char_indices();
    let mut first = None;
    let mut last = 0;
    for wanted in query.chars().filter(|c| !c.is_whitespace()) {
        let (index, _) = letters.find(|(_, c)| *c == wanted)?;
        first.get_or_insert(index);
        last = index;
    }
    Some(3 + last - first.unwrap_or(0))
}

/// The palette's commands for `area` matching `query`, best first.
pub fn palette_matches(area: Area, query: &str) -> Vec<Command> {
    let mut matches: Vec<(usize, usize, Command)> = Command::ALL
        .into_iter()
        .enumerate()
        .filter(|(_, command)| command.in_palette() && command.section().works_in(area))
        .filter_map(|(index, command)| Some((match_score(query, command.label())?, index, command)))
        .collect();
    matches.sort_by_key(|(score, index, _)| (*score, *index));
    matches.into_iter().map(|(_, _, command)| command).collect()
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn no_two_commands_share_a_key_where_both_work() {
        for area in Area::ALL {
            let mut seen = HashSet::new();
            for command in Command::ALL {
                if !command.section().works_in(area) {
                    continue;
                }
                for key in command.keys() {
                    assert!(seen.insert(*key), "{key:?} is bound twice in {area:?}");
                }
            }
        }
    }

    #[test]
    fn keys_find_their_commands_in_their_areas() {
        let key = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
        assert_eq!(
            Command::for_key(Area::Transfers, key('x')),
            Some(Command::Remove)
        );
        assert_eq!(Command::for_key(Area::Share, key('x')), None);
        assert_eq!(
            Command::for_key(Area::Share, key('T')),
            Some(Command::Theme)
        );
        let shifted = KeyEvent::new(KeyCode::Char('K'), KeyModifiers::SHIFT);
        assert_eq!(
            Command::for_key(Area::Transfers, shifted),
            Some(Command::QueueUp)
        );
        let ctrl = KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL);
        assert_eq!(Command::for_key(Area::Transfers, ctrl), None);
        assert_eq!(Command::Remove.key_label(), "x del");
    }

    #[test]
    fn hints_show_a_bound_key() {
        for command in Command::ALL {
            if let Some((keys, _)) = command.hint() {
                assert!(
                    command.key_label().split(' ').any(|k| keys.starts_with(k)),
                    "{command:?} hints {keys} but is bound to {}",
                    command.key_label()
                );
            }
        }
    }

    #[test]
    fn help_shows_a_bound_key() {
        for command in Command::ALL {
            if let Some((keys, _)) = command.help_line() {
                assert!(
                    command.key_label().split(' ').any(|k| keys.starts_with(k)),
                    "{command:?} shows {keys} but is bound to {}",
                    command.key_label()
                );
            }
        }
    }

    #[test]
    fn the_palette_ranks_closer_matches_first() {
        assert_eq!(palette_matches(Area::Transfers, "").len(), 28);
        assert_eq!(palette_matches(Area::Transfers, "rem")[0], Command::Remove);
        assert_eq!(palette_matches(Area::Transfers, "tags")[0], Command::Tags);
        assert_eq!(
            palette_matches(Area::Transfers, "queue")[0],
            Command::QueueUp
        );
        assert_eq!(
            palette_matches(Area::Transfers, "fbs"),
            [Command::StatusFilter]
        );
        assert!(palette_matches(Area::Transfers, "zzz").is_empty());
        assert_eq!(palette_matches(Area::Share, "rem"), []);
        assert_eq!(match_score("set cat", "Set the category"), Some(3 + 10));
    }
}

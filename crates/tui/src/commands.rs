//! Every command and the keys bound to it. The key handling, the hint bar, the help overlay and
//! the `:` palette all read this one table.
//!
//! A key can mean different things on different screens, as `g` does: the first transfer in the
//! list, and the rates chart from a transfer's detail (D-194).

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::Screen;

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
    Copy,
    Limits,
    Unpublish,

    QueueUp,
    QueueDown,
    QueueTop,
    QueueBottom,

    Detail,
    NextTab,
    PreviousTab,
    Chunks,
    Rates,
    Back,

    Filter,
    StatusFilter,
    Sort,
    Reverse,
    ClearFilters,
    Views,
    SaveView,
    Columns,

    NewShare,
    ShowShare,

    Change,
    AddEntry,
    RemoveEntry,
    NextSection,
    PreviousSection,

    Palette,
    GoTransfers,
    GoShare,
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
    /// A transfer's detail, chunk map and rates chart.
    View,
    /// Filtering, sorting, saved views and columns.
    Find,
    /// The Share area's form and shares.
    Share,
    /// Changing what the Settings area lists.
    Change,
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
            Self::View => "Detail",
            Self::Find => "Find",
            Self::Share => "Share",
            Self::Change => "Change",
            Self::App => "App",
        }
    }
}

impl Command {
    /// Every command, in the order help and the palette list them.
    pub const ALL: [Self; 53] = [
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
        Self::Copy,
        Self::Limits,
        Self::Unpublish,
        Self::QueueUp,
        Self::QueueDown,
        Self::QueueTop,
        Self::QueueBottom,
        Self::Detail,
        Self::NextTab,
        Self::PreviousTab,
        Self::Chunks,
        Self::Rates,
        Self::Back,
        Self::Filter,
        Self::StatusFilter,
        Self::Sort,
        Self::Reverse,
        Self::ClearFilters,
        Self::Views,
        Self::SaveView,
        Self::Columns,
        Self::NewShare,
        Self::ShowShare,
        Self::Change,
        Self::AddEntry,
        Self::RemoveEntry,
        Self::NextSection,
        Self::PreviousSection,
        Self::Palette,
        Self::GoTransfers,
        Self::GoShare,
        Self::GoSettings,
        Self::NextArea,
        Self::PreviousArea,
        Self::Theme,
        Self::Help,
        Self::Quit,
    ];

    /// Whether it works on `screen`.
    pub fn works_in(self, screen: Screen) -> bool {
        let deeper = matches!(screen, Screen::Detail | Screen::Chunks | Screen::Rates);
        let rows = screen == Screen::Settings;
        match self {
            Self::Detail => screen == Screen::List,
            Self::NextTab | Self::PreviousTab => screen == Screen::Detail,
            Self::Chunks => matches!(screen, Screen::Detail | Screen::Rates),
            Self::Rates => matches!(screen, Screen::Detail | Screen::Chunks),
            Self::Back => deeper,
            Self::Copy => matches!(screen, Screen::List | Screen::Detail | Screen::Share),
            Self::AddEntry => screen == Screen::Settings,
            _ => match self.section() {
                Section::Move => !matches!(screen, Screen::Chunks | Screen::Rates),
                Section::Transfer | Section::Queue => {
                    matches!(screen, Screen::List | Screen::Detail)
                }
                Section::Find => screen == Screen::List,
                Section::View => deeper,
                Section::Share => screen == Screen::Share,
                Section::Change => rows,
                Section::App => true,
            },
        }
    }

    /// The keys bound to it on `screen`, the first being the one shown.
    pub fn keys(self, screen: Screen) -> &'static [KeyCode] {
        use KeyCode::*;
        match self {
            Self::Down => &[Char('j'), Down],
            Self::Up => &[Char('k'), Up],
            // In the detail, `g` opens the rates chart.
            Self::First if screen == Screen::Detail => &[Home],
            Self::First => &[Char('g'), Home],
            Self::Last => &[Char('G'), End],
            Self::PageDown => &[PageDown],
            Self::PageUp => &[PageUp],
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
            Self::Copy => &[Char('y')],
            Self::Limits => &[Char('L')],
            Self::Unpublish => &[Char('U')],
            Self::QueueUp => &[Char('K')],
            Self::QueueDown => &[Char('J')],
            Self::QueueTop => &[Char('<')],
            Self::QueueBottom => &[Char('>')],
            Self::Detail => &[Enter],
            Self::NextTab => &[Tab, Char('l'), Right],
            Self::PreviousTab => &[BackTab, Char('h'), Left],
            Self::Chunks => &[Char('c')],
            Self::Rates => &[Char('g')],
            Self::Back => &[Esc],
            Self::Filter => &[Char('/')],
            Self::StatusFilter => &[Char('f')],
            Self::Sort => &[Char('s')],
            Self::Reverse => &[Char('S')],
            Self::ClearFilters => &[Esc],
            Self::Views => &[Char('v')],
            Self::SaveView => &[Char('V')],
            Self::Columns => &[Char('|')],
            Self::NewShare => &[Char('a')],
            Self::ShowShare => &[Enter],
            Self::Change => &[Enter, Char(' ')],
            Self::AddEntry => &[Char('a')],
            Self::RemoveEntry => &[Char('x'), Delete],
            Self::NextSection => &[Tab],
            Self::PreviousSection => &[BackTab],
            Self::Palette => &[Char(':')],
            Self::GoTransfers => &[Char('1')],
            Self::GoShare => &[Char('2')],
            Self::GoSettings => &[Char('3')],
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
            Self::Copy => "Copy its link",
            Self::Limits => "Speed and sharing limits",
            Self::Unpublish => "Unpublish from a tracker",
            Self::QueueUp => "Up the queue",
            Self::QueueDown => "Down the queue",
            Self::QueueTop => "Top of the queue",
            Self::QueueBottom => "Bottom of the queue",
            Self::Detail => "Show its detail",
            Self::NextTab => "Next tab",
            Self::PreviousTab => "Previous tab",
            Self::Chunks => "Chunk map",
            Self::Rates => "Rates chart",
            Self::Back => "Back",
            Self::Filter => "Filter by name or #tag",
            Self::StatusFilter => "Filter by status",
            Self::Sort => "Sort by…",
            Self::Reverse => "Reverse the order",
            Self::ClearFilters => "Clear the filters",
            Self::Views => "Saved views",
            Self::SaveView => "Save this view",
            Self::Columns => "Choose columns",
            Self::NewShare => "Share a file or folder",
            Self::ShowShare => "Show it in Transfers",
            Self::Change => "Change",
            Self::AddEntry => "Add a tracker, relay or category",
            Self::RemoveEntry => "Remove, or reset to the file",
            Self::NextSection => "Next section",
            Self::PreviousSection => "Previous section",
            Self::Palette => "Commands",
            Self::GoTransfers => "Go to Transfers",
            Self::GoShare => "Go to Share",
            Self::GoSettings => "Go to Settings",
            Self::NextArea => "Next area",
            Self::PreviousArea => "Previous area",
            Self::Theme => "Next theme",
            Self::Help => "Help",
            Self::Quit => "Quit",
        }
    }

    /// The keys and a word or two, as the hint bar shows it on `screen`, if it ever does.
    pub fn hint(self, screen: Screen) -> Option<(&'static str, &'static str)> {
        Some(match self {
            Self::Down if screen == Screen::Detail => ("j/k", "scroll"),
            Self::Down => ("j/k", "move"),
            Self::Copy => ("y", "copy link"),
            Self::Views => ("v", "views"),
            Self::NewShare => ("a", "share a path"),
            Self::ShowShare => ("enter", "show"),
            Self::Change => ("enter", "change"),
            Self::AddEntry => ("a", "add"),
            Self::RemoveEntry => ("x", "remove/reset"),
            Self::NextSection => ("tab", "sections"),
            Self::Detail => ("enter", "detail"),
            Self::NextTab => ("h/l", "tabs"),
            Self::Chunks => ("c", "chunks"),
            Self::Rates => ("g", "rates"),
            Self::Back => ("esc", "back"),
            Self::StartStop => ("space", "start/stop"),
            Self::Add => ("a", "add"),
            Self::Remove => ("x", "remove"),
            Self::Filter => ("/", "filter"),
            Self::Sort => ("s", "sort"),
            Self::ClearFilters => ("esc", "clear filters"),
            Self::Palette => (":", "command"),
            Self::GoTransfers => ("1-3", "areas"),
            Self::Theme => ("T", "theme"),
            Self::Quit => ("q", "quit"),
            Self::Help => ("?", "help"),
            _ => return None,
        })
    }

    /// Its line in help on `screen`, as keys and what they do. Pairs of commands share a line,
    /// which the first of them gives; the second gives none.
    pub fn help_line(self, screen: Screen) -> Option<(String, &'static str)> {
        let pair = |keys: &str, label| Some((keys.to_owned(), label));
        match self {
            Self::Down if screen == Screen::Detail => pair("j/k", "Scroll down/up"),
            Self::Down => pair("j/k", "Move down/up"),
            Self::First if screen == Screen::Detail => pair("home/end", "Top/bottom"),
            Self::First => pair("g/G", "First/last"),
            Self::NextTab => pair("tab h/l", "Next/previous tab"),
            Self::PageDown => pair("pgdn/pgup", "Page down/up"),
            Self::QueueUp => pair("K/J", "Up/down the queue"),
            Self::QueueTop => pair("</>", "Top/bottom of the queue"),
            Self::GoTransfers => pair("1-3", "Go to an area"),
            Self::NextSection => pair("tab ⇧tab", "Next/previous section"),
            Self::NextArea => pair("]/[", "Next/previous area"),
            Self::Up
            | Self::Last
            | Self::PageUp
            | Self::QueueDown
            | Self::QueueBottom
            | Self::PreviousTab
            | Self::GoShare
            | Self::GoSettings
            | Self::PreviousArea
            | Self::PreviousSection => None,
            _ => Some((self.key_label(screen), self.label())),
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
            | Self::Open
            | Self::Copy
            | Self::Limits
            | Self::Unpublish => Section::Transfer,
            Self::QueueUp | Self::QueueDown | Self::QueueTop | Self::QueueBottom => Section::Queue,
            Self::Detail
            | Self::NextTab
            | Self::PreviousTab
            | Self::Chunks
            | Self::Rates
            | Self::Back => Section::View,
            Self::Filter
            | Self::StatusFilter
            | Self::Sort
            | Self::Reverse
            | Self::ClearFilters
            | Self::Views
            | Self::SaveView
            | Self::Columns => Section::Find,
            Self::NewShare | Self::ShowShare => Section::Share,
            Self::Change
            | Self::AddEntry
            | Self::RemoveEntry
            | Self::NextSection
            | Self::PreviousSection => Section::Change,
            _ => Section::App,
        }
    }

    /// Whether the palette offers it. Moving the cursor is for keys alone.
    pub fn in_palette(self) -> bool {
        self.section() != Section::Move && self != Self::Palette
    }

    /// Its keys on `screen` as help shows them: "x del".
    pub fn key_label(self, screen: Screen) -> String {
        self.keys(screen)
            .iter()
            .map(|code| key_name(*code))
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// The command `key` runs on `screen`, if any.
    pub fn for_key(screen: Screen, key: KeyEvent) -> Option<Self> {
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return None;
        }
        Self::ALL
            .into_iter()
            .find(|command| command.works_in(screen) && command.keys(screen).contains(&key.code))
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
        KeyCode::Enter => "enter".to_owned(),
        KeyCode::Tab => "tab".to_owned(),
        KeyCode::BackTab => "⇧tab".to_owned(),
        KeyCode::Left => "←".to_owned(),
        KeyCode::Right => "→".to_owned(),
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

/// The palette's commands for `screen` matching `query`, best first.
pub fn palette_matches(screen: Screen, query: &str) -> Vec<Command> {
    let mut matches: Vec<(usize, usize, Command)> = Command::ALL
        .into_iter()
        .enumerate()
        .filter(|(_, command)| command.in_palette() && command.works_in(screen))
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
        for screen in Screen::ALL {
            let mut seen = HashSet::new();
            for command in Command::ALL {
                if !command.works_in(screen) {
                    continue;
                }
                for key in command.keys(screen) {
                    assert!(seen.insert(*key), "{key:?} is bound twice on {screen:?}");
                }
            }
        }
    }

    #[test]
    fn g_goes_to_the_top_of_the_list_and_to_the_rates_from_the_detail() {
        let g = KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE);
        assert_eq!(Command::for_key(Screen::List, g), Some(Command::First));
        assert_eq!(Command::for_key(Screen::Detail, g), Some(Command::Rates));
        assert_eq!(Command::for_key(Screen::Chunks, g), Some(Command::Rates));
        let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(
            Command::for_key(Screen::List, esc),
            Some(Command::ClearFilters)
        );
        assert_eq!(Command::for_key(Screen::Rates, esc), Some(Command::Back));
    }

    #[test]
    fn keys_find_their_commands_in_their_areas() {
        let key = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
        assert_eq!(
            Command::for_key(Screen::List, key('x')),
            Some(Command::Remove)
        );
        assert_eq!(Command::for_key(Screen::Share, key('x')), None);
        assert_eq!(
            Command::for_key(Screen::Settings, key('x')),
            Some(Command::RemoveEntry)
        );
        assert_eq!(
            Command::for_key(Screen::Share, key('T')),
            Some(Command::Theme)
        );
        assert_eq!(
            Command::for_key(Screen::Share, key('a')),
            Some(Command::NewShare)
        );
        assert_eq!(
            Command::for_key(Screen::Settings, key('a')),
            Some(Command::AddEntry)
        );
        assert_eq!(
            Command::for_key(Screen::Settings, key(' ')),
            Some(Command::Change)
        );
        let shifted = KeyEvent::new(KeyCode::Char('K'), KeyModifiers::SHIFT);
        assert_eq!(
            Command::for_key(Screen::List, shifted),
            Some(Command::QueueUp)
        );
        let ctrl = KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL);
        assert_eq!(Command::for_key(Screen::List, ctrl), None);
        assert_eq!(Command::Remove.key_label(Screen::List), "x del");
    }

    #[test]
    fn hints_and_help_show_a_bound_key() {
        for screen in Screen::ALL {
            for command in Command::ALL.into_iter().filter(|c| c.works_in(screen)) {
                let label = command.key_label(screen);
                let hint = command.hint(screen).map(|(keys, _)| keys.to_owned());
                let help = command.help_line(screen).map(|(keys, _)| keys);
                for keys in hint.into_iter().chain(help) {
                    // Paired keys, as in "h/l", need only one bound to the command.
                    let bound = |shown: &str| label.split(' ').any(|k| shown.starts_with(k));
                    assert!(
                        bound(&keys) || keys.split('/').any(bound),
                        "{command:?} shows {keys} on {screen:?} but is bound to {label}"
                    );
                }
            }
        }
    }

    #[test]
    fn the_palette_ranks_closer_matches_first() {
        assert_eq!(palette_matches(Screen::List, "").len(), 34);
        assert_eq!(palette_matches(Screen::List, "rem")[0], Command::Remove);
        assert_eq!(palette_matches(Screen::List, "tags")[0], Command::Tags);
        assert_eq!(palette_matches(Screen::List, "queue")[0], Command::QueueUp);
        assert_eq!(
            palette_matches(Screen::List, "fbs"),
            [Command::StatusFilter]
        );
        assert!(palette_matches(Screen::List, "zzz").is_empty());
        assert_eq!(palette_matches(Screen::Share, "rem"), []);
        assert_eq!(
            palette_matches(Screen::Settings, "rem")[0],
            Command::RemoveEntry
        );
        assert_eq!(palette_matches(Screen::Detail, "chunk")[0], Command::Chunks);
        assert_eq!(match_score("set cat", "Set the category"), Some(3 + 10));
    }
}

//! The path picker's logic, apart from the DOM (D-258): which entries show, where the cursor
//! is, what each key does, where moving goes, and what choosing returns. The view in
//! [`crate::picker`] draws this state and sends it the keys.

use tempest_api::folders::{EntryInfo, EntryKind, FolderInfo};
use tempest_api::units::date;

use crate::paths;

/// What the picker picks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// A file.
    File,
    /// A folder.
    Folder,
    /// A file or a folder, as sharing takes.
    Any,
}

impl Mode {
    /// Whether an entry of `kind` can be chosen.
    pub fn takes(self, kind: EntryKind) -> bool {
        match self {
            Mode::File => kind == EntryKind::File,
            Mode::Folder => kind == EntryKind::Folder,
            Mode::Any => true,
        }
    }
}

/// What a path picked from the picker is for, which words its title and footer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Purpose {
    /// A file or folder to share.
    Share,
    /// The folder a new download goes into.
    Download,
    /// The folder a transfer's files move into.
    Move,
    /// The engine's default save folder.
    SaveFolder,
    /// A category's save folder.
    Category,
}

impl Purpose {
    /// What it picks.
    pub fn mode(self) -> Mode {
        match self {
            Purpose::Share => Mode::Any,
            _ => Mode::Folder,
        }
    }

    /// The picker's title.
    pub fn title(self) -> &'static str {
        match self {
            Purpose::Share => "Choose what to share",
            Purpose::Download => "Choose where it downloads",
            Purpose::Move => "Choose where its files move",
            Purpose::SaveFolder => "Choose the save folder",
            Purpose::Category => "Choose the category's folder",
        }
    }

    /// What choosing does to a path, as the footer says it: "Share", then the path.
    pub fn verb(self) -> &'static str {
        match self {
            Purpose::Share => "Share",
            Purpose::Download => "Download into",
            Purpose::Move => "Move the files into",
            Purpose::SaveFolder | Purpose::Category => "Save downloads in",
        }
    }

    /// The kind of field, for remembering the last folder each used: sharing picks from
    /// one place, and folders for files to go to from another.
    pub fn memory(self) -> &'static str {
        match self {
            Purpose::Share => "share",
            _ => "folder",
        }
    }
}

/// A move of the cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Down one.
    Down,
    /// Up one.
    Up,
    /// Down a page.
    PageDown,
    /// Up a page.
    PageUp,
    /// To the first entry.
    First,
    /// To the last entry.
    Last,
}

/// Where a folder couldn't be read, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    /// The folder.
    pub path: String,
    /// The API's error code, such as `permission_denied`.
    pub code: String,
    /// What went wrong, in a sentence.
    pub message: String,
}

/// What the picker ends with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    /// The path chosen.
    pub path: String,
    /// The folder it was chosen from: the folder it is in, or the folder itself when the
    /// current folder was chosen.
    pub from: String,
    /// Whether it is the current folder rather than an entry of it.
    pub current: bool,
}

/// What a key asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Show this folder.
    Open(String),
    /// Show this folder with the entry of this name under the cursor.
    Back(String, String),
    /// End with this choice.
    Choose(Choice),
}

/// The picker's state.
#[derive(Clone, Debug)]
pub struct Browser {
    mode: Mode,
    folder: Option<FolderInfo>,
    failure: Option<Failure>,
    filter: String,
    hidden: bool,
    /// The entries shown, as indices into the folder's.
    shown: Vec<usize>,
    /// How many of the shown entries the cursor can rest on: all of them, except that a
    /// folder picker shows its files but can't choose them. Folders come first, so these
    /// are the first ones.
    choosable: usize,
    /// The cursor's place among the shown entries.
    cursor: Option<usize>,
}

impl Browser {
    /// A picker of `mode` that has shown nothing yet, showing hidden entries if `hidden`.
    pub fn new(mode: Mode, hidden: bool) -> Self {
        Self {
            mode,
            folder: None,
            failure: None,
            filter: String::new(),
            hidden,
            shown: Vec::new(),
            choosable: 0,
            cursor: None,
        }
    }

    /// Shows `folder`, with the cursor on the entry named `select` if it can rest there.
    pub fn show(&mut self, folder: FolderInfo, select: Option<&str>) {
        self.folder = Some(folder);
        self.failure = None;
        self.filter.clear();
        self.cursor = None;
        self.refresh();
        if let Some(name) = select {
            self.cursor = self.position(name);
        }
    }

    /// Shows that `failure.path` couldn't be read.
    pub fn fail(&mut self, failure: Failure) {
        self.folder = None;
        self.failure = Some(failure);
        self.filter.clear();
        self.cursor = None;
        self.refresh();
    }

    /// The folder shown, if it could be read.
    pub fn folder(&self) -> Option<&FolderInfo> {
        self.folder.as_ref()
    }

    /// Why the folder couldn't be read, if it couldn't.
    pub fn failure(&self) -> Option<&Failure> {
        self.failure.as_ref()
    }

    /// The path the picker is at: the folder shown, or the one that couldn't be read.
    pub fn location(&self) -> Option<&str> {
        match (&self.folder, &self.failure) {
            (Some(folder), _) => Some(&folder.path),
            (None, Some(failure)) => Some(&failure.path),
            (None, None) => None,
        }
    }

    /// The text the entries are filtered by.
    pub fn filter(&self) -> &str {
        &self.filter
    }

    /// Filters the entries by `text`: those whose names contain it, case aside. The cursor
    /// stays on its entry if that still shows, and otherwise goes to the first match.
    pub fn set_filter(&mut self, text: &str) {
        if self.filter == text {
            return;
        }
        let kept = self.selected().map(|e| e.name.clone());
        self.filter = text.to_owned();
        self.refresh();
        self.cursor = kept.and_then(|name| self.position(&name));
        if self.cursor.is_none() && !self.filter.is_empty() && self.choosable > 0 {
            self.cursor = Some(0);
        }
    }

    /// Whether hidden entries show.
    pub fn hidden(&self) -> bool {
        self.hidden
    }

    /// Shows or hides hidden entries, keeping the cursor on its entry if it still shows.
    pub fn set_hidden(&mut self, hidden: bool) {
        let kept = self.selected().map(|e| e.name.clone());
        self.hidden = hidden;
        self.refresh();
        self.cursor = kept.and_then(|name| self.position(&name));
    }

    /// How many entries show.
    pub fn len(&self) -> usize {
        self.shown.len()
    }

    /// Whether the folder has entries the filters hide, but none show.
    pub fn all_filtered(&self) -> bool {
        self.shown.is_empty() && self.folder.as_ref().is_some_and(|f| !f.entries.is_empty())
    }

    /// The entry shown at `at`.
    pub fn entry(&self, at: usize) -> Option<&EntryInfo> {
        let folder = self.folder.as_ref()?;
        folder.entries.get(*self.shown.get(at)?)
    }

    /// The entry shown at `at`'s index in the folder, which identifies it while the folder
    /// shows.
    pub fn key(&self, at: usize) -> Option<usize> {
        self.shown.get(at).copied()
    }

    /// Whether the cursor can rest on the entry shown at `at`.
    pub fn choosable(&self, at: usize) -> bool {
        at < self.choosable
    }

    /// The cursor's place among the entries shown.
    pub fn cursor(&self) -> Option<usize> {
        self.cursor
    }

    /// The entry under the cursor.
    pub fn selected(&self) -> Option<&EntryInfo> {
        self.entry(self.cursor?)
    }

    /// Moves the cursor by `step`, a page being `page` entries. With no cursor, moving down
    /// starts at the top and moving up at the bottom.
    pub fn go(&mut self, step: Step, page: usize) {
        let Some(last) = self.choosable.checked_sub(1) else {
            return;
        };
        let page = page.max(1);
        self.cursor = Some(match (step, self.cursor) {
            (Step::First, _) | (Step::Down | Step::PageDown, None) => 0,
            (Step::Last, _) | (Step::Up | Step::PageUp, None) => last,
            (Step::Down, Some(i)) => (i + 1).min(last),
            (Step::Up, Some(i)) => i.saturating_sub(1).min(last),
            (Step::PageDown, Some(i)) => (i + page).min(last),
            (Step::PageUp, Some(i)) => i.saturating_sub(page).min(last),
        });
    }

    /// Puts the cursor on the entry shown at `at`, if it can rest there.
    pub fn select(&mut self, at: usize) {
        if self.choosable(at) {
            self.cursor = Some(at);
        }
    }

    /// Takes the cursor off every entry.
    pub fn deselect(&mut self) {
        self.cursor = None;
    }

    /// What `enter` does: opens a folder under the cursor, or chooses a file the picker
    /// takes.
    pub fn enter(&self) -> Option<Action> {
        let entry = self.selected()?;
        match entry.kind {
            EntryKind::Folder => self.open(),
            EntryKind::File => self.choice().map(Action::Choose),
        }
    }

    /// What `→` does: opens the folder under the cursor.
    pub fn open(&self) -> Option<Action> {
        let (folder, entry) = (self.folder.as_ref()?, self.selected()?);
        (entry.kind == EntryKind::Folder)
            .then(|| Action::Open(paths::join(&folder.path, &entry.name)))
    }

    /// What `←` does: goes to the folder this one is in, with this one under the cursor.
    pub fn up(&self) -> Option<Action> {
        let location = self.location()?;
        let parent = match &self.folder {
            Some(folder) => folder.parent.clone(),
            None => paths::parent(location),
        }?;
        let name = paths::name(location).unwrap_or_default().to_owned();
        Some(Action::Back(parent, name))
    }

    /// What choosing now would return: the entry under the cursor if the picker takes it, or
    /// else the folder shown, unless the picker wants a file.
    pub fn choice(&self) -> Option<Choice> {
        let folder = self.folder.as_ref()?;
        match self.selected() {
            Some(entry) if self.mode.takes(entry.kind) => Some(Choice {
                path: paths::join(&folder.path, &entry.name),
                from: folder.path.clone(),
                current: false,
            }),
            Some(_) => None,
            None => (self.mode != Mode::File).then(|| Choice {
                path: folder.path.clone(),
                from: folder.path.clone(),
                current: true,
            }),
        }
    }

    /// The place among the shown entries of the one named `name`, if the cursor can rest
    /// there.
    fn position(&self, name: &str) -> Option<usize> {
        let folder = self.folder.as_ref()?;
        let at = self
            .shown
            .iter()
            .position(|&i| folder.entries[i].name == name)?;
        self.choosable(at).then_some(at)
    }

    /// Works out the entries shown after a change of folder or filters.
    fn refresh(&mut self) {
        let Some(folder) = &self.folder else {
            self.shown.clear();
            self.choosable = 0;
            return;
        };
        let filter = self.filter.to_lowercase();
        self.shown = folder
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| self.hidden || !e.hidden)
            .filter(|(_, e)| filter.is_empty() || e.name.to_lowercase().contains(&filter))
            .map(|(i, _)| i)
            .collect();
        self.choosable = match self.mode {
            Mode::Folder => self
                .shown
                .iter()
                .take_while(|&&i| folder.entries[i].kind == EntryKind::Folder)
                .count(),
            Mode::File | Mode::Any => self.shown.len(),
        };
    }
}

/// A key press, as the picker's keyboard reads it.
#[derive(Clone, Copy, Debug, Default)]
pub struct Press<'a> {
    /// `KeyboardEvent.key`.
    pub key: &'a str,
    /// Whether Ctrl or Cmd was held.
    pub command: bool,
    /// Whether Alt was held.
    pub alt: bool,
    /// Whether the filter box has the focus.
    pub in_filter: bool,
    /// Whether the filter is empty.
    pub filter_empty: bool,
    /// Whether the filter box's caret is at the end of its text.
    pub at_end: bool,
}

/// What a key does in the picker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    /// Move the cursor.
    Move(Step),
    /// Open the folder under the cursor, or choose the file.
    Enter,
    /// Open the folder under the cursor.
    Open,
    /// Go to the folder this one is in.
    Up,
    /// Choose what the footer says.
    Choose,
    /// Show or hide hidden entries.
    Hidden,
    /// Type a path, starting with this character if any.
    EditPath(Option<char>),
    /// Clear the filter, or close the picker.
    Escape,
    /// Add a character to the filter.
    Type(char),
    /// Take the last character off the filter.
    Erase,
}

/// What `press` does, if anything. Keys the filter box needs for editing stay its own.
pub fn command(press: Press) -> Option<Command> {
    let Press {
        key,
        command,
        alt,
        in_filter,
        filter_empty,
        at_end,
    } = press;
    if command {
        return match key {
            "l" | "L" => Some(Command::EditPath(None)),
            "Enter" => Some(Command::Choose),
            _ => None,
        };
    }
    let in_list = !in_filter;
    Some(match key {
        "ArrowDown" => Command::Move(Step::Down),
        "ArrowUp" => Command::Move(Step::Up),
        "PageDown" => Command::Move(Step::PageDown),
        "PageUp" => Command::Move(Step::PageUp),
        "Home" if in_list => Command::Move(Step::First),
        "End" if in_list => Command::Move(Step::Last),
        "Enter" => Command::Enter,
        "ArrowRight" if in_list || at_end => Command::Open,
        "ArrowLeft" if in_list || filter_empty => Command::Up,
        "Backspace" if filter_empty => Command::Up,
        "Backspace" if in_list => Command::Erase,
        "Escape" => Command::Escape,
        "." if in_list && filter_empty => Command::Hidden,
        "/" | "~" if in_list && filter_empty => Command::EditPath(key.chars().next()),
        _ if in_list && !alt && key.chars().count() == 1 => Command::Type(key.chars().next()?),
        _ => return None,
    })
}

/// The keys the footer lists, with what they do, written with ⌘ on a Mac.
pub fn hints(mac: bool) -> [(&'static str, &'static str); 8] {
    let (choose, path) = match mac {
        true => ("⌘enter", "⌘L"),
        false => ("ctrl+enter", "ctrl+L"),
    };
    [
        ("↑↓", "move"),
        ("→", "open"),
        ("←", "parent folder"),
        ("enter", "open or choose"),
        (choose, "choose"),
        ("type", "filter"),
        (".", "show hidden"),
        (path, "type path"),
    ]
}

/// When an entry last changed, in the browser's time zone: `2026-10-08 14:32`. `offset` is
/// the zone's minutes ahead of UTC.
pub fn modified(millis: i64, offset: i64) -> String {
    let local = millis + offset * 60_000;
    let minutes = local.div_euclid(60_000).rem_euclid(24 * 60);
    format!("{} {:02}:{:02}", date(local), minutes / 60, minutes % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, kind: EntryKind) -> EntryInfo {
        EntryInfo {
            name: name.to_owned(),
            kind,
            symlink: false,
            size: (kind == EntryKind::File).then_some(10),
            modified: None,
            hidden: name.starts_with('.'),
        }
    }

    /// `/home/c`, with two folders, a hidden folder and three files.
    fn home() -> FolderInfo {
        use EntryKind::{File, Folder};
        FolderInfo {
            path: "/home/c".to_owned(),
            parent: Some("/home".to_owned()),
            entries: vec![
                entry(".config", Folder),
                entry("Movies", Folder),
                entry("Music", Folder),
                entry(".profile", File),
                entry("notes.txt", File),
                entry("todo.md", File),
            ],
            truncated: false,
        }
    }

    fn names(browser: &Browser) -> Vec<String> {
        (0..browser.len())
            .map(|i| browser.entry(i).unwrap().name.clone())
            .collect()
    }

    fn browser(mode: Mode) -> Browser {
        let mut browser = Browser::new(mode, false);
        browser.show(home(), None);
        browser
    }

    #[test]
    fn hidden_entries_show_only_when_asked() {
        let mut b = browser(Mode::Any);
        assert_eq!(names(&b), ["Movies", "Music", "notes.txt", "todo.md"]);
        b.select(1);
        b.set_hidden(true);
        assert_eq!(names(&b).len(), 6);
        assert_eq!(b.selected().unwrap().name, "Music");
    }

    #[test]
    fn the_filter_matches_names_and_puts_the_cursor_on_the_first() {
        let mut b = browser(Mode::Any);
        b.set_filter("MU");
        assert_eq!(names(&b), ["Music"]);
        assert_eq!(b.cursor(), Some(0));
        b.set_filter("o");
        assert_eq!(names(&b), ["Movies", "notes.txt", "todo.md"]);
        assert_eq!(b.selected().unwrap().name, "Movies");
        // The cursor keeps its entry while it matches.
        b.go(Step::Last, 10);
        b.set_filter("od");
        assert_eq!(b.selected().unwrap().name, "todo.md");
        b.set_filter("zzz");
        assert!(b.all_filtered());
        assert_eq!(b.selected(), None);
        b.set_filter("");
        assert!(!b.all_filtered());
    }

    #[test]
    fn the_cursor_moves_by_steps_and_pages_and_stops_at_the_ends() {
        let mut b = browser(Mode::Any);
        b.go(Step::Up, 2);
        assert_eq!(b.cursor(), Some(3));
        b.go(Step::PageUp, 2);
        assert_eq!(b.cursor(), Some(1));
        b.go(Step::PageUp, 2);
        assert_eq!(b.cursor(), Some(0));
        b.go(Step::Down, 2);
        b.go(Step::PageDown, 2);
        assert_eq!(b.cursor(), Some(3));
        b.go(Step::Down, 2);
        assert_eq!(b.cursor(), Some(3));
        b.go(Step::First, 2);
        assert_eq!(b.cursor(), Some(0));
        b.deselect();
        b.go(Step::PageDown, 2);
        assert_eq!(b.cursor(), Some(0));
    }

    #[test]
    fn a_folder_picker_shows_files_but_never_rests_on_them() {
        let mut b = browser(Mode::Folder);
        assert_eq!(b.len(), 4);
        assert!(b.choosable(1) && !b.choosable(2));
        b.go(Step::Last, 10);
        assert_eq!(b.selected().unwrap().name, "Music");
        b.select(3);
        assert_eq!(b.selected().unwrap().name, "Music");
        // Nothing to rest on: the cursor stays off.
        b.set_filter(".txt");
        assert_eq!(b.cursor(), None);
        b.go(Step::Down, 10);
        assert_eq!(b.cursor(), None);
    }

    #[test]
    fn enter_opens_folders_and_chooses_files_the_picker_takes() {
        let mut b = browser(Mode::Any);
        b.select(0);
        assert_eq!(b.enter(), Some(Action::Open("/home/c/Movies".into())));
        assert_eq!(b.open(), Some(Action::Open("/home/c/Movies".into())));
        b.select(2);
        let notes = Choice {
            path: "/home/c/notes.txt".into(),
            from: "/home/c".into(),
            current: false,
        };
        assert_eq!(b.enter(), Some(Action::Choose(notes.clone())));
        assert_eq!(b.open(), None);

        let mut files = browser(Mode::File);
        files.select(2);
        assert_eq!(files.enter(), Some(Action::Choose(notes)));
        files.select(0);
        assert_eq!(files.choice(), None);
        files.deselect();
        assert_eq!(files.enter(), None);
    }

    #[test]
    fn choosing_takes_the_entry_or_else_the_folder_shown() {
        let current = Choice {
            path: "/home/c".into(),
            from: "/home/c".into(),
            current: true,
        };
        let mut folders = browser(Mode::Folder);
        assert_eq!(folders.choice(), Some(current.clone()));
        folders.select(1);
        assert_eq!(folders.choice().unwrap().path, "/home/c/Music");
        assert_eq!(browser(Mode::Any).choice(), Some(current));
        assert_eq!(browser(Mode::File).choice(), None);
        assert_eq!(Browser::new(Mode::Any, false).choice(), None);
    }

    #[test]
    fn going_up_selects_the_folder_left() {
        let b = browser(Mode::Any);
        assert_eq!(b.up(), Some(Action::Back("/home".into(), "c".into())));
        let mut opened = Browser::new(Mode::Any, false);
        let mut parent = home();
        parent.path = "/home".into();
        parent.parent = Some("/".into());
        parent.entries = vec![entry("c", EntryKind::Folder), entry("d", EntryKind::Folder)];
        opened.show(parent, Some("c"));
        assert_eq!(opened.cursor(), Some(0));

        // From a folder that couldn't be read, up goes to the folder it is in.
        let mut failed = browser(Mode::Any);
        failed.fail(Failure {
            path: "/root/secret".into(),
            code: "permission_denied".into(),
            message: "no".into(),
        });
        assert_eq!(failed.location(), Some("/root/secret"));
        assert_eq!(failed.len(), 0);
        assert_eq!(failed.choice(), None);
        assert_eq!(
            failed.up(),
            Some(Action::Back("/root".into(), "secret".into()))
        );

        let mut root = Browser::new(Mode::Any, false);
        root.show(
            FolderInfo {
                path: "/".into(),
                parent: None,
                entries: Vec::new(),
                truncated: false,
            },
            None,
        );
        assert_eq!(root.up(), None);
    }

    fn press(key: &str) -> Press<'_> {
        Press {
            key,
            filter_empty: true,
            ..Press::default()
        }
    }

    #[test]
    fn keys_move_open_choose_and_filter() {
        use Command as C;
        assert_eq!(command(press("ArrowDown")), Some(C::Move(Step::Down)));
        assert_eq!(command(press("End")), Some(C::Move(Step::Last)));
        assert_eq!(command(press("Enter")), Some(C::Enter));
        assert_eq!(command(press("ArrowRight")), Some(C::Open));
        assert_eq!(command(press("ArrowLeft")), Some(C::Up));
        assert_eq!(command(press("Backspace")), Some(C::Up));
        assert_eq!(command(press(".")), Some(C::Hidden));
        assert_eq!(command(press("/")), Some(C::EditPath(Some('/'))));
        assert_eq!(command(press("m")), Some(C::Type('m')));
        assert_eq!(command(press("Escape")), Some(C::Escape));
        assert_eq!(command(press("Tab")), None);
        let ctrl = |key| Press {
            command: true,
            ..press(key)
        };
        assert_eq!(command(ctrl("l")), Some(C::EditPath(None)));
        assert_eq!(command(ctrl("Enter")), Some(C::Choose));
        assert_eq!(command(ctrl("c")), None);

        // While a filter is typed in the list, backspace erases and `.` types.
        let filtering = |key| Press {
            filter_empty: false,
            ..press(key)
        };
        assert_eq!(command(filtering("Backspace")), Some(C::Erase));
        assert_eq!(command(filtering(".")), Some(C::Type('.')));
        assert_eq!(command(filtering("ArrowLeft")), Some(C::Up));
    }

    #[test]
    fn the_filter_box_keeps_its_editing_keys() {
        use Command as C;
        let typing = |key, at_end| Press {
            key,
            in_filter: true,
            at_end,
            ..Press::default()
        };
        assert_eq!(
            command(typing("ArrowDown", false)),
            Some(C::Move(Step::Down))
        );
        assert_eq!(command(typing("Enter", false)), Some(C::Enter));
        assert_eq!(command(typing("Home", false)), None);
        assert_eq!(command(typing("ArrowLeft", false)), None);
        assert_eq!(command(typing("ArrowRight", false)), None);
        assert_eq!(command(typing("ArrowRight", true)), Some(C::Open));
        assert_eq!(command(typing("Backspace", true)), None);
        assert_eq!(command(typing("m", true)), None);
        assert_eq!(command(typing(".", true)), None);
        let empty = Press {
            filter_empty: true,
            ..typing("Backspace", true)
        };
        assert_eq!(command(empty), Some(C::Up));
    }

    #[test]
    fn times_show_in_the_browsers_zone() {
        // 2026-10-08 13:05 UTC.
        let at = 1_791_464_700_000;
        assert_eq!(modified(at, 0), "2026-10-08 13:05");
        assert_eq!(modified(at, 120), "2026-10-08 15:05");
        assert_eq!(modified(at, -14 * 60), "2026-10-07 23:05");
    }
}

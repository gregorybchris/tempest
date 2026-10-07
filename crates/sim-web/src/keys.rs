//! The keys, in one table that drives both the keyboard and the hint bar.

use tempest_ui_kit::components::Hint;

use crate::route::Area;

/// What a key does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    /// Open an area.
    Area(Area),
    /// Focus the page's search field.
    Search,
    /// Switch to the next theme.
    Theme,
    /// Show the next page of runs.
    NextPage,
    /// Show the previous page of runs.
    PreviousPage,
}

/// Each key and its command, with the hint's keys and words.
const BINDINGS: &[(&str, Command)] = &[
    ("1", Command::Area(Area::Experiments)),
    ("2", Command::Area(Area::Runs)),
    ("3", Command::Area(Area::Compare)),
    ("4", Command::Area(Area::Baselines)),
    ("/", Command::Search),
    ("T", Command::Theme),
    ("]", Command::NextPage),
    ("[", Command::PreviousPage),
];

/// The command `key` gives, if any.
pub fn command(key: &str) -> Option<Command> {
    BINDINGS
        .iter()
        .find(|(k, _)| *k == key)
        .map(|&(_, command)| command)
}

/// The hint bar's hints.
pub fn hints() -> Vec<Hint> {
    vec![
        Hint {
            keys: vec!["1", "4"],
            label: "areas",
        },
        Hint {
            keys: vec!["/"],
            label: "search",
        },
        Hint {
            keys: vec!["[", "]"],
            label: "pages",
        },
        Hint {
            keys: vec!["esc"],
            label: "back",
        },
        Hint {
            keys: vec!["T"],
            label: "theme",
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_hinted_key_has_a_command_and_keys_are_unique() {
        for hint in hints() {
            for key in hint.keys {
                assert!(command(key).is_some() || key == "esc", "{key}");
            }
        }
        let mut keys: Vec<&str> = BINDINGS.iter().map(|(k, _)| *k).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), BINDINGS.len());
        assert_eq!(command("3"), Some(Command::Area(Area::Compare)));
        assert_eq!(command("x"), None);
    }
}

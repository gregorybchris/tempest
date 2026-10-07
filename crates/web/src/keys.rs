//! The keyboard: one table binds every command to its keys, matching the TUI's where both have
//! the command (D-191). Key handling, the hint bar and the help dialog all read it, so they
//! can't disagree.

/// Where the keyboard's focus is, which decides what keys mean.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Scope {
    /// The transfers list.
    List,
    /// A transfer's detail.
    Detail,
    /// The chunk map or rates chart, one level into the detail.
    Deeper,
    /// Share, Catalog or Settings.
    Area,
}

/// What a key does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    /// Show an area, by its place in the navigation.
    Area(usize),
    /// Show the next area.
    NextArea,
    /// Show the area before.
    PreviousArea,
    /// Switch to the next theme.
    Theme,
    /// Show the keys.
    Help,
    /// Move down the list.
    Down,
    /// Move up the list.
    Up,
    /// Go to the first transfer.
    First,
    /// Go to the last transfer.
    Last,
    /// Open the transfer under the cursor.
    Open,
    /// Start or stop the transfer.
    StartStop,
    /// Start the transfer regardless of the queue.
    ForceStart,
    /// Remove the transfer.
    Remove,
    /// Rename the transfer.
    Rename,
    /// Check the transfer's data again.
    Recheck,
    /// Copy the transfer's link.
    CopyLink,
    /// Add a transfer from a link.
    Add,
    /// Type a filter.
    Filter,
    /// Pick a status to show.
    StatusFilter,
    /// Pick what to sort by.
    Sort,
    /// Reverse the order.
    Reverse,
    /// Show the next tab of the detail.
    NextTab,
    /// Show the tab before.
    PreviousTab,
    /// Open the chunk map.
    ChunkMap,
    /// Open the rates chart.
    Rates,
}

/// Keys bound to a command.
#[derive(Clone, Copy, Debug)]
pub struct Binding {
    /// The keys, as `KeyboardEvent.key` names them.
    pub keys: &'static [&'static str],
    /// Where they work.
    pub scopes: &'static [Scope],
    /// What they do.
    pub command: Command,
    /// A word or two for the hint bar, or empty to leave them out of it.
    pub hint: &'static str,
    /// What they do, for the help dialog.
    pub help: &'static str,
}

use Scope::{Area, Deeper, Detail, List};

const EVERYWHERE: &[Scope] = &[List, Detail, Deeper, Area];
const TRANSFER: &[Scope] = &[List, Detail, Deeper];

/// Every binding, in the order hints and help list them.
pub const BINDINGS: &[Binding] = &[
    Binding {
        keys: &["j", "ArrowDown"],
        scopes: &[List],
        command: Command::Down,
        hint: "move",
        help: "Move down the list",
    },
    Binding {
        keys: &["k", "ArrowUp"],
        scopes: &[List],
        command: Command::Up,
        hint: "",
        help: "Move up the list",
    },
    Binding {
        keys: &["g", "Home"],
        scopes: &[List],
        command: Command::First,
        hint: "",
        help: "Go to the first transfer",
    },
    Binding {
        keys: &["G", "End"],
        scopes: &[List],
        command: Command::Last,
        hint: "",
        help: "Go to the last transfer",
    },
    Binding {
        keys: &["Enter"],
        scopes: &[List],
        command: Command::Open,
        hint: "detail",
        help: "Open the transfer's detail",
    },
    Binding {
        keys: &["h", "ArrowLeft"],
        scopes: &[Detail],
        command: Command::PreviousTab,
        hint: "",
        help: "Show the tab before",
    },
    Binding {
        keys: &["l", "ArrowRight"],
        scopes: &[Detail],
        command: Command::NextTab,
        hint: "tabs",
        help: "Show the next tab",
    },
    Binding {
        keys: &["c"],
        scopes: &[Detail, Deeper],
        command: Command::ChunkMap,
        hint: "chunks",
        help: "Open the chunk map",
    },
    Binding {
        keys: &["g"],
        scopes: &[Detail, Deeper],
        command: Command::Rates,
        hint: "rates",
        help: "Open the rates chart",
    },
    Binding {
        keys: &[" "],
        scopes: TRANSFER,
        command: Command::StartStop,
        hint: "start/stop",
        help: "Start or stop the transfer",
    },
    Binding {
        keys: &["F"],
        scopes: TRANSFER,
        command: Command::ForceStart,
        hint: "",
        help: "Force start: run it regardless of the queue",
    },
    Binding {
        keys: &["x"],
        scopes: TRANSFER,
        command: Command::Remove,
        hint: "remove",
        help: "Remove the transfer, keeping or deleting its files",
    },
    Binding {
        keys: &["r"],
        scopes: TRANSFER,
        command: Command::Rename,
        hint: "",
        help: "Rename the transfer",
    },
    Binding {
        keys: &["R"],
        scopes: TRANSFER,
        command: Command::Recheck,
        hint: "",
        help: "Check the transfer's data on disk again",
    },
    Binding {
        keys: &["y"],
        scopes: TRANSFER,
        command: Command::CopyLink,
        hint: "",
        help: "Copy the transfer's link",
    },
    Binding {
        keys: &["a"],
        scopes: &[List],
        command: Command::Add,
        hint: "add",
        help: "Add a transfer from a link",
    },
    Binding {
        keys: &["/"],
        scopes: &[List],
        command: Command::Filter,
        hint: "filter",
        help: "Filter by name, or by tag with #tag",
    },
    Binding {
        keys: &["f"],
        scopes: &[List],
        command: Command::StatusFilter,
        hint: "",
        help: "Show only one status",
    },
    Binding {
        keys: &["s"],
        scopes: &[List],
        command: Command::Sort,
        hint: "",
        help: "Choose what to sort by",
    },
    Binding {
        keys: &["S"],
        scopes: &[List],
        command: Command::Reverse,
        hint: "",
        help: "Reverse the order",
    },
    Binding {
        keys: &["1"],
        scopes: EVERYWHERE,
        command: Command::Area(0),
        hint: "",
        help: "Transfers",
    },
    Binding {
        keys: &["2"],
        scopes: EVERYWHERE,
        command: Command::Area(1),
        hint: "",
        help: "Share",
    },
    Binding {
        keys: &["3"],
        scopes: EVERYWHERE,
        command: Command::Area(2),
        hint: "",
        help: "Catalog",
    },
    Binding {
        keys: &["4"],
        scopes: EVERYWHERE,
        command: Command::Area(3),
        hint: "",
        help: "Settings",
    },
    Binding {
        keys: &["]"],
        scopes: EVERYWHERE,
        command: Command::NextArea,
        hint: "",
        help: "Show the next area",
    },
    Binding {
        keys: &["["],
        scopes: EVERYWHERE,
        command: Command::PreviousArea,
        hint: "",
        help: "Show the area before",
    },
    Binding {
        keys: &["T"],
        scopes: EVERYWHERE,
        command: Command::Theme,
        hint: "theme",
        help: "Switch to the next theme",
    },
    Binding {
        keys: &["?"],
        scopes: EVERYWHERE,
        command: Command::Help,
        hint: "help",
        help: "Show these keys",
    },
];

/// What `key` does in `scope`, if anything.
pub fn command(key: &str, scope: Scope) -> Option<Command> {
    BINDINGS
        .iter()
        .find(|b| b.scopes.contains(&scope) && b.keys.contains(&key))
        .map(|b| b.command)
}

/// The bindings that work in `scope`, for the help dialog.
pub fn help(scope: Scope) -> impl Iterator<Item = &'static Binding> {
    BINDINGS.iter().filter(move |b| b.scopes.contains(&scope))
}

/// The hint bar for `scope`: each hinted binding's keys, as shown, and its hint. Moving and
/// switching tabs show both directions' keys: `j` `k`, `h` `l`.
pub fn hints(scope: Scope) -> Vec<(Vec<&'static str>, &'static str)> {
    let mut hints = Vec::new();
    for binding in help(scope).filter(|b| !b.hint.is_empty()) {
        let mut keys = vec![shown(binding.keys[0])];
        match binding.command {
            Command::Down => keys.push("k"),
            Command::NextTab => keys.insert(0, "h"),
            _ => {}
        }
        hints.push((keys, binding.hint));
    }
    if scope != Scope::List {
        hints.insert(0, (vec!["esc"], "back"));
    }
    hints.push((vec!["1", "4"], "areas"));
    hints
}

/// A key as the hint bar and help show it.
pub fn shown(key: &'static str) -> &'static str {
    match key {
        " " => "space",
        "Enter" => "enter",
        "Escape" => "esc",
        "ArrowDown" => "↓",
        "ArrowUp" => "↑",
        "ArrowLeft" => "←",
        "ArrowRight" => "→",
        "Home" => "home",
        "End" => "end",
        key => key,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn keys_follow_the_tui() {
        assert_eq!(command("j", List), Some(Command::Down));
        assert_eq!(command("Enter", List), Some(Command::Open));
        assert_eq!(command(" ", Detail), Some(Command::StartStop));
        assert_eq!(command("/", List), Some(Command::Filter));
        assert_eq!(command("3", Detail), Some(Command::Area(2)));
        assert_eq!(command("T", Area), Some(Command::Theme));
        assert_eq!(command("?", List), Some(Command::Help));
        // `g` is the first transfer in the list and the rates chart in the detail (D-194).
        assert_eq!(command("g", List), Some(Command::First));
        assert_eq!(command("g", Detail), Some(Command::Rates));
        // Transfer keys need a transfer.
        assert_eq!(command("x", Area), None);
        assert_eq!(command("q", List), None);
    }

    #[test]
    fn no_key_means_two_things_in_one_scope() {
        for scope in [List, Detail, Deeper, Area] {
            let mut seen = HashSet::new();
            for binding in help(scope) {
                for key in binding.keys {
                    assert!(seen.insert(*key), "{key:?} is bound twice in {scope:?}");
                }
            }
        }
    }

    #[test]
    fn hints_pair_keys_and_lead_with_back() {
        let list = hints(List);
        assert_eq!(list[0], (vec!["j", "k"], "move"));
        assert_eq!(list[1], (vec!["enter"], "detail"));
        assert!(list.contains(&(vec!["space"], "start/stop")));
        assert_eq!(list.last(), Some(&(vec!["1", "4"], "areas")));
        let detail = hints(Detail);
        assert_eq!(detail[0], (vec!["esc"], "back"));
        assert!(detail.contains(&(vec!["h", "l"], "tabs")));
        assert!(BINDINGS.iter().all(|b| !b.help.is_empty()));
    }
}

//! The keys, in one table that drives both the keyboard and the hint bar.

use tempest_ui_kit::components::Hint;

use crate::route::Route;

/// What a key does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    /// Focus the search field, going to the listings first if need be.
    Search,
    /// Move the cursor to the next listing.
    Next,
    /// Move the cursor to the previous listing.
    Previous,
    /// Open the listing under the cursor.
    Open,
    /// Show the next page of listings.
    NextPage,
    /// Show the previous page of listings.
    PreviousPage,
    /// Sort by the next sort.
    NextSort,
    /// Vote the listing shown up.
    VoteUp,
    /// Vote the listing shown down.
    VoteDown,
    /// Write a comment on the listing shown.
    Comment,
    /// Edit the listing shown, if it is the account's.
    Edit,
    /// Open the account, or sign in.
    Account,
    /// Switch to the next theme.
    Theme,
}

/// Where a key works.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scope {
    Everywhere,
    Browse,
    Listing,
}

impl Scope {
    fn holds(self, route: &Route) -> bool {
        match self {
            Scope::Everywhere => true,
            Scope::Browse => matches!(route, Route::Browse(_)),
            Scope::Listing => matches!(route, Route::Listing(_)),
        }
    }
}

/// Each key, where it works and its command.
const BINDINGS: &[(&str, Scope, Command)] = &[
    ("/", Scope::Everywhere, Command::Search),
    ("j", Scope::Browse, Command::Next),
    ("k", Scope::Browse, Command::Previous),
    ("Enter", Scope::Browse, Command::Open),
    ("]", Scope::Browse, Command::NextPage),
    ("[", Scope::Browse, Command::PreviousPage),
    ("s", Scope::Browse, Command::NextSort),
    ("+", Scope::Listing, Command::VoteUp),
    ("-", Scope::Listing, Command::VoteDown),
    ("c", Scope::Listing, Command::Comment),
    ("e", Scope::Listing, Command::Edit),
    ("a", Scope::Everywhere, Command::Account),
    ("T", Scope::Everywhere, Command::Theme),
];

/// The command `key` gives on `route`, if any.
pub fn command(route: &Route, key: &str) -> Option<Command> {
    BINDINGS
        .iter()
        .find(|(k, scope, _)| *k == key && scope.holds(route))
        .map(|&(_, _, command)| command)
}

/// The hint bar's hints on `route`.
pub fn hints(route: &Route) -> Vec<Hint> {
    let hint = |keys: Vec<&'static str>, label| Hint { keys, label };
    let mut hints = vec![hint(vec!["/"], "search")];
    match route {
        Route::Browse(_) => hints.extend([
            hint(vec!["j", "k"], "move"),
            hint(vec!["enter"], "open"),
            hint(vec!["[", "]"], "page"),
            hint(vec!["s"], "sort"),
        ]),
        Route::Listing(_) => hints.extend([
            hint(vec!["+", "-"], "vote"),
            hint(vec!["c"], "comment"),
            hint(vec!["e"], "edit"),
            hint(vec!["esc"], "back"),
        ]),
        _ => hints.push(hint(vec!["esc"], "back")),
    }
    hints.extend([hint(vec!["a"], "account"), hint(vec!["T"], "theme")]);
    hints
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_hinted_key_has_a_command_where_it_is_hinted() {
        let bundle = "3f9a000000000000000000000000000000000000000000000000000000000000";
        let routes = [
            Route::default(),
            Route::Listing(bundle.parse().unwrap()),
            Route::SignIn,
            Route::Me,
        ];
        for route in &routes {
            for hint in hints(route) {
                for key in hint.keys {
                    let key = if key == "enter" { "Enter" } else { key };
                    assert!(
                        command(route, key).is_some() || key == "esc",
                        "{key} on {route:?}"
                    );
                }
            }
        }
        assert_eq!(command(&Route::Me, "j"), None);
        assert_eq!(command(&Route::Me, "T"), Some(Command::Theme));
    }

    #[test]
    fn a_key_means_one_thing_in_each_scope() {
        let mut seen: Vec<(&str, Scope)> = BINDINGS.iter().map(|&(k, s, _)| (k, s)).collect();
        let all = seen.len();
        seen.sort_by_key(|&(k, s)| (k, s as u8));
        seen.dedup();
        assert_eq!(seen.len(), all);
        for &(key, scope, _) in BINDINGS {
            if scope != Scope::Everywhere {
                assert!(
                    !BINDINGS
                        .iter()
                        .any(|&(k, s, _)| k == key && s == Scope::Everywhere),
                    "{key}"
                );
            }
        }
    }
}

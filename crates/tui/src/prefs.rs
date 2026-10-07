//! What the user keeps between runs, in the engine's `ui` settings (D-195): the shared parts
//! are [`tempest_api::prefs`], and the TUI adds its theme.

pub use tempest_api::prefs::*;
use tempest_api::ui::UiSettings;

use crate::theme::Theme;

/// The theme `ui` names, or the default.
pub fn theme(ui: &UiSettings) -> Theme {
    Theme::named(&ui.theme).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_themes_fall_back() {
        let ui = UiSettings {
            theme: "pink".to_owned(),
            ..UiSettings::default()
        };
        assert_eq!(theme(&ui), Theme::GREEN);
    }
}

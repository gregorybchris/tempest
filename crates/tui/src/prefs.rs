//! What the user keeps between runs: the theme, the list's columns and saved views. They live
//! in the engine's `ui` settings, shared with the web UI (D-195); names this TUI doesn't know
//! are skipped.

use serde_json::{Value, json};
use tempest_engine::config::{SavedView, UiSettings};

use crate::columns::Column;
use crate::finding::{Finding, Sort, SortKey, StatusFilter};
use crate::theme::Theme;

/// The setting holding the theme.
pub const THEME: &str = "ui.theme";
/// The setting holding the optional columns shown.
pub const COLUMNS: &str = "ui.columns";
/// The setting holding the saved views.
pub const VIEWS: &str = "ui.views";

/// The theme `ui` names, or the default.
pub fn theme(ui: &UiSettings) -> Theme {
    Theme::named(&ui.theme).unwrap_or_default()
}

/// The optional columns `ui` shows, in the list's order, or the default ones.
pub fn columns(ui: &UiSettings) -> Vec<Column> {
    match &ui.columns {
        None => Column::DEFAULT.to_vec(),
        Some(names) => Column::OPTIONAL
            .into_iter()
            .filter(|column| names.iter().any(|name| name == column.id()))
            .collect(),
    }
}

/// `columns` as the `ui.columns` setting holds them.
pub fn columns_value(columns: &[Column]) -> Value {
    let names: Vec<&str> = Column::OPTIONAL
        .into_iter()
        .filter(|column| columns.contains(column))
        .map(Column::id)
        .collect();
    json!(names)
}

/// `views` as the `ui.views` setting holds them.
pub fn views_value(views: &[SavedView]) -> Value {
    serde_json::to_value(views).expect("views serialize")
}

/// What `view` shows, as a finding. Unknown statuses show every status, and unknown sorts
/// keep queue order.
pub fn finding(view: &SavedView) -> Finding {
    Finding {
        text: view.query.clone(),
        status: view.status.as_deref().and_then(StatusFilter::from_id),
        sort: Sort {
            key: SortKey::from_id(&view.sort).unwrap_or(SortKey::Queue),
            reversed: view.reversed,
        },
    }
}

/// `finding` saved as a view named `name`.
pub fn saved(name: &str, finding: &Finding) -> SavedView {
    let sort = match finding.sort.key {
        SortKey::Queue => String::new(),
        key => key.id().to_owned(),
    };
    SavedView {
        name: name.trim().to_owned(),
        query: finding.text.trim().to_owned(),
        status: finding.status.map(|status| status.id().to_owned()),
        sort,
        reversed: finding.sort.reversed,
    }
}

/// `views` with `view` in place of the one of its name, or added at the end.
pub fn with_view(views: &[SavedView], view: SavedView) -> Vec<SavedView> {
    let mut views = views.to_vec();
    match views.iter_mut().find(|v| v.name == view.name) {
        Some(existing) => *existing = view,
        None => views.push(view),
    }
    views
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn unknown_names_fall_back() {
        let ui = UiSettings {
            theme: "pink".to_owned(),
            columns: Some(vec!["tags".to_owned(), "wat".to_owned(), "size".to_owned()]),
            views: Vec::new(),
        };
        assert_eq!(theme(&ui), Theme::GREEN);
        assert_eq!(columns(&ui), [Column::Size, Column::Tags]);
        assert_eq!(columns(&UiSettings::default()), Column::DEFAULT);
        assert_eq!(
            columns_value(&[Column::Tags, Column::Size]),
            json!(["size", "tags"])
        );

        let view = SavedView {
            name: "odd".to_owned(),
            status: Some("asleep".to_owned()),
            sort: "colour".to_owned(),
            ..SavedView::default()
        };
        assert_eq!(finding(&view), Finding::default());
    }

    #[test]
    fn saving_a_view_replaces_one_of_the_same_name() {
        let a = saved("A", &Finding::default());
        let b = saved("B", &Finding::default());
        let finding = Finding {
            text: "#music".to_owned(),
            ..Finding::default()
        };
        let views = with_view(&[a.clone(), b.clone()], saved("A", &finding));
        assert_eq!(views.len(), 2);
        assert_eq!(views[0].query, "#music");
        assert_eq!(with_view(&[a], b).len(), 2);
    }

    fn any_finding() -> impl Strategy<Value = Finding> {
        (
            "[a-z# ]{0,12}",
            proptest::option::of(proptest::sample::select(StatusFilter::ALL.to_vec())),
            proptest::sample::select(SortKey::ALL.to_vec()),
            any::<bool>(),
        )
            .prop_map(|(text, status, key, reversed)| Finding {
                text: text.trim().to_owned(),
                status,
                sort: Sort { key, reversed },
            })
    }

    proptest! {
        /// A saved view brings back the finding it saved, through the setting's JSON too.
        #[test]
        fn views_round_trip(finding in any_finding()) {
            let view = saved("view", &finding);
            let json = views_value(std::slice::from_ref(&view));
            let read: Vec<SavedView> = serde_json::from_value(json).unwrap();
            prop_assert_eq!(super::finding(&read[0]), finding);
        }
    }
}

//! Which of the list's columns fit the window, in pixels (D-138). The columns, their order and
//! priorities are [`tempest_api::columns`]; this module says how wide each is here.

use tempest_api::columns::{self, Column};

/// The pixels a column needs, cell padding included; the name's least.
pub fn needs(column: Column) -> u32 {
    match column {
        Column::Name => 200,
        Column::Size => 86,
        Column::Progress => 168,
        Column::Down | Column::Up => 100,
        Column::Eta => 76,
        Column::Status => 132,
        Column::Peers => 72,
        Column::Ratio => 70,
        Column::Category => 112,
        Column::Tags => 160,
        Column::Added => 112,
    }
}

/// The columns that fit `width` pixels, left to right, from those `chosen`.
pub fn fitting(width: f64, chosen: &[Column]) -> Vec<Column> {
    columns::fitting(width.max(0.0) as u32, chosen, 0, needs)
}

/// The table's classes that hide every column not `shown`. Rows draw every column, so a
/// change of width touches one attribute rather than every row.
pub fn hiding(shown: &[Column]) -> String {
    let mut class = "table transfers".to_owned();
    for column in Column::ORDER {
        if !shown.contains(&column) {
            class.push_str(" hide-");
            class.push_str(column.id());
        }
    }
    class
}

/// A column's cell class, which [`hiding`] refers to.
pub fn cell_class(column: Column) -> String {
    let mut class = format!("col-{}", column.id());
    if column.numeric() {
        class.push_str(" num");
    }
    if column == Column::Name {
        class.push_str(" table__clip");
    }
    class
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phones_keep_the_core_and_desktops_show_the_defaults() {
        use Column::*;
        assert_eq!(fitting(360.0, &Column::DEFAULT), [Name, Progress, Status]);
        assert_eq!(
            fitting(700.0, &Column::DEFAULT),
            [Name, Size, Progress, Down, Status]
        );
        assert_eq!(
            fitting(1100.0, &Column::DEFAULT),
            [Name, Size, Progress, Down, Up, Eta, Status, Peers, Ratio]
        );
    }

    #[test]
    fn hidden_columns_are_named_in_the_class() {
        let class = hiding(&[Column::Name, Column::Progress, Column::Status]);
        assert!(class.contains("hide-size") && class.contains("hide-up"));
        assert!(!class.contains("hide-name") && !class.contains("hide-status"));
        assert_eq!(cell_class(Column::Size), "col-size num");
    }
}

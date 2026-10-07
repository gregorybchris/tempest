//! The transfers list's columns in characters: how wide each is, and which fit a terminal
//! (D-138). The columns themselves are [`tempest_api::columns`].

pub use tempest_api::columns::Column;

/// The gap between columns.
pub const SPACING: u16 = 1;

/// The narrowest a name may get before columns are dropped instead.
pub const NAME_MIN: u16 = 14;

/// The width of the progress bar, without its percentage.
pub const BAR: u16 = 10;

/// A column's fixed width in characters, or for the name its least.
pub fn width(column: Column) -> u16 {
    match column {
        Column::Name => NAME_MIN,
        Column::Size | Column::Down | Column::Up => 5,
        Column::Progress => BAR + 5,
        Column::Eta => 6,
        Column::Status => 11,
        Column::Peers => 7,
        // Room for "Ratio" and a sort arrow.
        Column::Ratio => 6,
        Column::Category => 10,
        Column::Tags => 12,
        // Room for "Added" and a sort arrow, and for a date.
        Column::Added => 10,
    }
}

/// The columns that fit in `width` characters, left to right: those that always show, then the
/// most important of `chosen` that fit.
pub fn fitting(width: u16, chosen: &[Column]) -> Vec<Column> {
    tempest_api::columns::fitting(width.into(), chosen, SPACING.into(), |column| {
        self::width(column).into()
    })
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn total(columns: &[Column]) -> u16 {
        columns.iter().map(|c| width(*c)).sum::<u16>() + SPACING * (columns.len() as u16 - 1)
    }

    #[test]
    fn narrow_terminals_drop_up_then_eta_then_down_then_size() {
        use Column::*;
        let fitting = |width| fitting(width, &Column::DEFAULT);
        let all = [Name, Size, Progress, Down, Up, Eta, Status, Peers, Ratio];
        assert_eq!(fitting(82), all);
        assert_eq!(
            fitting(81),
            [Name, Size, Progress, Down, Up, Eta, Status, Peers]
        );
        assert_eq!(fitting(74), [Name, Size, Progress, Down, Up, Eta, Status]);
        assert_eq!(fitting(66), [Name, Size, Progress, Down, Eta, Status]);
        assert_eq!(fitting(60), [Name, Size, Progress, Down, Status]);
        assert_eq!(fitting(53), [Name, Size, Progress, Status]);
        assert_eq!(fitting(47), [Name, Progress, Status]);
        assert_eq!(fitting(10), [Name, Progress, Status]);
        assert_eq!(fitting(300), all);
    }

    #[test]
    fn only_chosen_columns_show() {
        use Column::*;
        assert_eq!(fitting(200, &[]), [Name, Progress, Status]);
        assert_eq!(
            fitting(200, &[Tags, Up, Added]),
            [Name, Progress, Up, Status, Tags, Added]
        );
        assert_eq!(fitting(200, &Column::OPTIONAL), Column::ORDER);
        // Chosen columns still give way in priority order.
        assert_eq!(fitting(60, &[Tags, Size]), [Name, Size, Progress, Status]);
        for column in Column::OPTIONAL {
            assert_eq!(Column::from_id(column.id()), Some(column));
        }
        assert_eq!(Column::from_id("name"), None);
    }

    proptest! {
        /// Whatever fits, fits, and widening never drops a column.
        #[test]
        fn columns_fit_and_only_grow(
            width in 42u16..300,
            chosen in proptest::sample::subsequence(Column::OPTIONAL.to_vec(), 0..=9),
        ) {
            let columns = fitting(width, &chosen);
            prop_assert!(total(&columns) <= width);
            prop_assert!(columns.iter().all(|c| c.always() || chosen.contains(c)));
            let wider = fitting(width + 1, &chosen);
            prop_assert!(columns.iter().all(|c| wider.contains(c)));
        }
    }
}

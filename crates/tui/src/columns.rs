//! The transfers list's columns, and which of them fit (D-138): name, progress and status
//! always; then size, ↓ and time left; then ↑; then whatever else is room for.

/// A column of the transfers list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Column {
    /// The transfer's name. It takes whatever width is left.
    Name,
    /// The bundle's size.
    Size,
    /// A bar and a percentage.
    Progress,
    /// The download rate.
    Down,
    /// The upload rate.
    Up,
    /// Time left.
    Eta,
    /// The status.
    Status,
    /// Connections out of the peers known.
    Peers,
    /// Bytes sent per byte received.
    Ratio,
}

/// The gap between columns.
pub const SPACING: u16 = 1;

/// The narrowest a name may get before columns are dropped instead.
pub const NAME_MIN: u16 = 14;

/// The width of the progress bar, without its percentage.
pub const BAR: u16 = 10;

impl Column {
    /// Left to right.
    pub const ORDER: [Self; 9] = [
        Self::Name,
        Self::Size,
        Self::Progress,
        Self::Down,
        Self::Up,
        Self::Eta,
        Self::Status,
        Self::Peers,
        Self::Ratio,
    ];

    /// Most important first: the order in which columns are given room.
    pub const PRIORITY: [Self; 9] = [
        Self::Name,
        Self::Progress,
        Self::Status,
        Self::Size,
        Self::Down,
        Self::Eta,
        Self::Up,
        Self::Peers,
        Self::Ratio,
    ];

    /// The heading.
    pub fn title(self) -> &'static str {
        match self {
            Self::Name => "Name",
            Self::Size => "Size",
            Self::Progress => "Progress",
            Self::Down => "↓",
            Self::Up => "↑",
            Self::Eta => "ETA",
            Self::Status => "Status",
            Self::Peers => "Peers",
            Self::Ratio => "Ratio",
        }
    }

    /// The fixed width, or for the name its least.
    pub fn width(self) -> u16 {
        match self {
            Self::Name => NAME_MIN,
            Self::Size | Self::Down | Self::Up => 5,
            Self::Progress => BAR + 5,
            Self::Eta => 6,
            Self::Status => 11,
            Self::Peers => 7,
            Self::Ratio => 5,
        }
    }

    /// Whether the column's values line up on the right.
    pub fn numeric(self) -> bool {
        !matches!(self, Self::Name | Self::Progress | Self::Status)
    }
}

/// The columns that fit in `width`, left to right: the most important ones that fit, as
/// [`Column::PRIORITY`] orders them.
pub fn fitting(width: u16) -> Vec<Column> {
    let mut used = 0;
    let mut chosen = Vec::new();
    for column in Column::PRIORITY {
        let needed = column.width() + if chosen.is_empty() { 0 } else { SPACING };
        let always = matches!(column, Column::Name | Column::Progress | Column::Status);
        if !always && used + needed > width {
            break;
        }
        used += needed;
        chosen.push(column);
    }
    Column::ORDER
        .into_iter()
        .filter(|column| chosen.contains(column))
        .collect()
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn total(columns: &[Column]) -> u16 {
        columns.iter().map(|c| c.width()).sum::<u16>() + SPACING * (columns.len() as u16 - 1)
    }

    #[test]
    fn narrow_terminals_drop_up_then_eta_then_down_then_size() {
        use Column::*;
        assert_eq!(fitting(81), Column::ORDER);
        assert_eq!(
            fitting(80),
            [Name, Size, Progress, Down, Up, Eta, Status, Peers]
        );
        assert_eq!(fitting(74), [Name, Size, Progress, Down, Up, Eta, Status]);
        assert_eq!(fitting(66), [Name, Size, Progress, Down, Eta, Status]);
        assert_eq!(fitting(60), [Name, Size, Progress, Down, Status]);
        assert_eq!(fitting(53), [Name, Size, Progress, Status]);
        assert_eq!(fitting(47), [Name, Progress, Status]);
        assert_eq!(fitting(10), [Name, Progress, Status]);
        assert_eq!(fitting(200), Column::ORDER);
    }

    proptest! {
        /// Whatever fits, fits, and widening never drops a column.
        #[test]
        fn columns_fit_and_only_grow(width in 42u16..300) {
            let columns = fitting(width);
            prop_assert!(total(&columns) <= width);
            let wider = fitting(width + 1);
            prop_assert!(columns.iter().all(|c| wider.contains(c)));
        }
    }
}

//! The transfers list's columns, and which of them fit (D-138): name, progress and status
//! always; then size, ↓ and time left; then ↑; then whatever else is room for. Which optional
//! columns show at all is the user's choice (D-197).

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
    /// The category.
    Category,
    /// The tags.
    Tags,
    /// The date it was added.
    Added,
}

/// The gap between columns.
pub const SPACING: u16 = 1;

/// The narrowest a name may get before columns are dropped instead.
pub const NAME_MIN: u16 = 14;

/// The width of the progress bar, without its percentage.
pub const BAR: u16 = 10;

impl Column {
    /// Left to right.
    pub const ORDER: [Self; 12] = [
        Self::Name,
        Self::Size,
        Self::Progress,
        Self::Down,
        Self::Up,
        Self::Eta,
        Self::Status,
        Self::Peers,
        Self::Ratio,
        Self::Category,
        Self::Tags,
        Self::Added,
    ];

    /// Most important first: the order in which columns are given room.
    pub const PRIORITY: [Self; 12] = [
        Self::Name,
        Self::Progress,
        Self::Status,
        Self::Size,
        Self::Down,
        Self::Eta,
        Self::Up,
        Self::Peers,
        Self::Ratio,
        Self::Category,
        Self::Tags,
        Self::Added,
    ];

    /// The columns the user can show or hide, as the column picker lists them.
    pub const OPTIONAL: [Self; 9] = [
        Self::Size,
        Self::Down,
        Self::Up,
        Self::Eta,
        Self::Peers,
        Self::Ratio,
        Self::Category,
        Self::Tags,
        Self::Added,
    ];

    /// The optional columns shown until the user chooses.
    pub const DEFAULT: [Self; 6] = [
        Self::Size,
        Self::Down,
        Self::Up,
        Self::Eta,
        Self::Peers,
        Self::Ratio,
    ];

    /// Whether it always shows.
    pub fn always(self) -> bool {
        matches!(self, Self::Name | Self::Progress | Self::Status)
    }

    /// Its name in the `ui.columns` setting.
    pub fn id(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Size => "size",
            Self::Progress => "progress",
            Self::Down => "down",
            Self::Up => "up",
            Self::Eta => "eta",
            Self::Status => "status",
            Self::Peers => "peers",
            Self::Ratio => "ratio",
            Self::Category => "category",
            Self::Tags => "tags",
            Self::Added => "added",
        }
    }

    /// The optional column named `id` in the `ui.columns` setting.
    pub fn from_id(id: &str) -> Option<Self> {
        Self::OPTIONAL.into_iter().find(|column| column.id() == id)
    }

    /// What it shows, as the column picker describes it.
    pub fn description(self) -> &'static str {
        match self {
            Self::Name => "Name",
            Self::Size => "Size",
            Self::Progress => "Progress",
            Self::Down => "↓ Download rate",
            Self::Up => "↑ Upload rate",
            Self::Eta => "ETA (time left)",
            Self::Status => "Status",
            Self::Peers => "Peers (connected/known)",
            Self::Ratio => "Ratio",
            Self::Category => "Category",
            Self::Tags => "Tags",
            Self::Added => "Date added (UTC)",
        }
    }

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
            Self::Category => "Category",
            Self::Tags => "Tags",
            Self::Added => "Added",
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
            // Room for "Ratio" and a sort arrow.
            Self::Ratio => 6,
            Self::Category => 10,
            Self::Tags => 12,
            // Room for "Added" and a sort arrow, and for a date.
            Self::Added => 10,
        }
    }

    /// Whether the column's values line up on the right.
    pub fn numeric(self) -> bool {
        !matches!(
            self,
            Self::Name | Self::Progress | Self::Status | Self::Category | Self::Tags
        )
    }
}

/// The columns that fit in `width`, left to right: those that always show, then the most
/// important of `chosen` that fit, as [`Column::PRIORITY`] orders them.
pub fn fitting(width: u16, chosen: &[Column]) -> Vec<Column> {
    let mut used = 0;
    let mut shown = Vec::new();
    let wanted = Column::PRIORITY
        .into_iter()
        .filter(|column| column.always() || chosen.contains(column));
    for column in wanted {
        let needed = column.width() + if shown.is_empty() { 0 } else { SPACING };
        if !column.always() && used + needed > width {
            break;
        }
        used += needed;
        shown.push(column);
    }
    Column::ORDER
        .into_iter()
        .filter(|column| shown.contains(column))
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

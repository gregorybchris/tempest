//! The transfers list's columns, and which of them fit (D-138): name, progress and status
//! always; then size, ↓ and time left; then ↑; then whatever else there is room for. Which
//! optional columns show at all is the user's choice (D-197). How wide each column is depends on
//! the client: characters in the TUI, pixels in the web UI.

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
            Self::Eta => "Time left",
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

    /// Whether the column's values line up on the right.
    pub fn numeric(self) -> bool {
        !matches!(
            self,
            Self::Name | Self::Progress | Self::Status | Self::Category | Self::Tags
        )
    }
}

/// The columns that fit in `width`, left to right: those that always show, then the most
/// important of `chosen` that fit, as [`Column::PRIORITY`] orders them. Each column needs
/// `needs(column)`, with `gap` between columns; the name's need is its least.
pub fn fitting(
    width: u32,
    chosen: &[Column],
    gap: u32,
    needs: impl Fn(Column) -> u32,
) -> Vec<Column> {
    let mut used = 0;
    let mut shown = Vec::new();
    let wanted = Column::PRIORITY
        .into_iter()
        .filter(|column| column.always() || chosen.contains(column));
    for column in wanted {
        let needed = needs(column) + if shown.is_empty() { 0 } else { gap };
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

    /// Every column ten wide, with a gap of one.
    fn tens(width: u32, chosen: &[Column]) -> Vec<Column> {
        fitting(width, chosen, 1, |_| 10)
    }

    #[test]
    fn narrow_lists_drop_up_then_eta_then_down_then_size() {
        use Column::*;
        let fitting = |width| tens(width, &[Size, Down, Up, Eta]);
        assert_eq!(fitting(76), [Name, Size, Progress, Down, Up, Eta, Status]);
        assert_eq!(fitting(75), [Name, Size, Progress, Down, Eta, Status]);
        assert_eq!(fitting(54), [Name, Size, Progress, Down, Status]);
        assert_eq!(fitting(43), [Name, Size, Progress, Status]);
        assert_eq!(fitting(42), [Name, Progress, Status]);
        assert_eq!(fitting(0), [Name, Progress, Status]);
    }

    #[test]
    fn only_chosen_columns_show() {
        use Column::*;
        assert_eq!(tens(500, &[]), [Name, Progress, Status]);
        assert_eq!(
            tens(500, &[Tags, Up, Added]),
            [Name, Progress, Up, Status, Tags, Added]
        );
        assert_eq!(tens(500, &Column::OPTIONAL), Column::ORDER);
        for column in Column::OPTIONAL {
            assert_eq!(Column::from_id(column.id()), Some(column));
        }
        assert_eq!(Column::from_id("name"), None);
    }

    proptest! {
        /// Whatever fits, fits, and widening never drops a column.
        #[test]
        fn columns_fit_and_only_grow(
            width in 0u32..400,
            chosen in proptest::sample::subsequence(Column::OPTIONAL.to_vec(), 0..=9),
            needs in proptest::collection::vec(1u32..40, 12),
        ) {
            let needs = |column: Column| {
                needs[Column::ORDER.iter().position(|c| *c == column).unwrap()]
            };
            let columns = fitting(width, &chosen, 2, needs);
            let total: u32 = columns.iter().map(|c| needs(*c)).sum::<u32>()
                + 2 * (columns.len() as u32 - 1);
            let core: u32 = columns.iter().filter(|c| c.always()).map(|c| needs(*c) + 2).sum();
            prop_assert!(total <= width.max(core));
            prop_assert!(columns.iter().all(|c| c.always() || chosen.contains(c)));
            let wider = fitting(width + 1, &chosen, 2, needs);
            prop_assert!(columns.iter().all(|c| wider.contains(c)));
        }
    }
}

//! Fitting a transfer's chunk map to the terminal, and how each of its cells looks.
//!
//! The engine summarizes the chunks into as many cells as the client asks for (D-193). Each
//! terminal cell shows two of them, one above the other, as a half block.

use tempest_engine::inspect::CellInfo;

/// The screen's rows the full-screen map leaves for other things: the header, the rule below it,
/// the title, the legend, the blank line under them and the hint bar.
pub const CHROME_ROWS: u16 = 6;

/// The availability at which a missing cell is drawn at its brightest. Rarer ones are dimmer.
pub const COMMON: u32 = 5;

/// How many map cells fit in `width` by `height` terminal cells.
pub fn capacity(width: u16, height: u16) -> u32 {
    u32::from(width) * u32::from(height) * 2
}

/// How many map cells to ask for to fill a terminal of `width` by `height`. The engine sends no
/// more than the bundle has chunks.
pub fn cells_for_screen(width: u16, height: u16) -> u32 {
    capacity(width, height.saturating_sub(CHROME_ROWS)).max(1)
}

/// Where map cell `index` goes in a map `columns` wide: its column, and its row counting in
/// half rows from the top. Cells fill rows left to right, top to bottom.
pub fn place(index: usize, columns: u16) -> (u16, usize) {
    let columns = usize::from(columns.max(1));
    ((index % columns) as u16, index / columns)
}

/// How many terminal rows `cells` map cells take in a map `columns` wide.
pub fn rows(cells: usize, columns: u16) -> usize {
    cells.div_ceil(usize::from(columns.max(1))).div_ceil(2)
}

/// How a map cell looks.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Shade {
    /// It covers no chunks: past the end of the map.
    Empty,
    /// Every chunk is stored.
    Have,
    /// This share of its chunks is stored, and the rest are missing.
    Partial(f32),
    /// Some of its chunks are being downloaded.
    InFlight,
    /// Some are missing, and its rarest chunk is held by this many connected peers, at most
    /// [`COMMON`].
    Missing(u32),
    /// Some are missing, and no connected peer holds its rarest chunk.
    Nowhere,
    /// Some are missing, and the transfer isn't running, so nobody's holdings are known.
    Unknown,
}

/// How `cell` looks, in a map of a transfer that is `running` or not.
pub fn shade(cell: &CellInfo, running: bool) -> Shade {
    let chunks = cell.chunks();
    if chunks == 0 {
        Shade::Empty
    } else if cell.in_flight > 0 {
        Shade::InFlight
    } else if cell.have == chunks {
        Shade::Have
    } else if cell.have > 0 {
        Shade::Partial(cell.have as f32 / chunks as f32)
    } else if !running {
        Shade::Unknown
    } else if cell.availability == 0 {
        Shade::Nowhere
    } else {
        Shade::Missing(cell.availability.min(COMMON))
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn cell(have: u32, in_flight: u32, missing: u32, availability: u32) -> CellInfo {
        CellInfo::from([have, in_flight, missing, availability])
    }

    #[test]
    fn a_screen_asks_for_two_cells_per_terminal_cell_below_the_chrome() {
        assert_eq!(capacity(80, 10), 1600);
        assert_eq!(cells_for_screen(80, 24), 80 * 18 * 2);
        // A terminal too short for a map still asks for one cell.
        assert_eq!(cells_for_screen(80, 3), 1);
    }

    #[test]
    fn cells_fill_rows_in_half_rows() {
        assert_eq!(place(0, 10), (0, 0));
        assert_eq!(place(9, 10), (9, 0));
        assert_eq!(place(10, 10), (0, 1));
        assert_eq!(place(25, 10), (5, 2));
        assert_eq!(rows(0, 10), 0);
        assert_eq!(rows(10, 10), 1);
        assert_eq!(rows(20, 10), 1);
        assert_eq!(rows(21, 10), 2);
        // A map never has zero columns.
        assert_eq!(place(3, 0), (0, 3));
    }

    #[test]
    fn cells_shade_by_state_then_rarity() {
        assert_eq!(shade(&cell(0, 0, 0, 0), true), Shade::Empty);
        assert_eq!(shade(&cell(4, 0, 0, 3), true), Shade::Have);
        assert_eq!(shade(&cell(1, 1, 2, 3), true), Shade::InFlight);
        assert_eq!(shade(&cell(1, 0, 3, 3), true), Shade::Partial(0.25));
        assert_eq!(shade(&cell(0, 0, 4, 2), true), Shade::Missing(2));
        assert_eq!(shade(&cell(0, 0, 4, 40), true), Shade::Missing(COMMON));
        assert_eq!(shade(&cell(0, 0, 4, 0), true), Shade::Nowhere);
        assert_eq!(shade(&cell(0, 0, 4, 0), false), Shade::Unknown);
    }

    proptest! {
        #[test]
        fn every_cell_asked_for_fits_on_the_screen(width in 1u16..400, height in 0u16..120) {
            let cells = cells_for_screen(width, height) as usize;
            let rows = rows(cells, width);
            prop_assert!(rows <= usize::from(height.saturating_sub(CHROME_ROWS)).max(1));
            let (column, half_row) = place(cells - 1, width);
            prop_assert!(column < width);
            prop_assert!(half_row / 2 < rows);
        }
    }
}

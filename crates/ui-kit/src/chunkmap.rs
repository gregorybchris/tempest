//! The chunk map: every chunk of a bundle as cells that show what is stored, downloading and
//! missing, missing ones dimmer the rarer they are.
//!
//! The engine summarizes chunks into as many cells as a client asks for (D-193); [`ChunkCell`]
//! has the same shape. The map is the UIs' hottest live view, so it follows the Leptos
//! guidelines in specs/interfaces.md: one effect diffs new cells into a signal per cell, and a
//! tick touches only the cells that changed.

use leptos::html::Div;
use leptos::prelude::*;
use leptos::wasm_bindgen::JsCast;

use crate::format;

/// One cell of a chunk map: a run of consecutive chunks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ChunkCell {
    /// Chunks stored.
    pub have: u32,
    /// Chunks requested and not yet stored.
    pub in_flight: u32,
    /// Chunks neither stored nor requested.
    pub missing: u32,
    /// The fewest connected peers holding any one of its chunks.
    pub availability: u32,
}

impl ChunkCell {
    /// How many chunks it covers.
    pub fn chunks(&self) -> u32 {
        self.have + self.in_flight + self.missing
    }
}

/// The availability at which a missing cell is drawn at its brightest. Rarer ones are dimmer.
pub const COMMON: u32 = 5;

/// How a cell looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shade {
    /// It covers no chunks.
    Empty,
    /// Every chunk is stored.
    Have,
    /// This many tenths of its chunks are stored, from 1 to 9, and the rest are missing.
    Partial(u8),
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

impl Shade {
    /// How `cell` looks, in a map of a transfer that is `running` or not.
    pub fn of(cell: &ChunkCell, running: bool) -> Shade {
        let chunks = cell.chunks();
        if chunks == 0 {
            Shade::Empty
        } else if cell.in_flight > 0 {
            Shade::InFlight
        } else if cell.have == chunks {
            Shade::Have
        } else if cell.have > 0 {
            let tenths = (u64::from(cell.have) * 10 / u64::from(chunks)).clamp(1, 9);
            Shade::Partial(tenths as u8)
        } else if !running {
            Shade::Unknown
        } else if cell.availability == 0 {
            Shade::Nowhere
        } else {
            Shade::Missing(cell.availability.min(COMMON))
        }
    }

    /// Its classes.
    pub fn class(self) -> &'static str {
        match self {
            Shade::Empty => "cell cell--empty",
            Shade::Have => "cell cell--have",
            Shade::Partial(_) => "cell cell--partial",
            Shade::InFlight => "cell cell--flight",
            Shade::Missing(_) | Shade::Unknown => "cell cell--missing",
            Shade::Nowhere => "cell cell--nowhere",
        }
    }

    /// Its inline style: how full a partial cell is, or how bright a missing one.
    pub fn style(self) -> String {
        match self {
            Shade::Partial(tenths) => format!("--p:{}", f64::from(tenths) / 10.0),
            Shade::Missing(peers) => format!(
                "--a:{:.2}",
                0.2 + 0.8 * f64::from(peers) / f64::from(COMMON)
            ),
            Shade::Unknown => "--a:0.35".to_owned(),
            _ => String::new(),
        }
    }
}

/// The chunks cell `index` covers, as a half-open range of chunk indexes.
pub fn chunk_range(cells: &[ChunkCell], index: usize) -> (u64, u64) {
    let start: u64 = cells[..index.min(cells.len())]
        .iter()
        .map(|c| u64::from(c.chunks()))
        .sum();
    let len = cells.get(index).map_or(0, |c| u64::from(c.chunks()));
    (start, start + len)
}

/// A sentence about cell `index`, such as "chunks 1,024–1,535 · 512 missing · 3 peers have
/// them".
pub fn describe(cells: &[ChunkCell], running: bool, index: usize) -> String {
    let Some(cell) = cells.get(index) else {
        return String::new();
    };
    let (start, end) = chunk_range(cells, index);
    let span = match end - start {
        0 => return "no chunks".to_owned(),
        1 => format!("chunk {}", format::count(start)),
        _ => format!("chunks {}–{}", format::count(start), format::count(end - 1)),
    };
    let state = match (cell.have, cell.in_flight, cell.missing) {
        (_, 0, 0) => "stored".to_owned(),
        (0, 0, _) => "missing".to_owned(),
        (have, in_flight, missing) => [
            (have, "stored"),
            (in_flight, "downloading"),
            (missing, "missing"),
        ]
        .into_iter()
        .filter(|(n, _)| *n > 0)
        .map(|(n, what)| format!("{} {what}", format::count(n.into())))
        .collect::<Vec<_>>()
        .join(", "),
    };
    let peers = if cell.have == cell.chunks() {
        None
    } else if !running {
        Some("not running, so peers are unknown".to_owned())
    } else {
        Some(match cell.availability {
            0 => "no connected peer has them".to_owned(),
            1 => "1 peer has the rarest chunk here".to_owned(),
            n => format!("{n} peers have the rarest chunk here"),
        })
    };
    let mut text = format!("{span} · {state}");
    if let Some(peers) = peers {
        text.push_str(" · ");
        text.push_str(&peers);
    }
    text
}

/// A chunk map with its legend and a readout of the cell under the pointer.
#[component]
pub fn ChunkMap(
    /// The cells, in chunk order. Their number may change.
    #[prop(into)]
    cells: Signal<Vec<ChunkCell>>,
    /// Whether the transfer is running, which decides whether availability is known.
    #[prop(into)]
    running: Signal<bool>,
    /// Cells per row on wide screens.
    #[prop(default = 64)]
    columns: usize,
) -> impl IntoView {
    let grid = NodeRef::<Div>::new();
    let shades = Shades::new(cells, running);
    let hovered = RwSignal::new(None::<usize>);
    let readout = move || match hovered.get() {
        Some(i) => cells.with(|c| describe(c, running.get(), i)),
        None => "hover a cell for details".to_owned(),
    };
    let summary = move || {
        cells.with(|c| {
            let chunks: u64 = c.iter().map(|c| u64::from(c.chunks())).sum();
            let have: u64 = c.iter().map(|c| u64::from(c.have)).sum();
            let fraction = if chunks == 0 {
                0.0
            } else {
                have as f64 / chunks as f64
            };
            format!(
                "Chunk map: {} of {} chunks stored",
                format::percent(fraction),
                format::count(chunks)
            )
        })
    };
    // One delegated listener serves every cell.
    let on_over = move |e: web_sys::MouseEvent| {
        let index = e
            .target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
            .and_then(|el| el.get_attribute("data-cell"))
            .and_then(|i| i.parse().ok());
        if index.is_some() {
            hovered.set(index);
        }
    };

    view! {
        <div class="chunkmap-wrap">
            <div
                node_ref=grid
                class="chunkmap"
                style=format!("--cols:{columns}")
                role="img"
                aria-label=summary
                on:mouseover=on_over
                on:mouseleave=move |_| hovered.set(None)
            >
                {move || shades.view()}
            </div>
            <div class="chunkmap__foot">
                <div class="key">
                    <span class="key__item"><span class="cell cell--have"></span>"stored"</span>
                    <span class="key__item"><span class="cell cell--flight"></span>"downloading"</span>
                    <span class="key__item">
                        <span class="cell cell--missing" style="--a:1"></span>
                        "missing, widely available"
                    </span>
                    <span class="key__item">
                        <span class="cell cell--missing" style="--a:0.2"></span>
                        "missing, few peers have it"
                    </span>
                    <span class="key__item"><span class="cell cell--nowhere"></span>"no connected peer has it"</span>
                </div>
                <span class="chunkmap__readout">{readout}</span>
            </div>
        </div>
    }
}

/// A signal per cell, kept in step with the cells by one effect.
#[derive(Clone, Copy)]
struct Shades {
    /// The cells' shades, rebuilt only when their number changes. Reference counted rather
    /// than owned, since the effect that rebuilds them outlives each run.
    cells: StoredValue<Vec<ArcRwSignal<Shade>>>,
    /// Bumped when `cells` is rebuilt, so the view renders the new set.
    generation: RwSignal<u64>,
}

impl Shades {
    fn new(cells: Signal<Vec<ChunkCell>>, running: Signal<bool>) -> Self {
        let shades = Self {
            cells: StoredValue::new(Vec::new()),
            generation: RwSignal::new(0),
        };
        // Seeded now, so the first frame isn't blank while the effect waits to run.
        cells.with_untracked(|c| shades.rebuild(c, running.get_untracked()));
        Effect::new(move |_| {
            let running = running.get();
            cells.with(|c| {
                let resized = shades.cells.with_value(|signals| signals.len() != c.len());
                if resized {
                    shades.rebuild(c, running);
                    shades.generation.update(|g| *g += 1);
                    return;
                }
                shades.cells.with_value(|signals| {
                    for (signal, cell) in signals.iter().zip(c) {
                        let shade = Shade::of(cell, running);
                        if signal.get_untracked() != shade {
                            signal.set(shade);
                        }
                    }
                });
            });
        });
        shades
    }

    fn rebuild(self, cells: &[ChunkCell], running: bool) {
        let signals = cells
            .iter()
            .map(|c| ArcRwSignal::new(Shade::of(c, running)))
            .collect();
        self.cells.set_value(signals);
    }

    /// The cells' elements. Re-renders all of them only when their number changes.
    fn view(self) -> impl IntoView {
        self.generation.track();
        self.cells.with_value(|signals| {
            signals
                .iter()
                .enumerate()
                .map(|(i, shade)| {
                    let (class, style) = (shade.clone(), shade.clone());
                    view! {
                        <span
                            class=move || class.get().class()
                            style=move || style.get().style()
                            data-cell=i
                        ></span>
                    }
                })
                .collect_view()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(have: u32, in_flight: u32, missing: u32, availability: u32) -> ChunkCell {
        ChunkCell {
            have,
            in_flight,
            missing,
            availability,
        }
    }

    #[test]
    fn shades_by_state_then_rarity() {
        assert_eq!(Shade::of(&cell(0, 0, 0, 0), true), Shade::Empty);
        assert_eq!(Shade::of(&cell(4, 0, 0, 3), true), Shade::Have);
        assert_eq!(Shade::of(&cell(1, 1, 2, 3), true), Shade::InFlight);
        assert_eq!(Shade::of(&cell(1, 0, 3, 3), true), Shade::Partial(2));
        assert_eq!(Shade::of(&cell(1, 0, 99, 3), true), Shade::Partial(1));
        assert_eq!(Shade::of(&cell(99, 0, 1, 3), true), Shade::Partial(9));
        assert_eq!(Shade::of(&cell(0, 0, 4, 2), true), Shade::Missing(2));
        assert_eq!(Shade::of(&cell(0, 0, 4, 40), true), Shade::Missing(COMMON));
        assert_eq!(Shade::of(&cell(0, 0, 4, 0), true), Shade::Nowhere);
        assert_eq!(Shade::of(&cell(0, 0, 4, 0), false), Shade::Unknown);
        assert_eq!(Shade::Missing(COMMON).style(), "--a:1.00");
        assert_eq!(Shade::Partial(5).style(), "--p:0.5");
    }

    #[test]
    fn describes_cells() {
        let cells = [
            cell(512, 0, 0, 3),
            cell(100, 12, 400, 2),
            cell(0, 0, 512, 0),
            cell(0, 0, 1, 1),
        ];
        assert_eq!(chunk_range(&cells, 1), (512, 1024));
        assert_eq!(describe(&cells, true, 0), "chunks 0–511 · stored");
        assert_eq!(
            describe(&cells, true, 1),
            "chunks 512–1,023 · 100 stored, 12 downloading, 400 missing · 2 peers have the rarest chunk here"
        );
        assert_eq!(
            describe(&cells, true, 2),
            "chunks 1,024–1,535 · missing · no connected peer has them"
        );
        assert_eq!(
            describe(&cells, true, 3),
            "chunk 1,536 · missing · 1 peer has the rarest chunk here"
        );
        assert_eq!(
            describe(&cells, false, 2),
            "chunks 1,024–1,535 · missing · not running, so peers are unknown"
        );
        assert_eq!(describe(&cells, true, 9), "");
    }
}

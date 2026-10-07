//! Charts drawn by hand in SVG. The geometry is plain Rust, tested without a browser; the
//! components draw it and handle the pointer.

mod bars;
mod line;
mod scale;
mod view;

pub use bars::{Bar, BarPlot, BarShape, plot as plot_bars, shorten};
pub use line::{
    Color, Hover, HoverRow, LineData, LinePlot, Series, SeriesPaths, Unit, XAxis, sparkline, zoom,
};
pub use scale::{Layout, Scale, Tick, ago_ticks, area_path, band_path, line_path, nice_ticks};
pub use view::{BarChart, LineChart, Sparkline, use_width};

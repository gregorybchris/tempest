//! Turns state changes into Datastar SSE patches for one stream.
//!
//! Every wake-up renders the page's fragments and sends only those whose HTML
//! differs from what this stream last sent. The big fragments get finer
//! patches: chunk-map cells one by one, new log entries prepended, and chart
//! pointer feedback removed and re-appended instead of re-sending the paths.

use std::collections::HashMap;

use maud::Markup;
use datastar::prelude::{DatastarEvent, ElementPatchMode, Namespace, PatchElements, PatchSignals};
use proto_core::chart::{self, Layout, RateChart};
use proto_core::{Bundle, Sample, World};

use crate::render::{self, Ui, CHART_HEIGHT, CHART_WIDTH, LOG_LEN};

/// Pointer feedback inside the chart.
const OVERLAY: &str = "#chart > .chart__selection, #chart > .chart__crosshair, #chart > .chart__dot, #rates .chart__tooltip";

/// Above this many changed cells, re-send the whole chunk map.
const MAX_CELL_PATCHES: usize = 200;

#[derive(Default)]
pub struct Patcher {
    /// HTML last sent, by fragment.
    sent: HashMap<&'static str, String>,
    cells: Vec<String>,
    log: Vec<String>,
    /// World tick and pointer-free UI state of the last full update.
    last: Option<(u64, Ui)>,
    /// The chart and its samples as of `last`, for pointer-only updates.
    chart: Option<(RateChart, Vec<Sample>)>,
}

impl Patcher {
    /// The patches that bring this stream's page up to date with tick `tick`.
    pub fn update(&mut self, world: &World, tick: u64, ui: &Ui) -> Vec<DatastarEvent> {
        let b = &world.bundles[ui.selected];
        let mut out = Vec::new();
        let still = Ui { chart_x: None, drag_from: None, cell: None, ..ui.clone() };

        // Only the pointer moved: just the readout and chart overlay.
        if self.last.as_ref() == Some(&(tick, still.clone())) {
            self.readout(&mut out, b, ui);
            self.overlay(&mut out, ui);
            return out;
        }
        self.last = Some((tick, still.clone()));
        let history: Vec<Sample> = b.history.iter().copied().collect();
        let chart = chart::rates(&history, ui.zoom, Layout::rates(CHART_WIDTH, CHART_HEIGHT), ui.visible);
        self.chart = Some((chart, history));

        if self.changed("signals", render::signals(b, ui)) {
            out.push(PatchSignals::new(render::signals(b, ui)).into_datastar_event());
        }
        for (key, html) in ["meter-down", "meter-up", "meter-peers"].into_iter().zip(render::meter_values(world)) {
            self.element(&mut out, key, None, html.into_string());
        }
        self.element(&mut out, "spark-down", Some(".meter--down .spark"), render::meter_spark(world, "down").into_string());
        self.element(&mut out, "spark-up", Some(".meter--up .spark"), render::meter_spark(world, "up").into_string());
        self.element(&mut out, "controls", Some(".controls"), render::controls(ui).into_string());
        self.element(&mut out, "transfers", Some(".transfer-list"), render::transfer_list(world, ui).into_string());
        self.element(&mut out, "summary", None, render::summary(b).into_string());
        self.chunks(&mut out, b, ui);
        if self.changed("rates", render::rates(b, &still).into_string()) {
            out.push(PatchElements::new(render::rates(b, ui).into_string()).into_datastar_event());
            let (svg, tooltip) = self.overlay_html(ui);
            self.sent.insert("overlay", svg.concat() + &tooltip);
        } else {
            self.overlay(&mut out, ui);
        }
        self.element(&mut out, "peers", None, render::peers(b).into_string());
        self.log(&mut out, b, ui);
        out
    }

    /// The chunk map, cell by cell unless the bundle or its scale changed.
    fn chunks(&mut self, out: &mut Vec<DatastarEvent>, b: &Bundle, ui: &Ui) {
        let cells: Vec<String> = b.cells.iter().enumerate().map(|(i, c)| render::cell(i, c).into_string()).collect();
        let changed: Vec<usize> = (0..cells.len()).filter(|&i| self.cells.get(i) != Some(&cells[i])).collect();
        let frame = format!("{} {}", b.id, b.chunks_per_cell());
        if self.changed("chunks", frame) || changed.len() > MAX_CELL_PATCHES {
            out.push(PatchElements::new(render::chunks(b, ui).into_string()).into_datastar_event());
            self.sent.insert("readout", render::readout(b, ui.cell).into_string());
        } else {
            for i in changed {
                let selector = format!("#chunks [data-cell=\"{i}\"]");
                out.push(PatchElements::new(cells[i].clone()).selector(selector).into_datastar_event());
            }
            self.readout(out, b, ui);
        }
        self.cells = cells;
    }

    fn readout(&mut self, out: &mut Vec<DatastarEvent>, b: &Bundle, ui: &Ui) {
        self.element(out, "readout", None, render::readout(b, ui.cell).into_string());
    }

    /// Crosshair, dots, selection and tooltip: removed, then re-appended.
    fn overlay(&mut self, out: &mut Vec<DatastarEvent>, ui: &Ui) {
        let (svg, tooltip) = self.overlay_html(ui);
        let had = self.sent.get("overlay").is_some_and(|s| !s.is_empty());
        if !self.changed("overlay", svg.concat() + &tooltip) {
            return;
        }
        if had {
            out.push(PatchElements::new_remove(OVERLAY).into_datastar_event());
        }
        // One event per SVG element: Datastar 1.0.4 drops every other
        // element of a multi-element SVG-namespace patch.
        for el in svg {
            out.push(append("#chart", el).namespace(Namespace::Svg).into_datastar_event());
        }
        if !tooltip.is_empty() {
            out.push(append("#rates .chart", tooltip).into_datastar_event());
        }
    }

    /// The decision log: new entries are prepended and old ones trimmed.
    fn log(&mut self, out: &mut Vec<DatastarEvent>, b: &Bundle, ui: &Ui) {
        let new: Vec<String> = b.decisions.iter().take(LOG_LEN).map(|d| render::log_entry(d).into_string()).collect();
        let old = std::mem::take(&mut self.log);
        // How many entries were added at the top since last time.
        let added = (0..=new.len()).find(|&k| old.len() >= new.len() - k && new[k..] == old[..new.len() - k]);
        let frame = format!("{} {}", ui.selected, new.is_empty());
        if self.changed("log", frame) || added.is_none() {
            out.push(PatchElements::new(render::log(b).into_string()).into_datastar_event());
        } else if let Some(k @ 1..) = added {
            let html = new[..k].concat();
            out.push(PatchElements::new(html).selector("#log .log").mode(ElementPatchMode::Prepend).into_datastar_event());
            if old.len() + k > LOG_LEN {
                let selector = format!("#log .log > li:nth-child(n+{})", LOG_LEN + 1);
                out.push(PatchElements::new_remove(selector).into_datastar_event());
            }
        }
        self.log = new;
    }

    /// The chart overlay's SVG elements and tooltip, as HTML.
    fn overlay_html(&self, ui: &Ui) -> (Vec<String>, String) {
        let (chart, history) = self.chart.as_ref().expect("set by a full update");
        let overlay = render::chart_overlay(chart, history, ui);
        (overlay.svg.into_iter().map(Markup::into_string).collect(), overlay.tooltip.into_string())
    }

    /// Records `html` as sent under `key`; true if it differs from last time.
    fn changed(&mut self, key: &'static str, html: String) -> bool {
        if self.sent.get(key) == Some(&html) {
            return false;
        }
        self.sent.insert(key, html);
        true
    }

    /// Morphs `html` into the element matching `selector` (or its own id) if it changed.
    fn element(&mut self, out: &mut Vec<DatastarEvent>, key: &'static str, selector: Option<&str>, html: String) {
        if self.changed(key, html.clone()) {
            let patch = PatchElements::new(html);
            let patch = match selector {
                Some(s) => patch.selector(s),
                None => patch,
            };
            out.push(patch.into_datastar_event());
        }
    }
}


fn append(selector: &str, html: String) -> PatchElements {
    PatchElements::new(html).selector(selector).mode(ElementPatchMode::Append)
}

/// Size of an event on the wire, for measurements.
pub fn wire_bytes(e: &DatastarEvent) -> usize {
    e.to_string().len()
}

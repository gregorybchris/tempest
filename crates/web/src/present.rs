//! How transfers read on the page: statuses as badges, numbers as text, rates as charts and
//! chunks as cells. Plain functions, so views stay thin.

use tempest_api::finding::StatusFilter;
use tempest_api::inspect::{CellInfo, PeerInfo};
use tempest_api::model::History;
pub use tempest_api::units::{clock, date};
use tempest_api::{Status, TransferInfo};
use tempest_ui_kit::ChunkCell;
use tempest_ui_kit::chart::{Color, LineData, Series, Unit, XAxis};
use tempest_ui_kit::components::Tone;
use tempest_ui_kit::format;

/// A status's accent.
pub fn tone(status: &Status) -> Tone {
    match status {
        Status::Downloading => Tone::Down,
        Status::Sharing => Tone::Up,
        Status::Stalled { .. } => Tone::Warn,
        Status::Errored { .. } => Tone::Bad,
        Status::Checking | Status::Moving => Tone::Hot,
        Status::Queued | Status::Stopped | Status::Completed => Tone::Dim,
    }
}

/// A status's name, as the list shows it.
pub fn label(status: &Status) -> &'static str {
    StatusFilter::of(status).title()
}

/// A count of bytes, or a dash while unknown.
pub fn size(size: Option<u64>) -> String {
    size.map_or_else(|| "–".to_owned(), format::bytes)
}

/// A rate in bytes per second, or a dash for none.
pub fn rate(bytes_per_second: u64) -> String {
    format::rate(bytes_per_second as f64)
}

/// Time left, or a dash while there is no estimate.
pub fn eta(eta: Option<u64>) -> String {
    eta.map_or_else(|| "–".to_owned(), format::duration)
}

/// A ratio to two decimals, or a dash.
pub fn ratio(ratio: Option<f64>) -> String {
    ratio.map_or_else(|| "–".to_owned(), |r| format!("{r:.2}"))
}

/// Connections out of the peers known: `3/12`.
pub fn peers(transfer: &TransferInfo) -> String {
    format!("{}/{}", transfer.connections, transfer.peers)
}

/// What a connection is doing, in a few words: `sending to us, we're sending`, `connecting`, `no activity`.
pub fn peer_state(peer: &PeerInfo) -> String {
    if !peer.open {
        return "connecting".to_owned();
    }
    let mut parts = Vec::new();
    if peer.snubbed {
        parts.push("snubbed");
    } else if peer.serving_us {
        parts.push("sending to us");
    } else if peer.we_want {
        parts.push("we're requesting");
    }
    if peer.we_serve {
        parts.push("we're sending");
    } else if peer.they_want {
        parts.push("requesting from us");
    }
    match parts.is_empty() {
        true => "no activity".to_owned(),
        false => parts.join(", "),
    }
}

/// The last `count` samples of `history`, for a sparkline: download or upload rates.
pub fn recent(history: &History, count: usize, upload: bool) -> Vec<f64> {
    let samples = history.samples();
    let skip = samples.len().saturating_sub(count);
    samples
        .skip(skip)
        .map(|(down, up)| if upload { up } else { down } as f64)
        .collect()
}

/// Rates over time as a chart: one sample a second, the latest at 0.
pub fn rates_chart(history: &History) -> LineData {
    let count = history.samples().len() as f64;
    let at = |i: usize| (i as f64 + 1.0 - count) * 1000.0;
    let (down, up): (Vec<_>, Vec<_>) = history
        .samples()
        .enumerate()
        .map(|(i, (down, up))| ((at(i), down as f64), (at(i), up as f64)))
        .unzip();
    LineData {
        series: vec![
            Series::new("down", "download", Color::Down, down).filled(),
            Series::new("up", "upload", Color::Up, up).dashed(),
        ],
        x: XAxis::Ago,
        y: Unit::Rate,
        // A quiet transfer's axis still reaches 100 KB/s, so noise doesn't look like a surge.
        y_floor: 100_000.0,
    }
}

/// The engine's chunk map cells as the kit draws them.
pub fn cells(cells: &[CellInfo]) -> Vec<ChunkCell> {
    cells
        .iter()
        .map(|c| ChunkCell {
            have: c.have,
            in_flight: c.in_flight,
            missing: c.missing,
            availability: c.availability,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use tempest_api::ApiEvent;
    use tempest_api::model::Model;

    use super::*;

    #[test]
    fn statuses_read_as_words_with_accents() {
        let stalled = Status::Stalled {
            reason: "No peers found yet.".to_owned(),
        };
        assert_eq!(label(&stalled), "Stalled");
        assert_eq!(tone(&stalled), Tone::Warn);
        assert_eq!(tone(&Status::Downloading), Tone::Down);
        assert_eq!(label(&Status::Queued), "Queued");
    }

    #[test]
    fn numbers_read_compactly_with_dashes_for_unknowns() {
        assert_eq!(size(None), "–");
        assert_eq!(size(Some(4_200_000_000)), "4.20 GB");
        assert_eq!(rate(0), "–");
        assert_eq!(rate(9_800_000), "9.80 MB/s");
        assert_eq!(eta(Some(185)), "3m 05s");
        assert_eq!(ratio(Some(0.4242)), "0.42");
    }

    #[test]
    fn connections_say_who_sends() {
        let peer = PeerInfo {
            peer_id: "ab".repeat(32),
            address: "127.0.0.1:1".to_owned(),
            path: "direct".to_owned(),
            direction: "dialed".to_owned(),
            open: true,
            progress: None,
            we_want: true,
            they_want: false,
            serving_us: true,
            we_serve: true,
            snubbed: false,
            download_rate: 0,
            upload_rate: 0,
            downloaded: 0,
            uploaded: 0,
            rtt_ms: None,
        };
        assert_eq!(peer_state(&peer), "sending to us, we're sending");
        let quiet = PeerInfo {
            serving_us: false,
            we_want: false,
            we_serve: false,
            ..peer.clone()
        };
        assert_eq!(peer_state(&quiet), "no activity");
        assert_eq!(
            peer_state(&PeerInfo {
                open: false,
                ..peer
            }),
            "connecting"
        );
    }

    #[test]
    fn rates_end_now_one_sample_a_second() {
        let mut model = Model::new();
        let mut busy = TransferInfo::example(1, "a", 0);
        busy.download_rate = 5;
        busy.upload_rate = 2;
        let id = busy.id;
        model.apply(ApiEvent::Snapshot(vec![busy]));
        for _ in 0..3 {
            model.sample();
        }
        let chart = rates_chart(model.history(&id).unwrap());
        assert_eq!(
            chart.series[0].points,
            [(-2000.0, 5.0), (-1000.0, 5.0), (0.0, 5.0)]
        );
        assert_eq!(chart.series[1].points[2], (0.0, 2.0));
        assert_eq!(recent(model.total_history(), 2, true), [2.0, 2.0]);
    }
}

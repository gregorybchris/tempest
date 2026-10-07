//! How transfers read on the page: statuses as badges, numbers as text, rates as charts and
//! chunks as cells. Plain functions, so views stay thin.

use tempest_api::finding::StatusFilter;
use tempest_api::inspect::{CellInfo, PeerInfo};
use tempest_api::model::History;
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

/// A moment in milliseconds since the Unix epoch as its date in UTC: `2026-10-07`.
pub fn date(millis: i64) -> String {
    // Howard Hinnant's days-to-civil algorithm, on days since 1970-01-01.
    let days = millis.div_euclid(86_400_000) + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

/// A time on a session's clock, in seconds since it started: `0:07.3`, `1:02:03.5`.
pub fn clock(seconds: f64) -> String {
    let tenths = (seconds.max(0.0) * 10.0).floor() as u64;
    let (whole, tenth) = (tenths / 10, tenths % 10);
    let (hours, minutes, seconds) = (whole / 3600, whole / 60 % 60, whole % 60);
    match hours {
        0 => format!("{minutes}:{seconds:02}.{tenth}"),
        hours => format!("{hours}:{minutes:02}:{seconds:02}.{tenth}"),
    }
}

/// What a connection is doing, in a few words: `sends to us, we send`, `opening`, `idle`.
pub fn peer_state(peer: &PeerInfo) -> String {
    if !peer.open {
        return "opening".to_owned();
    }
    let mut parts = Vec::new();
    if peer.snubbed {
        parts.push("snubbed");
    } else if peer.serving_us {
        parts.push("sends to us");
    } else if peer.we_want {
        parts.push("we want");
    }
    if peer.we_serve {
        parts.push("we send");
    } else if peer.they_want {
        parts.push("it wants");
    }
    match parts.is_empty() {
        true => "idle".to_owned(),
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
        assert_eq!(date(1_791_331_200_000), "2026-10-07");
        assert_eq!(date(-1), "1969-12-31");
        assert_eq!(clock(3723.5), "1:02:03.5");
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
        assert_eq!(peer_state(&peer), "sends to us, we send");
        let quiet = PeerInfo {
            serving_us: false,
            we_want: false,
            we_serve: false,
            ..peer.clone()
        };
        assert_eq!(peer_state(&quiet), "idle");
        assert_eq!(
            peer_state(&PeerInfo {
                open: false,
                ..peer
            }),
            "opening"
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

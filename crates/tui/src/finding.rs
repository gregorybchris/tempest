//! Finding transfers (D-118): filtering by name, tag and status, and sorting.

use std::cmp::Ordering;

use tempest_engine::{Status, TransferInfo};

/// A status to filter by. Stalled and errored transfers match whatever their reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusFilter {
    /// Running and receiving data.
    Downloading,
    /// Complete and serving.
    Sharing,
    /// Running but getting nowhere.
    Stalled,
    /// Waiting for the queue.
    Queued,
    /// Stopped by the user.
    Stopped,
    /// Complete, and stopped sharing on its own.
    Completed,
    /// Stopped by a failure.
    Errored,
    /// Having its data checked.
    Checking,
    /// Having its data moved.
    Moving,
}

impl StatusFilter {
    /// Every status, in the order statuses are listed elsewhere.
    pub const ALL: [Self; 9] = [
        Self::Downloading,
        Self::Sharing,
        Self::Stalled,
        Self::Queued,
        Self::Stopped,
        Self::Completed,
        Self::Errored,
        Self::Checking,
        Self::Moving,
    ];

    /// The filter that `status` matches.
    pub fn of(status: &Status) -> Self {
        match status {
            Status::Downloading => Self::Downloading,
            Status::Sharing => Self::Sharing,
            Status::Stalled { .. } => Self::Stalled,
            Status::Queued => Self::Queued,
            Status::Stopped => Self::Stopped,
            Status::Completed => Self::Completed,
            Status::Errored { .. } => Self::Errored,
            Status::Checking => Self::Checking,
            Status::Moving => Self::Moving,
        }
    }

    /// Its name, capitalized as the list shows statuses.
    pub fn title(self) -> &'static str {
        match self {
            Self::Downloading => "Downloading",
            Self::Sharing => "Sharing",
            Self::Stalled => "Stalled",
            Self::Queued => "Queued",
            Self::Stopped => "Stopped",
            Self::Completed => "Completed",
            Self::Errored => "Errored",
            Self::Checking => "Checking",
            Self::Moving => "Moving",
        }
    }

    /// Its name in a saved view: the status's own, such as `downloading`.
    pub fn id(self) -> &'static str {
        match self {
            Self::Downloading => "downloading",
            Self::Sharing => "sharing",
            Self::Stalled => "stalled",
            Self::Queued => "queued",
            Self::Stopped => "stopped",
            Self::Completed => "completed",
            Self::Errored => "errored",
            Self::Checking => "checking",
            Self::Moving => "moving",
        }
    }

    /// The status named `id` in a saved view.
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|status| status.id() == id)
    }

    fn rank(self) -> usize {
        Self::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }
}

/// What the list can be sorted by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    /// Queue order, the default.
    Queue,
    /// Name, A to Z.
    Name,
    /// Size, largest first.
    Size,
    /// Progress, furthest first.
    Progress,
    /// Status, in [`StatusFilter::ALL`]'s order.
    Status,
    /// Download rate, fastest first.
    Down,
    /// Upload rate, fastest first.
    Up,
    /// Connected peers, most first.
    Peers,
    /// Time left, soonest first.
    TimeLeft,
    /// Ratio, highest first.
    Ratio,
    /// Date added, newest first.
    Added,
}

impl SortKey {
    /// Every key, as the sort picker lists them.
    pub const ALL: [Self; 11] = [
        Self::Queue,
        Self::Name,
        Self::Size,
        Self::Progress,
        Self::Status,
        Self::Down,
        Self::Up,
        Self::Peers,
        Self::TimeLeft,
        Self::Ratio,
        Self::Added,
    ];

    /// Its name.
    pub fn title(self) -> &'static str {
        match self {
            Self::Queue => "queue",
            Self::Name => "name",
            Self::Size => "size",
            Self::Progress => "progress",
            Self::Status => "status",
            Self::Down => "↓",
            Self::Up => "↑",
            Self::Peers => "peers",
            Self::TimeLeft => "time left",
            Self::Ratio => "ratio",
            Self::Added => "date added",
        }
    }

    /// Its name in a saved view, such as `time_left`.
    pub fn id(self) -> &'static str {
        match self {
            Self::Queue => "queue",
            Self::Name => "name",
            Self::Size => "size",
            Self::Progress => "progress",
            Self::Status => "status",
            Self::Down => "down",
            Self::Up => "up",
            Self::Peers => "peers",
            Self::TimeLeft => "time_left",
            Self::Ratio => "ratio",
            Self::Added => "added",
        }
    }

    /// The key named `id` in a saved view.
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|key| key.id() == id)
    }

    /// Compares two transfers in this key's natural direction. Unknown values (a size before
    /// the manifest, a time left while not downloading) compare as `None`.
    fn compare(self, a: &TransferInfo, b: &TransferInfo) -> Ordering {
        fn known<T: PartialOrd>(a: Option<T>, b: Option<T>, descending: bool) -> Ordering {
            match (a, b) {
                (Some(a), Some(b)) => {
                    let order = a.partial_cmp(&b).unwrap_or(Ordering::Equal);
                    if descending { order.reverse() } else { order }
                }
                // Unknowns go last whichever way the list runs; see `Sort::compare`.
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => Ordering::Equal,
            }
        }
        match self {
            Self::Queue => a.queue_position.cmp(&b.queue_position),
            Self::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            Self::Size => known(a.size, b.size, true),
            Self::Progress => b.progress.total_cmp(&a.progress),
            Self::Status => StatusFilter::of(&a.status)
                .rank()
                .cmp(&StatusFilter::of(&b.status).rank()),
            Self::Down => b.download_rate.cmp(&a.download_rate),
            Self::Up => b.upload_rate.cmp(&a.upload_rate),
            Self::Peers => b.connections.cmp(&a.connections),
            Self::TimeLeft => known(a.eta, b.eta, false),
            Self::Ratio => known(a.ratio, b.ratio, true),
            Self::Added => b.added.cmp(&a.added),
        }
    }

    /// Whether the key can leave a value unknown, which sorts last either way.
    fn has_unknowns(self) -> bool {
        matches!(self, Self::Size | Self::TimeLeft | Self::Ratio)
    }
}

/// How the list is sorted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sort {
    /// What by.
    pub key: SortKey,
    /// Against the key's natural direction.
    pub reversed: bool,
}

impl Default for Sort {
    fn default() -> Self {
        Self {
            key: SortKey::Queue,
            reversed: false,
        }
    }
}

impl Sort {
    /// Compares two transfers, breaking ties by queue order so the order is total and stable.
    pub fn compare(&self, a: &TransferInfo, b: &TransferInfo) -> Ordering {
        let unknown = |t: &TransferInfo| match self.key {
            SortKey::Size => t.size.is_none(),
            SortKey::TimeLeft => t.eta.is_none(),
            SortKey::Ratio => t.ratio.is_none(),
            _ => false,
        };
        let order = if self.key.has_unknowns() && unknown(a) != unknown(b) {
            // Unknowns last, even reversed.
            unknown(a).cmp(&unknown(b))
        } else if self.reversed {
            self.key.compare(a, b).reverse()
        } else {
            self.key.compare(a, b)
        };
        order.then(a.queue_position.cmp(&b.queue_position))
    }

    /// Its description, as the list's header shows it: "size ▼" for largest first.
    pub fn describe(&self) -> String {
        let arrow = if self.ascending() { "▲" } else { "▼" };
        format!("{} {arrow}", self.key.title())
    }

    /// Whether the list runs from small to large (or A to Z, or first to last).
    pub fn ascending(&self) -> bool {
        let natural = matches!(
            self.key,
            SortKey::Queue | SortKey::Name | SortKey::Status | SortKey::TimeLeft
        );
        natural != self.reversed
    }
}

/// Which transfers the list shows, and in what order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Finding {
    /// Words that must all match: `#word` a tag, any other word the name. Case is ignored and
    /// a word matches anywhere within.
    pub text: String,
    /// The status to show only, if any.
    pub status: Option<StatusFilter>,
    /// The order.
    pub sort: Sort,
}

impl Finding {
    /// Whether anything is filtered out.
    pub fn filtering(&self) -> bool {
        !self.text.trim().is_empty() || self.status.is_some()
    }

    /// Whether `transfer` passes the filters.
    pub fn matches(&self, transfer: &TransferInfo) -> bool {
        if self
            .status
            .is_some_and(|status| status != StatusFilter::of(&transfer.status))
        {
            return false;
        }
        let name = transfer.name.to_lowercase();
        self.text.split_whitespace().all(|word| {
            let word = word.to_lowercase();
            match word.strip_prefix('#') {
                Some(tag) => transfer.tags.iter().any(|t| t.to_lowercase().contains(tag)),
                None => name.contains(&word),
            }
        })
    }

    /// The transfers that pass the filters, sorted.
    pub fn apply<'a>(&self, transfers: &'a [TransferInfo]) -> Vec<&'a TransferInfo> {
        let mut shown: Vec<&TransferInfo> = transfers.iter().filter(|t| self.matches(t)).collect();
        shown.sort_by(|a, b| self.sort.compare(a, b));
        shown
    }

    /// The filters and order in a few words, for the list's header: `/ens #live · Stalled ·
    /// size ▼`. Empty when the list shows everything in queue order.
    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        if !self.text.trim().is_empty() {
            parts.push(format!("/{}", self.text.trim()));
        }
        if let Some(status) = self.status {
            parts.push(status.title().to_owned());
        }
        if self.sort != Sort::default() {
            parts.push(format!("by {}", self.sort.describe()));
        }
        parts.join(" · ")
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::model::tests::transfer;

    fn names<'a>(shown: &[&'a TransferInfo]) -> Vec<&'a str> {
        shown.iter().map(|t| t.name.as_str()).collect()
    }

    fn sample() -> Vec<TransferInfo> {
        let mut a = transfer(1, "Ensemble-2026.tar", 0);
        a.size = Some(4_000);
        a.status = Status::Downloading;
        a.eta = Some(60);
        a.tags = ["music".to_owned(), "live".to_owned()].into();
        a.added = 3;
        let mut b = transfer(2, "field-recordings", 1);
        b.size = Some(9_000);
        b.status = Status::Sharing;
        b.tags = ["music".to_owned()].into();
        b.added = 1;
        let mut c = transfer(3, "corpus", 2);
        c.status = Status::Stalled {
            reason: "No peers.".to_owned(),
        };
        c.added = 2;
        vec![a, b, c]
    }

    #[test]
    fn words_match_names_and_hash_words_match_tags() {
        let transfers = sample();
        let find = |text: &str| {
            let finding = Finding {
                text: text.to_owned(),
                ..Finding::default()
            };
            names(&finding.apply(&transfers))
        };
        assert_eq!(
            find(""),
            ["Ensemble-2026.tar", "field-recordings", "corpus"]
        );
        assert_eq!(find("ENS"), ["Ensemble-2026.tar"]);
        assert_eq!(find("#mus"), ["Ensemble-2026.tar", "field-recordings"]);
        assert_eq!(find("#music rec"), ["field-recordings"]);
        assert_eq!(find("#live #none"), Vec::<&str>::new());
    }

    #[test]
    fn status_filters_ignore_reasons() {
        let transfers = sample();
        let finding = Finding {
            status: Some(StatusFilter::Stalled),
            ..Finding::default()
        };
        assert_eq!(names(&finding.apply(&transfers)), ["corpus"]);
        assert_eq!(finding.describe(), "Stalled");
    }

    #[test]
    fn sorts_run_in_natural_directions_with_unknowns_last() {
        let transfers = sample();
        let sorted = |key, reversed| {
            let finding = Finding {
                sort: Sort { key, reversed },
                ..Finding::default()
            };
            names(&finding.apply(&transfers))
        };
        assert_eq!(
            sorted(SortKey::Size, false),
            ["field-recordings", "Ensemble-2026.tar", "corpus"]
        );
        assert_eq!(
            sorted(SortKey::Size, true),
            ["Ensemble-2026.tar", "field-recordings", "corpus"]
        );
        assert_eq!(
            sorted(SortKey::Name, false),
            ["corpus", "Ensemble-2026.tar", "field-recordings"]
        );
        assert_eq!(
            sorted(SortKey::Added, false),
            ["Ensemble-2026.tar", "corpus", "field-recordings"]
        );
        assert_eq!(
            sorted(SortKey::TimeLeft, true)[0],
            "Ensemble-2026.tar",
            "the only known time left stays first"
        );
        let size = Sort {
            key: SortKey::Size,
            reversed: false,
        };
        assert_eq!(size.describe(), "size ▼");
        assert_eq!(
            Finding {
                text: " ens ".to_owned(),
                sort: size,
                ..Finding::default()
            }
            .describe(),
            "/ens · by size ▼"
        );
    }

    fn any_transfer() -> impl Strategy<Value = TransferInfo> {
        (
            any::<u8>(),
            "[a-c ]{0,4}",
            proptest::collection::btree_set("[a-c]{1,2}", 0..3),
            0usize..StatusFilter::ALL.len(),
            proptest::option::of(0u64..5),
            0u64..3,
            proptest::option::of(0.0f64..2.0),
        )
            .prop_map(|(seed, name, tags, status, size, rate, ratio)| {
                let mut t = transfer(seed, &name, u32::from(seed));
                t.tags = tags;
                t.status = match StatusFilter::ALL[status] {
                    StatusFilter::Stalled => Status::Stalled {
                        reason: String::new(),
                    },
                    StatusFilter::Errored => Status::Errored {
                        reason: String::new(),
                    },
                    StatusFilter::Downloading => Status::Downloading,
                    StatusFilter::Sharing => Status::Sharing,
                    StatusFilter::Queued => Status::Queued,
                    StatusFilter::Stopped => Status::Stopped,
                    StatusFilter::Completed => Status::Completed,
                    StatusFilter::Checking => Status::Checking,
                    StatusFilter::Moving => Status::Moving,
                };
                t.size = size;
                t.eta = size;
                t.download_rate = rate;
                t.ratio = ratio;
                t
            })
    }

    fn any_finding() -> impl Strategy<Value = Finding> {
        (
            "[a-c #]{0,5}",
            proptest::option::of(0usize..StatusFilter::ALL.len()),
            0usize..SortKey::ALL.len(),
            any::<bool>(),
        )
            .prop_map(|(text, status, key, reversed)| Finding {
                text,
                status: status.map(|s| StatusFilter::ALL[s]),
                sort: Sort {
                    key: SortKey::ALL[key],
                    reversed,
                },
            })
    }

    proptest! {
        /// The list shows exactly the matching transfers, each once, in an order consistent
        /// with the sort.
        #[test]
        fn filtering_hides_only_non_matches_and_sorting_permutes(
            transfers in proptest::collection::vec(any_transfer(), 0..12),
            finding in any_finding(),
        ) {
            let shown = finding.apply(&transfers);
            prop_assert!(shown.iter().all(|t| finding.matches(t)));
            let mut expected: Vec<*const TransferInfo> =
                transfers.iter().filter(|t| finding.matches(t)).map(|t| t as *const _).collect();
            let mut got: Vec<*const TransferInfo> = shown.iter().map(|t| *t as *const _).collect();
            expected.sort();
            got.sort();
            prop_assert_eq!(expected, got);
            for pair in shown.windows(2) {
                prop_assert_ne!(finding.sort.compare(pair[0], pair[1]), Ordering::Greater);
            }
        }
    }
}

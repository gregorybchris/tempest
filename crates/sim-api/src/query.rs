//! The API's query strings: filters and sorts of experiments and runs, and the variants a
//! comparison selects. The UI writes them and the server reads them, so the two always agree.
//!
//! Each query writes only what differs from its default, so links stay short, and reads
//! `key=value` pairs, percent-encoded, ignoring keys it doesn't know. A value it can't read is
//! an error that names the key.

use std::fmt;

/// The most runs a page holds.
pub const MAX_LIMIT: u32 = 1000;
/// Runs a page holds unless asked for another number.
pub const DEFAULT_LIMIT: u32 = 200;
/// The most variants a comparison takes.
pub const MAX_COMPARED: usize = 12;

/// Why a query string can't be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueryError {
    /// The key whose value is wrong.
    pub key: String,
    /// What is wrong with it.
    pub problem: String,
}

impl fmt::Display for QueryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.key, self.problem)
    }
}

impl std::error::Error for QueryError {}

fn error(key: &str, problem: impl Into<String>) -> QueryError {
    QueryError {
        key: key.to_owned(),
        problem: problem.into(),
    }
}

/// `text` with every byte but the unreserved ones (RFC 3986) percent-encoded.
pub fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// `text` with percent-escapes decoded and `+` read as a space. Escapes that aren't two hex
/// digits stay as they are, and bytes that aren't UTF-8 become U+FFFD.
pub fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let escape = match bytes.get(i + 1..i + 3) {
            Some(&[high, low]) if bytes[i] == b'%' => hex(high).zip(hex(low)),
            _ => None,
        };
        match (bytes[i], escape) {
            (_, Some((high, low))) => {
                out.push(high << 4 | low);
                i += 2;
            }
            (b'+', None) => out.push(b' '),
            (byte, None) => out.push(byte),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(byte: u8) -> Option<u8> {
    (byte as char).to_digit(16).map(|d| d as u8)
}

/// The decoded `key=value` pairs of `query`, with or without its leading `?`. A pair without
/// `=` has an empty value.
pub fn pairs(query: &str) -> Vec<(String, String)> {
    query
        .strip_prefix('?')
        .unwrap_or(query)
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            (decode(key), decode(value))
        })
        .collect()
}

/// Writes `pairs` as a query string, without the `?`.
fn join(pairs: &[(&str, String)]) -> String {
    pairs
        .iter()
        .map(|(key, value)| format!("{key}={}", encode(value)))
        .collect::<Vec<_>>()
        .join("&")
}

fn number<T: std::str::FromStr>(key: &str, value: &str) -> Result<T, QueryError> {
    value
        .parse()
        .map_err(|_| error(key, format!("{value:?} isn't a whole number in range")))
}

/// A choice written in queries by a fixed name.
pub trait Named: Copy + PartialEq + Sized + 'static {
    /// Every choice, in the order pickers show them.
    const ALL: &'static [Self];

    /// Its name in queries.
    fn key(self) -> &'static str;

    /// The choice named `key`.
    fn named(key: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|c| c.key() == key)
    }
}

fn named<T: Named>(key: &str, value: &str) -> Result<T, QueryError> {
    T::named(value).ok_or_else(|| {
        let known: Vec<&str> = T::ALL.iter().map(|c| c.key()).collect();
        error(key, format!("{value:?} isn't one of {}", known.join(", ")))
    })
}

fn order(key: &str, value: &str) -> Result<bool, QueryError> {
    match value {
        "asc" => Ok(false),
        "desc" => Ok(true),
        _ => Err(error(key, format!("{value:?} isn't asc or desc"))),
    }
}

fn order_name(descending: bool) -> String {
    if descending { "desc" } else { "asc" }.to_owned()
}

/// What experiments are sorted by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExperimentSort {
    /// Their IDs, which follow the order they were recorded in.
    Id,
    /// The scenario's name.
    Name,
    /// How many variants.
    Variants,
    /// How many runs.
    Runs,
    /// The most peers in a run.
    Peers,
}

impl Named for ExperimentSort {
    const ALL: &'static [Self] = &[
        Self::Id,
        Self::Name,
        Self::Variants,
        Self::Runs,
        Self::Peers,
    ];

    fn key(self) -> &'static str {
        match self {
            Self::Id => "id",
            Self::Name => "name",
            Self::Variants => "variants",
            Self::Runs => "runs",
            Self::Peers => "peers",
        }
    }
}

/// Which experiments to list, and in what order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExperimentQuery {
    /// Text the name, description, scenario file or commit must contain, ignoring case.
    pub search: String,
    /// What to sort by.
    pub sort: ExperimentSort,
    /// Largest first.
    pub descending: bool,
}

impl Default for ExperimentQuery {
    /// Every experiment, newest first.
    fn default() -> Self {
        Self {
            search: String::new(),
            sort: ExperimentSort::Id,
            descending: true,
        }
    }
}

impl ExperimentQuery {
    /// Reads a query string.
    pub fn parse(query: &str) -> Result<Self, QueryError> {
        let mut out = Self::default();
        for (key, value) in pairs(query) {
            match key.as_str() {
                "q" => out.search = value,
                "sort" => out.sort = named(&key, &value)?,
                "order" => out.descending = order(&key, &value)?,
                _ => {}
            }
        }
        Ok(out)
    }

    /// Writes the query string, without the `?`, leaving out defaults.
    pub fn to_query(&self) -> String {
        let default = Self::default();
        let mut pairs = Vec::new();
        if !self.search.is_empty() {
            pairs.push(("q", self.search.clone()));
        }
        if self.sort != default.sort {
            pairs.push(("sort", self.sort.key().to_owned()));
        }
        if self.descending != default.descending {
            pairs.push(("order", order_name(self.descending)));
        }
        join(&pairs)
    }
}

/// What runs are sorted by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunSort {
    /// Their IDs.
    Id,
    /// Their experiment's name.
    Experiment,
    /// Their variant's name.
    Variant,
    /// Their seed.
    Seed,
    /// The fraction of downloaders that completed.
    Completed,
    /// The median completion time.
    CompletionP50,
    /// The 90th percentile completion time.
    CompletionP90,
    /// The median time to first byte.
    FirstByteP50,
    /// The upload utilization.
    Utilization,
    /// The fairness index.
    Fairness,
    /// The rarest chunk's mean availability.
    Rarest,
    /// Overhead bytes.
    Overhead,
    /// Endgame duplicate bytes.
    Duplicate,
    /// When the run ended.
    End,
}

impl Named for RunSort {
    const ALL: &'static [Self] = &[
        Self::Id,
        Self::Experiment,
        Self::Variant,
        Self::Seed,
        Self::Completed,
        Self::CompletionP50,
        Self::CompletionP90,
        Self::FirstByteP50,
        Self::Utilization,
        Self::Fairness,
        Self::Rarest,
        Self::Overhead,
        Self::Duplicate,
        Self::End,
    ];

    fn key(self) -> &'static str {
        match self {
            Self::Id => "id",
            Self::Experiment => "experiment",
            Self::Variant => "variant",
            Self::Seed => "seed",
            Self::Completed => "completed",
            Self::CompletionP50 => "completion_p50",
            Self::CompletionP90 => "completion_p90",
            Self::FirstByteP50 => "first_byte_p50",
            Self::Utilization => "utilization",
            Self::Fairness => "fairness",
            Self::Rarest => "rarest",
            Self::Overhead => "overhead",
            Self::Duplicate => "duplicate",
            Self::End => "end",
        }
    }
}

/// Which runs to show by how many of their downloaders completed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Outcome {
    /// Every run.
    #[default]
    Any,
    /// Runs where every downloader completed.
    Complete,
    /// Runs where some downloader didn't.
    Incomplete,
}

impl Named for Outcome {
    const ALL: &'static [Self] = &[Self::Any, Self::Complete, Self::Incomplete];

    fn key(self) -> &'static str {
        match self {
            Self::Any => "any",
            Self::Complete => "complete",
            Self::Incomplete => "incomplete",
        }
    }
}

/// Which runs to list, in what order, and which page of them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunQuery {
    /// Only this experiment's runs.
    pub experiment: Option<i64>,
    /// Only this variant's, by its index; meaningful with an experiment.
    pub variant: Option<u32>,
    /// Only runs with this seed.
    pub seed: Option<u64>,
    /// Text the experiment's or variant's name must contain, ignoring case.
    pub search: String,
    /// Only runs whose downloaders all completed, or not.
    pub outcome: Outcome,
    /// What to sort by.
    pub sort: RunSort,
    /// Largest first.
    pub descending: bool,
    /// The most runs on the page, from 1 to [`MAX_LIMIT`].
    pub limit: u32,
    /// How many matching runs come before the page.
    pub offset: u64,
}

impl Default for RunQuery {
    /// The first page of every run, in the order they were recorded.
    fn default() -> Self {
        Self {
            experiment: None,
            variant: None,
            seed: None,
            search: String::new(),
            outcome: Outcome::Any,
            sort: RunSort::Id,
            descending: false,
            limit: DEFAULT_LIMIT,
            offset: 0,
        }
    }
}

impl RunQuery {
    /// Reads a query string.
    pub fn parse(query: &str) -> Result<Self, QueryError> {
        let mut out = Self::default();
        for (key, value) in pairs(query) {
            match key.as_str() {
                "experiment" => out.experiment = Some(number(&key, &value)?),
                "variant" => out.variant = Some(number(&key, &value)?),
                "seed" => out.seed = Some(number(&key, &value)?),
                "q" => out.search = value,
                "outcome" => out.outcome = named(&key, &value)?,
                "sort" => out.sort = named(&key, &value)?,
                "order" => out.descending = order(&key, &value)?,
                "limit" => {
                    out.limit = number(&key, &value)?;
                    if !(1..=MAX_LIMIT).contains(&out.limit) {
                        return Err(error(&key, format!("must be from 1 to {MAX_LIMIT}")));
                    }
                }
                "offset" => out.offset = number(&key, &value)?,
                _ => {}
            }
        }
        Ok(out)
    }

    /// Writes the query string, without the `?`, leaving out defaults.
    pub fn to_query(&self) -> String {
        let default = Self::default();
        let mut pairs = Vec::new();
        if let Some(experiment) = self.experiment {
            pairs.push(("experiment", experiment.to_string()));
        }
        if let Some(variant) = self.variant {
            pairs.push(("variant", variant.to_string()));
        }
        if let Some(seed) = self.seed {
            pairs.push(("seed", seed.to_string()));
        }
        if !self.search.is_empty() {
            pairs.push(("q", self.search.clone()));
        }
        if self.outcome != default.outcome {
            pairs.push(("outcome", self.outcome.key().to_owned()));
        }
        if self.sort != default.sort {
            pairs.push(("sort", self.sort.key().to_owned()));
        }
        if self.descending != default.descending {
            pairs.push(("order", order_name(self.descending)));
        }
        if self.limit != default.limit {
            pairs.push(("limit", self.limit.to_string()));
        }
        if self.offset != default.offset {
            pairs.push(("offset", self.offset.to_string()));
        }
        join(&pairs)
    }
}

/// A variant of an experiment, written `EXPERIMENT:VARIANT`, as in `3:1`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VariantRef {
    /// The experiment's ID.
    pub experiment: i64,
    /// The variant's index in it.
    pub variant: u32,
}

impl fmt::Display for VariantRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.experiment, self.variant)
    }
}

impl std::str::FromStr for VariantRef {
    type Err = ();

    fn from_str(text: &str) -> Result<Self, ()> {
        let (experiment, variant) = text.split_once(':').ok_or(())?;
        Ok(Self {
            experiment: experiment.parse().map_err(|_| ())?,
            variant: variant.parse().map_err(|_| ())?,
        })
    }
}

/// The variants to compare, and which of them the others are compared with.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CompareQuery {
    /// From 1 to [`MAX_COMPARED`] variants, each once.
    pub variants: Vec<VariantRef>,
    /// The position of the reference among them.
    pub reference: usize,
}

impl CompareQuery {
    /// Every variant of `experiment`, the first the reference.
    pub fn experiment(experiment: i64, variants: u32) -> Self {
        Self {
            variants: (0..variants)
                .map(|variant| VariantRef {
                    experiment,
                    variant,
                })
                .collect(),
            reference: 0,
        }
    }

    /// Reads a query string; it must name some variants.
    pub fn parse(query: &str) -> Result<Self, QueryError> {
        let mut out = Self::default();
        let mut named = false;
        for (key, value) in pairs(query) {
            match key.as_str() {
                "variants" => {
                    named = true;
                    out.variants = value
                        .split(',')
                        .map(|v| {
                            v.parse().map_err(|()| {
                                error(&key, format!("{v:?} isn't EXPERIMENT:VARIANT"))
                            })
                        })
                        .collect::<Result<_, _>>()?;
                }
                "reference" => out.reference = number(&key, &value)?,
                _ => {}
            }
        }
        if !named || out.variants.is_empty() {
            return Err(error(
                "variants",
                "name at least one, as EXPERIMENT:VARIANT",
            ));
        }
        if out.variants.len() > MAX_COMPARED {
            return Err(error(
                "variants",
                format!("at most {MAX_COMPARED} can be compared"),
            ));
        }
        let mut sorted = out.variants.clone();
        sorted.sort();
        if sorted.windows(2).any(|w| w[0] == w[1]) {
            return Err(error("variants", "each may be named only once"));
        }
        if out.reference >= out.variants.len() {
            return Err(error(
                "reference",
                "must be the position of a named variant",
            ));
        }
        Ok(out)
    }

    /// Writes the query string, without the `?`.
    pub fn to_query(&self) -> String {
        let variants: Vec<String> = self.variants.iter().map(ToString::to_string).collect();
        let mut pairs = vec![("variants", variants.join(","))];
        if self.reference != 0 {
            pairs.push(("reference", self.reference.to_string()));
        }
        // Commas and colons read well in links and are safe in a query's value.
        join(&pairs).replace("%2C", ",").replace("%3A", ":")
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn percent_encoding_round_trips_and_tolerates_stray_escapes() {
        assert_eq!(encode("a b&c=d/é"), "a%20b%26c%3Dd%2F%C3%A9");
        assert_eq!(decode("a%20b+c%26"), "a b c&");
        assert_eq!(decode("100%"), "100%");
        assert_eq!(decode("%zz%4"), "%zz%4");
        assert_eq!(decode("%FF"), "\u{FFFD}");
    }

    #[test]
    fn defaults_write_nothing() {
        assert_eq!(ExperimentQuery::default().to_query(), "");
        assert_eq!(RunQuery::default().to_query(), "");
        assert_eq!(RunQuery::parse("").unwrap(), RunQuery::default());
        assert_eq!(RunQuery::parse("?").unwrap(), RunQuery::default());
    }

    #[test]
    fn run_queries_read_what_they_write() {
        let query = RunQuery {
            experiment: Some(3),
            variant: Some(1),
            seed: Some(7),
            search: "flash crowd".to_owned(),
            outcome: Outcome::Incomplete,
            sort: RunSort::CompletionP90,
            descending: true,
            limit: 50,
            offset: 100,
        };
        let text = query.to_query();
        assert_eq!(
            text,
            "experiment=3&variant=1&seed=7&q=flash%20crowd&outcome=incomplete&sort=completion_p90&order=desc&limit=50&offset=100"
        );
        assert_eq!(RunQuery::parse(&text).unwrap(), query);
    }

    #[test]
    fn bad_values_name_their_key_and_unknown_keys_are_ignored() {
        assert_eq!(RunQuery::parse("seed=-1").unwrap_err().key, "seed");
        assert_eq!(RunQuery::parse("limit=0").unwrap_err().key, "limit");
        assert_eq!(RunQuery::parse("limit=1001").unwrap_err().key, "limit");
        let sort = RunQuery::parse("sort=drop table").unwrap_err();
        assert!(
            sort.to_string()
                .starts_with("sort: \"drop table\" isn't one of id,")
        );
        assert_eq!(RunQuery::parse("order=up").unwrap_err().key, "order");
        assert_eq!(RunQuery::parse("colour=blue&seed=2").unwrap().seed, Some(2));
    }

    #[test]
    fn comparisons_need_distinct_variants_and_a_reference_among_them() {
        let query = CompareQuery::parse("variants=3:0,3:1,5:0&reference=2").unwrap();
        assert_eq!(
            query.variants[2],
            VariantRef {
                experiment: 5,
                variant: 0
            }
        );
        assert_eq!(query.to_query(), "variants=3:0,3:1,5:0&reference=2");
        assert_eq!(
            CompareQuery::parse("variants=3%3A0%2C3%3A1")
                .unwrap()
                .variants
                .len(),
            2
        );
        for bad in [
            "",
            "variants=",
            "variants=3",
            "variants=3:0,3:0",
            "variants=3:0&reference=1",
            "variants=a:b",
        ] {
            assert!(CompareQuery::parse(bad).is_err(), "{bad}");
        }
        let many: Vec<String> = (0..=MAX_COMPARED).map(|v| format!("1:{v}")).collect();
        assert!(CompareQuery::parse(&format!("variants={}", many.join(","))).is_err());
        assert_eq!(
            CompareQuery::experiment(4, 2).to_query(),
            "variants=4:0,4:1"
        );
    }

    fn named_choice<T: Named + fmt::Debug>() -> impl Strategy<Value = T> {
        proptest::sample::select(T::ALL)
    }

    fn run_query() -> impl Strategy<Value = RunQuery> {
        (
            proptest::option::of(any::<i64>()),
            proptest::option::of(any::<u32>()),
            proptest::option::of(any::<u64>()),
            ".*",
            named_choice::<Outcome>(),
            named_choice::<RunSort>(),
            any::<bool>(),
            1..=MAX_LIMIT,
            any::<u64>(),
        )
            .prop_map(
                |(experiment, variant, seed, search, outcome, sort, descending, limit, offset)| {
                    RunQuery {
                        experiment,
                        variant,
                        seed,
                        search,
                        outcome,
                        sort,
                        descending,
                        limit,
                        offset,
                    }
                },
            )
    }

    proptest! {
        #[test]
        fn any_text_decodes_back_from_its_encoding(text in ".*") {
            prop_assert_eq!(decode(&encode(&text)), text);
        }

        #[test]
        fn any_run_query_round_trips(query in run_query()) {
            prop_assert_eq!(RunQuery::parse(&query.to_query()).unwrap(), query);
        }

        #[test]
        fn any_experiment_query_round_trips(
            search in ".*",
            sort in named_choice::<ExperimentSort>(),
            descending in any::<bool>(),
        ) {
            let query = ExperimentQuery { search, sort, descending };
            prop_assert_eq!(ExperimentQuery::parse(&query.to_query()).unwrap(), query);
        }

        #[test]
        fn any_comparison_round_trips(
            variants in proptest::collection::btree_set((any::<i64>(), any::<u32>()), 1..=MAX_COMPARED),
            pick in any::<usize>(),
        ) {
            let variants: Vec<VariantRef> = variants
                .into_iter()
                .map(|(experiment, variant)| VariantRef { experiment, variant })
                .collect();
            let query = CompareQuery { reference: pick % variants.len(), variants };
            prop_assert_eq!(CompareQuery::parse(&query.to_query()).unwrap(), query);
        }

        #[test]
        fn any_text_parses_or_fails_without_panicking(text in ".*") {
            let _ = RunQuery::parse(&text);
            let _ = ExperimentQuery::parse(&text);
            let _ = CompareQuery::parse(&text);
        }
    }
}

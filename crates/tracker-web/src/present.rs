//! How the tracker's listings read: names, swarm health, ages, votes, excerpts, search highlights, pages,
//! and the commands and links the page hands out. Plain functions, tested natively.

use tempest_protocol::BundleId;
use tempest_protocol::link::Link;
use tempest_protocol::tracker::BundleStats;
use tempest_tracker_api::Category;
use tempest_tracker_api::listing::{ListingEdit, Person, VoteCounts};
use tempest_tracker_api::query::Sort;
use tempest_tracker_api::text::{self, TextError};
use tempest_ui_kit::components::Tone;
use tempest_ui_kit::format;

/// `n` and `word`, made plural unless `n` is one: `1 file`, `3 files`.
pub fn plural(n: u64, word: &str) -> String {
    match n {
        1 => format!("1 {word}"),
        n => format!("{} {word}s", format::count(n)),
    }
}

/// A person's chosen name, or else the start of their key.
pub fn person(person: &Person) -> String {
    match &person.name {
        Some(name) => name.clone(),
        None => short_key(&person.key.to_string()),
    }
}

/// The start of a key or bundle ID, enough to tell it apart at a glance.
pub fn short_key(hex: &str) -> String {
    format!("{}…", format::short_id(hex))
}

/// A category's name, from its slug.
pub fn category_name(categories: &[Category], slug: &str) -> String {
    categories
        .iter()
        .find(|c| c.slug == slug)
        .map_or_else(|| slug.to_owned(), |c| c.name.clone())
}

/// How a sort is named in the picker.
pub fn sort_label(sort: Sort) -> &'static str {
    match sort {
        Sort::Relevance => "Best match",
        Sort::Newest => "Newest",
        Sort::Availability => "Most available",
        Sort::Votes => "Top voted",
        Sort::Trending => "Trending",
    }
}

/// A swarm's health from 0 (nobody) to 4 (many complete copies), for the health bars.
pub fn health_level(swarm: &BundleStats) -> u8 {
    match (swarm.complete, swarm.incomplete) {
        (0, 0) => 0,
        (0, _) => 1,
        (1..=2, _) => 2,
        (3..=9, _) => 3,
        _ => 4,
    }
}

/// A swarm's health in plain words, with its accent.
pub fn health(swarm: &BundleStats) -> (&'static str, Tone) {
    match health_level(swarm) {
        0 => ("No one is sharing this right now", Tone::Dim),
        1 => ("No complete copy online right now", Tone::Warn),
        2 => ("Available from a few peers", Tone::Down),
        3 => ("Available from several peers", Tone::Up),
        _ => ("Widely available", Tone::Up),
    }
}

/// Net votes, signed: `+12`, `0`, `−3`.
pub fn score(votes: VoteCounts) -> String {
    let net = votes.up as i64 - votes.down as i64;
    match net {
        0 => "0".to_owned(),
        n if n > 0 => format!("+{n}"),
        n => format!("−{}", n.unsigned_abs()),
    }
}

/// How long ago `then` was, at `now` (both in seconds since the Unix epoch): `just now`,
/// `5 minutes ago`, `3 days ago`.
pub fn ago(now: u64, then: u64) -> String {
    const UNITS: [(u64, &str); 6] = [
        (365 * 86_400, "year"),
        (30 * 86_400, "month"),
        (7 * 86_400, "week"),
        (86_400, "day"),
        (3600, "hour"),
        (60, "minute"),
    ];
    let secs = now.saturating_sub(then);
    UNITS.iter().find(|(size, _)| secs >= *size).map_or_else(
        || "just now".to_owned(),
        |(size, unit)| format!("{} ago", plural(secs / size, unit)),
    )
}

/// The first paragraph of `description`, cut to about `max` characters at a word.
pub fn excerpt(description: &str, max: usize) -> String {
    let first = description.split("\n\n").next().unwrap_or("").trim();
    let flat: String = first.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    let cut: String = flat.chars().take(max).collect();
    let cut = cut.rsplit_once(' ').map_or(cut.as_str(), |(head, _)| head);
    format!("{}…", cut.trim_end_matches([',', '.', ';', ':']))
}

/// `description`'s paragraphs, split at blank lines.
pub fn paragraphs(description: &str) -> Vec<String> {
    description
        .split("\n\n")
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(str::to_owned)
        .collect()
}

/// `text` cut into runs, each marked if it matches a word of the search `q`, ignoring case.
/// Words under two characters aren't marked. Fuzzy matches aren't either: the marks show only
/// where the search's own letters are.
pub fn highlight(text: &str, q: &str) -> Vec<(String, bool)> {
    let fold = |c: char| c.to_lowercase().next().unwrap_or(c);
    let chars: Vec<char> = text.chars().collect();
    let folded: Vec<char> = chars.iter().map(|&c| fold(c)).collect();
    let words: Vec<Vec<char>> = q
        .split_whitespace()
        .map(|w| w.chars().map(fold).collect::<Vec<_>>())
        .filter(|w| w.len() >= 2)
        .collect();
    let mut marked = vec![false; chars.len()];
    for word in &words {
        for start in 0..chars.len().saturating_sub(word.len() - 1) {
            if folded[start..start + word.len()] == word[..] {
                marked[start..start + word.len()].fill(true);
            }
        }
    }
    let mut runs: Vec<(String, bool)> = Vec::new();
    for (c, mark) in chars.into_iter().zip(marked) {
        match runs.last_mut() {
            Some((run, m)) if *m == mark => run.push(c),
            _ => runs.push((c.to_string(), mark)),
        }
    }
    runs
}

/// `first–last of total` for a page of `shown` results from `offset`.
pub fn range(offset: u64, shown: usize, total: u64) -> String {
    if shown == 0 {
        return plural(total, "bundle");
    }
    let first = offset + 1;
    let last = offset + shown as u64;
    format!(
        "{}–{} of {}",
        format::count(first),
        format::count(last),
        plural(total, "bundle")
    )
}

/// A button in the pager: a page number (from 0), or a gap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageButton {
    /// The page with this index.
    Page(u64),
    /// Pages left out.
    Gap,
}

/// The pager's buttons: the first and last pages, and those next to the current one.
pub fn pages(offset: u64, limit: u32, total: u64) -> Vec<PageButton> {
    let limit = u64::from(limit.max(1));
    let count = total.div_ceil(limit);
    if count <= 1 {
        return Vec::new();
    }
    let current = (offset / limit).min(count - 1);
    let mut buttons = Vec::new();
    let mut last = None;
    for page in 0..count {
        let near = page.abs_diff(current) <= 1;
        let end = page == 0 || page == count - 1;
        // A gap of exactly one page shows that page instead.
        let fills_gap = page.abs_diff(current) == 2 && (page == 1 || page == count - 2);
        if near || end || fills_gap {
            if last.is_some_and(|l: u64| page > l + 1) {
                buttons.push(PageButton::Gap);
            }
            buttons.push(PageButton::Page(page));
            last = Some(page);
        }
    }
    buttons
}

/// The command that signs a browser's sign-in code with the user's Tempest key.
pub fn sign_in_command(origin: &str, code: &str, invite: &str) -> String {
    let invite = invite.trim();
    match invite.is_empty() {
        true => format!("tempest sign-in {origin} --code {code}"),
        false => format!("tempest sign-in {origin} --code {code} --invite {invite}"),
    }
}

/// The `tempest:` link for a listed bundle, naming this tracker so peers can be found.
pub fn link(bundle: BundleId, tracker: &str, name: Option<&str>) -> String {
    let mut link = Link::new(bundle);
    link.trackers.push(tracker.to_owned());
    link.name = name.filter(|n| !n.is_empty()).map(str::to_owned);
    link.to_string()
}

/// The command that downloads `link`, quoted for a shell.
pub fn get_command(link: &str) -> String {
    format!("tempest get '{}'", link.replace('\'', "'\\''"))
}

/// Tags as typed in the form: separated by commas, each normalized, without repeats.
pub fn parse_tags(typed: &str) -> Result<Vec<String>, TextError> {
    let tags: Vec<String> = typed
        .split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_owned)
        .collect();
    text::normalize_tags(&tags)
}

/// What the publisher's form holds, as typed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EditForm {
    /// The title.
    pub title: String,
    /// The description.
    pub description: String,
    /// The category's slug, or empty for none.
    pub category: String,
    /// The tags, separated by commas.
    pub tags: String,
}

/// Which field of the form a problem is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    /// The title.
    Title,
    /// The description.
    Description,
    /// The tags.
    Tags,
}

impl EditForm {
    /// The edit the form asks for, every field set, or the first problem and its field.
    pub fn edit(&self) -> Result<ListingEdit, (Field, String)> {
        let title = text::clean_title(&self.title).map_err(|e| (Field::Title, sentence(e)))?;
        let description = text::clean_description(&self.description)
            .map_err(|e| (Field::Description, sentence(e)))?;
        let tags = parse_tags(&self.tags).map_err(|e| (Field::Tags, sentence(e)))?;
        let category = Some(self.category.clone()).filter(|c| !c.is_empty());
        Ok(ListingEdit {
            title: Some(title),
            description: Some(description),
            category: Some(category),
            tags: Some(tags),
        })
    }
}

fn sentence(error: TextError) -> String {
    crate::request::sentence(&error.0)
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn health_reads_in_plain_words() {
        let stats = |complete, incomplete| BundleStats {
            complete,
            incomplete,
        };
        assert_eq!(health_level(&stats(0, 0)), 0);
        assert_eq!(health(&stats(0, 4)).1, Tone::Warn);
        assert_eq!(health_level(&stats(1, 0)), 2);
        assert_eq!(health_level(&stats(5, 1)), 3);
        assert_eq!(health_level(&stats(40, 9)), 4);
        assert_eq!(health(&stats(0, 0)).0, "No one is sharing this right now");
    }

    #[test]
    fn scores_ages_and_plurals() {
        assert_eq!(score(VoteCounts { up: 5, down: 2 }), "+3");
        assert_eq!(score(VoteCounts { up: 1, down: 4 }), "−3");
        assert_eq!(score(VoteCounts::default()), "0");
        assert_eq!(ago(1000, 990), "just now");
        assert_eq!(ago(1000, 2000), "just now");
        assert_eq!(ago(10_000, 10_000 - 120), "2 minutes ago");
        assert_eq!(ago(10_000_000, 10_000_000 - 86_400), "1 day ago");
        assert_eq!(ago(100_000_000, 100_000_000 - 400 * 86_400), "1 year ago");
        assert_eq!(plural(1_200, "file"), "1,200 files");
    }

    #[test]
    fn excerpts_take_the_first_paragraph_and_cut_at_a_word() {
        assert_eq!(excerpt("One  two\nthree.\n\nSecond.", 80), "One two three.");
        assert_eq!(
            excerpt("Daily rainfall, by month and station", 18),
            "Daily rainfall…"
        );
        assert_eq!(paragraphs("a\n\n\n\nb\n"), ["a", "b"]);
    }

    #[test]
    fn highlights_mark_the_searchs_words() {
        let runs = highlight("Rainfall CSV, rain", "RAIN csv x");
        assert_eq!(
            runs,
            [
                ("Rain".to_owned(), true),
                ("fall ".to_owned(), false),
                ("CSV".to_owned(), true),
                (", ".to_owned(), false),
                ("rain".to_owned(), true),
            ]
        );
        assert_eq!(highlight("abc", ""), [("abc".to_owned(), false)]);
        assert!(highlight("", "abc").is_empty());
    }

    #[test]
    fn pagers_show_the_ends_and_the_neighbors() {
        use PageButton::{Gap, Page};
        assert!(pages(0, 25, 25).is_empty());
        assert_eq!(pages(0, 25, 26), [Page(0), Page(1)]);
        assert_eq!(pages(0, 10, 100), [Page(0), Page(1), Gap, Page(9)]);
        assert_eq!(
            pages(50, 10, 100),
            [Page(0), Gap, Page(4), Page(5), Page(6), Gap, Page(9)]
        );
        assert_eq!(
            pages(20, 10, 100),
            [Page(0), Page(1), Page(2), Page(3), Gap, Page(9)]
        );
        assert_eq!(range(25, 25, 60), "26–50 of 60 bundles");
        assert_eq!(range(0, 0, 0), "0 bundles");
    }

    #[test]
    fn commands_and_links() {
        assert_eq!(
            sign_in_command("http://127.0.0.1:4777", "K7QM2-X4PDV", " "),
            "tempest sign-in http://127.0.0.1:4777 --code K7QM2-X4PDV"
        );
        assert!(sign_in_command("http://t", "C", "INV").ends_with("--invite INV"));
        let bundle: BundleId = "3f9a000000000000000000000000000000000000000000000000000000000000"
            .parse()
            .unwrap();
        let link = link(bundle, "https://t.example.org", Some("Rain 2025"));
        assert_eq!(
            link,
            "tempest:?bundle=3f9a000000000000000000000000000000000000000000000000000000000000\
             &tracker=https://t.example.org&name=Rain%202025"
        );
        assert_eq!(get_command("a'b"), r"tempest get 'a'\''b'");
    }

    #[test]
    fn the_form_applies_the_trackers_rules() {
        let form = EditForm {
            title: "  Rainfall ".into(),
            description: "Daily.\r\n".into(),
            category: String::new(),
            tags: "Weather, CSV,, weather".into(),
        };
        let edit = form.edit().unwrap();
        assert_eq!(edit.title.as_deref(), Some("Rainfall"));
        assert_eq!(edit.description.as_deref(), Some("Daily."));
        assert_eq!(edit.category, Some(None));
        assert_eq!(edit.tags, Some(vec!["weather".into(), "csv".into()]));
        let bad = EditForm {
            tags: "c++".into(),
            ..form
        };
        assert_eq!(bad.edit().unwrap_err().0, Field::Tags);
    }

    proptest! {
        #[test]
        fn highlights_keep_the_text(text in "\\PC{0,40}", q in "\\PC{0,12}") {
            let runs = highlight(&text, &q);
            let joined: String = runs.iter().map(|(run, _)| run.as_str()).collect();
            prop_assert_eq!(joined, text);
            prop_assert!(runs.windows(2).all(|w| w[0].1 != w[1].1));
        }

        #[test]
        fn pagers_hold_the_current_page(offset in 0u64..10_000, limit in 1u32..100, total in 0u64..10_000) {
            let buttons = pages(offset, limit, total);
            let count = total.div_ceil(u64::from(limit));
            if count > 1 {
                let current = (offset / u64::from(limit)).min(count - 1);
                prop_assert!(buttons.contains(&PageButton::Page(current)));
                prop_assert_eq!(buttons.first(), Some(&PageButton::Page(0)));
                prop_assert_eq!(buttons.last(), Some(&PageButton::Page(count - 1)));
                prop_assert!(buttons.len() <= 9);
            } else {
                prop_assert!(buttons.is_empty());
            }
        }
    }
}

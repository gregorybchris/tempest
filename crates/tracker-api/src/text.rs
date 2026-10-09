//! The rules for the text people write in listings. The tracker applies them to what it stores;
//! clients can apply them first to explain a problem sooner.
//!
//! Each rule trims surrounding whitespace, refuses control characters other than those it names,
//! and counts length in characters.

use std::fmt;

use unicode_normalization::UnicodeNormalization;

/// The longest title.
pub const MAX_TITLE: usize = 120;
/// The longest description.
pub const MAX_DESCRIPTION: usize = 10_000;
/// The longest comment.
pub const MAX_COMMENT: usize = 4_000;

/// The longest report.
pub const MAX_REPORT: usize = 1_000;
/// The longest account name.
pub const MAX_NAME: usize = 40;
/// The most tags on a listing.
pub const MAX_TAGS: usize = 10;
/// The longest tag.
pub const MAX_TAG: usize = 32;

/// Why text was refused: a sentence that names the field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextError(pub String);

impl fmt::Display for TextError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for TextError {}

/// A title: one line, possibly empty.
pub fn clean_title(title: &str) -> Result<String, TextError> {
    line(title, "the title", MAX_TITLE)
}

/// An account name: one line, not empty.
pub fn clean_name(name: &str) -> Result<String, TextError> {
    let name = line(name, "the name", MAX_NAME)?;
    if let Some(c) = name.chars().find(|c| is_invisible(*c)) {
        return Err(TextError(format!(
            "the name has the invisible character {c:?}, which could hide who is who"
        )));
    }
    match name.is_empty() {
        true => Err(TextError("the name is empty".into())),
        false => Ok(name),
    }
}

/// Whether `c` is a format character that shows nothing or reorders text around it: zero-width
/// spaces and joiners, direction marks and overrides, isolates, the byte order mark and the
/// like. A name with one could look like another.
fn is_invisible(c: char) -> bool {
    matches!(
        c,
        '\u{00ad}'
            | '\u{034f}'
            | '\u{061c}'
            | '\u{115f}'..='\u{1160}'
            | '\u{17b4}'..='\u{17b5}'
            | '\u{180b}'..='\u{180f}'
            | '\u{200b}'..='\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2060}'..='\u{206f}'
            | '\u{3164}'
            | '\u{fe00}'..='\u{fe0f}'
            | '\u{feff}'
            | '\u{ffa0}'
            | '\u{fff9}'..='\u{fffb}'
            | '\u{e0000}'..='\u{e0fff}'
    )
}

/// What two account names must differ in to be different names (D-261): the name with accents,
/// case, spacing and punctuation removed, and letters that look alike made the same. `None` if
/// nothing is left, as in a name of only symbols.
///
/// ```
/// use tempest_tracker_api::text::name_key;
/// assert_eq!(name_key("CGreg"), name_key("c_greg"));
/// assert_eq!(name_key("cgreg"), name_key("cgrég"));
/// assert_eq!(name_key("paul"), name_key("pauI"));
/// assert_eq!(name_key("★"), None);
/// ```
pub fn name_key(name: &str) -> Option<String> {
    let folded: String = name
        .nfkd()
        .flat_map(char::to_lowercase)
        // Accents split off by NFKD, spaces and punctuation are not alphanumeric.
        .filter(|c| c.is_alphanumeric())
        .map(look_alike)
        .collect();
    // Two letters can pass for one.
    let folded = folded.replace("rn", "m").replace("vv", "w");
    (!folded.is_empty()).then_some(folded)
}

/// The letter that `c` can pass for, or `c`: Cyrillic and Greek letters like Latin ones, and
/// `l`, `i`, `I`, `1` and `0` like each other and `o`.
fn look_alike(c: char) -> char {
    match c {
        'i' | '1' | 'ı' | 'і' | 'ι' | 'ӏ' => 'l',
        '0' | 'ο' | 'о' | 'օ' => 'o',
        'а' | 'α' => 'a',
        'е' | 'ε' => 'e',
        'р' | 'ρ' => 'p',
        'с' | 'ϲ' => 'c',
        'у' | 'γ' => 'y',
        'х' | 'χ' => 'x',
        'ѕ' => 's',
        'ј' => 'j',
        'һ' => 'h',
        'ԁ' => 'd',
        'ԛ' => 'q',
        'ԝ' | 'ѡ' => 'w',
        'ν' => 'v',
        'κ' | 'к' => 'k',
        'τ' | 'т' => 't',
        'υ' => 'u',
        'м' => 'm',
        'н' => 'h',
        'в' => 'b',
        _ => c,
    }
}

/// A description: lines and tabs allowed, possibly empty. Line endings become `\n`.
pub fn clean_description(description: &str) -> Result<String, TextError> {
    paragraphs(description, "the description", MAX_DESCRIPTION)
}

/// A comment: lines and tabs allowed, not empty. Line endings become `\n`.
pub fn clean_comment(body: &str) -> Result<String, TextError> {
    let body = paragraphs(body, "the comment", MAX_COMMENT)?;
    match body.is_empty() {
        true => Err(TextError("the comment is empty".into())),
        false => Ok(body),
    }
}

/// A report's reason: lines and tabs allowed, not empty. Line endings become `\n`.
pub fn clean_report(reason: &str) -> Result<String, TextError> {
    let reason = paragraphs(reason, "the reason", MAX_REPORT)?;
    match reason.is_empty() {
        true => Err(TextError("the reason is empty".into())),
        false => Ok(reason),
    }
}

/// A tag in its one form: lowercase letters and digits in words joined by `-`. Spaces and `_`
/// join words too, so `Machine Learning` becomes `machine-learning`.
pub fn normalize_tag(tag: &str) -> Result<String, TextError> {
    let mut normal = String::new();
    for c in tag.trim().chars().flat_map(char::to_lowercase) {
        if c.is_alphanumeric() {
            normal.push(c);
        } else if matches!(c, '-' | '_' | ' ') {
            if !normal.is_empty() && !normal.ends_with('-') {
                normal.push('-');
            }
        } else {
            return Err(TextError(format!(
                "the tag {tag:?} has {c:?}: use letters, digits and -"
            )));
        }
    }
    let normal = normal.trim_end_matches('-').to_owned();
    if normal.is_empty() {
        return Err(TextError("a tag is empty".into()));
    }
    if normal.chars().count() > MAX_TAG {
        return Err(TextError(format!(
            "the tag {normal:?} is longer than {MAX_TAG} characters"
        )));
    }
    Ok(normal)
}

/// Tags, each normalized, without repeats, in their first order.
pub fn normalize_tags(tags: &[String]) -> Result<Vec<String>, TextError> {
    let mut normal = Vec::new();
    for tag in tags {
        let tag = normalize_tag(tag)?;
        if !normal.contains(&tag) {
            normal.push(tag);
        }
    }
    match normal.len() > MAX_TAGS {
        true => Err(TextError(format!("a listing has at most {MAX_TAGS} tags"))),
        false => Ok(normal),
    }
}

fn line(text: &str, what: &str, max: usize) -> Result<String, TextError> {
    let text = text.trim();
    if text.chars().any(char::is_control) {
        return Err(TextError(format!("{what} must be one line of text")));
    }
    within(text.to_owned(), what, max)
}

fn paragraphs(text: &str, what: &str, max: usize) -> Result<String, TextError> {
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let text = text.trim();
    if text
        .chars()
        .any(|c| c.is_control() && c != '\n' && c != '\t')
    {
        return Err(TextError(format!("{what} has control characters")));
    }
    within(text.to_owned(), what, max)
}

fn within(text: String, what: &str, max: usize) -> Result<String, TextError> {
    match text.chars().count() > max {
        true => Err(TextError(format!("{what} is longer than {max} characters"))),
        false => Ok(text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn names_refuse_invisible_and_direction_characters() {
        for name in [
            "a\u{200b}b",
            "evil\u{202e}gnp",
            "x\u{feff}",
            "\u{2066}y\u{2069}",
        ] {
            assert!(clean_name(name).is_err(), "{name:?}");
        }
        assert_eq!(clean_name("Zoë 🙂").unwrap(), "Zoë 🙂");
    }

    #[test]
    fn names_that_look_alike_share_a_key() {
        let key = name_key("cgreg").unwrap();
        for twin in [
            "CGreg",
            "c greg",
            "c_g.r.e.g",
            "cgrég",
            "сgrеg",
            "cgreg\u{200b}",
        ] {
            assert_eq!(name_key(twin).as_deref(), Some(key.as_str()), "{twin}");
        }
        assert_eq!(name_key("rnoon"), name_key("moon"));
        assert_eq!(name_key("Alice"), name_key("AIice"));
        assert_eq!(name_key("A1ice"), name_key("Alice"));
        assert_ne!(name_key("alice"), name_key("alicia"));
        assert_eq!(name_key("   "), None);
        assert_eq!(name_key("---"), None);
    }

    proptest! {
        #[test]
        fn a_name_key_is_stable(name in "\\PC{1,40}") {
            if let Some(key) = name_key(&name) {
                prop_assert_eq!(name_key(&key), Some(key));
            }
        }
    }

    #[test]
    fn tags_are_lowercase_words_joined_by_dashes() {
        assert_eq!(
            normalize_tag(" Machine  Learning ").unwrap(),
            "machine-learning"
        );
        assert_eq!(normalize_tag("open_source--").unwrap(), "open-source");
        assert_eq!(normalize_tag("Ünïcode").unwrap(), "ünïcode");
        assert!(normalize_tag("c++").is_err());
        assert!(normalize_tag(" - ").is_err());
        assert!(normalize_tag(&"x".repeat(MAX_TAG + 1)).is_err());
    }

    #[test]
    fn tags_lose_repeats_and_are_limited() {
        let tags = ["ML", "ml", "Data"].map(String::from);
        assert_eq!(normalize_tags(&tags).unwrap(), ["ml", "data"]);
        let many: Vec<String> = (0..=MAX_TAGS).map(|i| format!("t{i}")).collect();
        assert!(normalize_tags(&many).is_err());
    }

    #[test]
    fn lines_and_paragraphs() {
        assert_eq!(clean_title("  A title ").unwrap(), "A title");
        assert!(clean_title("two\nlines").is_err());
        assert!(clean_title(&"é".repeat(MAX_TITLE + 1)).is_err());
        assert_eq!(
            clean_title(&"é".repeat(MAX_TITLE)).unwrap().len(),
            2 * MAX_TITLE
        );
        assert_eq!(clean_description("a\r\nb\rc\t").unwrap(), "a\nb\nc");
        assert!(clean_description("bell\u{7}").is_err());
        assert!(clean_comment("  \n ").is_err());
        assert!(clean_report(" \n").is_err());
        assert_eq!(clean_report(" spam\r\n").unwrap(), "spam");
        assert!(clean_report(&"x".repeat(MAX_REPORT + 1)).is_err());
        assert!(clean_name("").is_err());
    }

    proptest! {
        #[test]
        fn normal_tags_are_fixed_points(tag in "\\PC{0,40}") {
            if let Ok(normal) = normalize_tag(&tag) {
                prop_assert_eq!(normalize_tag(&normal), Ok(normal.clone()));
                prop_assert!(!normal.starts_with('-') && !normal.ends_with('-'));
                prop_assert!(!normal.contains("--"));
                prop_assert!(normal.chars().all(|c| c == '-' || c.is_alphanumeric()));
            }
        }

        #[test]
        fn clean_text_obeys_its_rules(text in "\\PC{0,200}|[\\s\\S]{0,50}") {
            if let Ok(clean) = clean_description(&text) {
                prop_assert_eq!(clean.trim(), clean.as_str());
                prop_assert!(!clean.contains('\r'));
                prop_assert_eq!(clean_description(&clean), Ok(clean.clone()));
            }
            if let Ok(clean) = clean_title(&text) {
                prop_assert!(!clean.chars().any(char::is_control));
                prop_assert!(clean.chars().count() <= MAX_TITLE);
            }
        }
    }
}

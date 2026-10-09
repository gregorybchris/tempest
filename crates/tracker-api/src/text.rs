//! The rules for the text people write in listings. The tracker applies them to what it stores;
//! clients can apply them first to explain a problem sooner.
//!
//! Each rule trims surrounding whitespace, refuses control characters other than those it names,
//! and counts length in characters.

use std::fmt;

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
    match name.is_empty() {
        true => Err(TextError("the name is empty".into())),
        false => Ok(name),
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

/// A category in its one form, which is a tag's: `Machine Learning` becomes `machine-learning`.
pub fn normalize_category(category: &str) -> Result<String, TextError> {
    normalize_tag(category).map_err(|TextError(e)| TextError(e.replace("tag", "category")))
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

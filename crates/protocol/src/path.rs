//! Manifest paths: the rules that let every valid bundle be written identically on Windows, macOS
//! and Linux (`protocol/content.md`, Paths).

mod case_folding;

use unicode_normalization::is_nfc;

/// The longest path, in bytes.
pub const MAX_PATH_LEN: usize = 4096;

/// The longest path segment, in bytes.
pub const MAX_SEGMENT_LEN: usize = 255;

/// Characters no segment may contain, besides control characters.
const FORBIDDEN_CHARS: &[char] = &['<', '>', ':', '"', '\\', '|', '?', '*'];

/// Names reserved on Windows, in case-folded form. `COM` and `LPT` with a digit are handled
/// separately.
const RESERVED_NAMES: &[&str] = &["con", "prn", "aux", "nul", "conin$", "conout$"];

/// Why a path or symlink target is invalid.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PathError {
    /// The path was not in Unicode Normalization Form C.
    #[error("not in Unicode Normalization Form C")]
    NotNfc,
    /// The path was longer than [`MAX_PATH_LEN`] bytes.
    #[error("{0} bytes long, more than the limit of 4096")]
    TooLong(usize),
    /// The path began with `/`.
    #[error("begins with '/'")]
    LeadingSlash,
    /// The path ended with `/`.
    #[error("ends with '/'")]
    TrailingSlash,
    /// A segment broke a rule.
    #[error("segment {segment:?}: {error}")]
    Segment {
        /// The offending segment.
        segment: String,
        /// The rule it broke.
        error: SegmentError,
    },
}

/// Why a path segment is invalid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SegmentError {
    /// The segment was empty.
    #[error("empty")]
    Empty,
    /// The segment was `.`.
    #[error("is '.'")]
    Dot,
    /// The segment was `..` where that isn't allowed.
    #[error("is '..'")]
    DotDot,
    /// The segment was longer than [`MAX_SEGMENT_LEN`] bytes.
    #[error("{0} bytes long, more than the limit of 255")]
    TooLong(usize),
    /// The segment contained a control character.
    #[error("contains control character {0:?}")]
    ControlChar(char),
    /// The segment contained a character that is invalid in file names on some system.
    #[error("contains {0:?}")]
    ForbiddenChar(char),
    /// The segment ended with `.` or a space.
    #[error("ends with '.' or a space")]
    TrailingDotOrSpace,
    /// The segment was a name reserved on Windows.
    #[error("is a name reserved on Windows")]
    Reserved,
}

/// Folds `s` for case-insensitive comparison, with Unicode simple case folding (the `C` and `S`
/// mappings of `CaseFolding.txt`, Unicode 15.1).
pub fn fold_case(s: &str) -> String {
    s.chars().map(fold_char).collect()
}

fn fold_char(c: char) -> char {
    match case_folding::SIMPLE_CASE_FOLDING.binary_search_by_key(&c, |&(from, _)| from) {
        Ok(i) => case_folding::SIMPLE_CASE_FOLDING[i].1,
        Err(_) => c,
    }
}

/// Whether `segment` is a name reserved on Windows: the part before its first `.`, with trailing
/// spaces removed, is one of `CON`, `PRN`, `AUX`, `NUL`, `CONIN$`, `CONOUT$`, or `COM` or `LPT`
/// followed by a digit or `¹`, `²` or `³`, compared case-insensitively.
pub fn is_reserved_name(segment: &str) -> bool {
    let stem = segment.split('.').next().unwrap_or_default();
    let stem = fold_case(stem.trim_end_matches(' '));
    if RESERVED_NAMES.contains(&stem.as_str()) {
        return true;
    }
    let Some(suffix) = stem.strip_prefix("com").or(stem.strip_prefix("lpt")) else {
        return false;
    };
    let mut chars = suffix.chars();
    matches!(
        (chars.next(), chars.next()),
        (Some('0'..='9' | '¹' | '²' | '³'), None)
    )
}

/// Checks the rules for one path segment. A segment can't contain `/`.
///
/// This doesn't check Normalization Form C, which applies to whole paths.
pub fn validate_segment(segment: &str) -> Result<(), SegmentError> {
    match segment {
        "" => return Err(SegmentError::Empty),
        "." => return Err(SegmentError::Dot),
        ".." => return Err(SegmentError::DotDot),
        _ => {}
    }
    if segment.len() > MAX_SEGMENT_LEN {
        return Err(SegmentError::TooLong(segment.len()));
    }
    for c in segment.chars() {
        if c.is_ascii_control() {
            return Err(SegmentError::ControlChar(c));
        }
        if c == '/' || FORBIDDEN_CHARS.contains(&c) {
            return Err(SegmentError::ForbiddenChar(c));
        }
    }
    if segment.ends_with(['.', ' ']) {
        return Err(SegmentError::TrailingDotOrSpace);
    }
    if is_reserved_name(segment) {
        return Err(SegmentError::Reserved);
    }
    Ok(())
}

/// Checks that `name` is a valid bundle name: a single valid path segment, in NFC.
pub fn validate_name(name: &str) -> Result<(), PathError> {
    if !is_nfc(name) {
        return Err(PathError::NotNfc);
    }
    validate_segment(name).map_err(|error| PathError::Segment {
        segment: name.to_owned(),
        error,
    })
}

/// Checks the rules for an entry's path.
pub fn validate_path(path: &str) -> Result<(), PathError> {
    validate(path, false)
}

/// Checks the rules for a symlink's target: those of a path, except that `..` segments are
/// allowed.
///
/// This doesn't check where the target leads; see [`symlink_steps`].
pub fn validate_symlink_target(target: &str) -> Result<(), PathError> {
    validate(target, true)
}

fn validate(path: &str, allow_dot_dot: bool) -> Result<(), PathError> {
    if !is_nfc(path) {
        return Err(PathError::NotNfc);
    }
    if path.len() > MAX_PATH_LEN {
        return Err(PathError::TooLong(path.len()));
    }
    if path.starts_with('/') {
        return Err(PathError::LeadingSlash);
    }
    if path.ends_with('/') {
        return Err(PathError::TrailingSlash);
    }
    for segment in path.split('/') {
        if allow_dot_dot && segment == ".." {
            continue;
        }
        validate_segment(segment).map_err(|error| PathError::Segment {
            segment: segment.to_owned(),
            error,
        })?;
    }
    Ok(())
}

/// The parent directory of `path`, or `""` for the top level.
pub fn parent(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(parent, _)| parent)
}

/// Every directory that contains `path`, from the outermost: `"a/b/c"` gives `"a"` and `"a/b"`.
pub fn ancestors(path: &str) -> impl Iterator<Item = &str> {
    path.match_indices('/').map(|(i, _)| &path[..i])
}

/// A symlink target that leads out of the bundle's top level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("target leads out of the bundle")]
pub struct EscapesBundle;

/// Resolves the target of the symlink at `link` one segment at a time, starting from the link's
/// own directory, and returns the path reached after each step (`""` is the top level).
///
/// Fails if a step leaves the top level. `target` should already have passed
/// [`validate_symlink_target`].
pub fn symlink_steps(link: &str, target: &str) -> Result<Vec<String>, EscapesBundle> {
    let mut current: Vec<&str> = match parent(link) {
        "" => Vec::new(),
        dir => dir.split('/').collect(),
    };
    let mut steps = Vec::new();
    for segment in target.split('/') {
        if segment == ".." {
            current.pop().ok_or(EscapesBundle)?;
        } else {
            current.push(segment);
        }
        steps.push(current.join("/"));
    }
    Ok(steps)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn case_folding_uses_simple_mappings() {
        assert_eq!(fold_case("Photo.JPG"), "photo.jpg");
        // S mapping: capital sharp s folds to ß, not "ss".
        assert_eq!(fold_case("\u{1E9E}"), "ß");
        assert_eq!(fold_case("ß"), "ß");
        // Kelvin sign, final sigma.
        assert_eq!(fold_case("\u{212A}"), "k");
        assert_eq!(fold_case("Σς"), "σσ");
        // İ has only F and T mappings, so it is unchanged.
        assert_eq!(fold_case("\u{130}"), "\u{130}");
    }

    #[test]
    fn case_folding_table_is_sorted() {
        let table = case_folding::SIMPLE_CASE_FOLDING;
        assert!(table.windows(2).all(|w| w[0].0 < w[1].0));
    }

    #[test]
    fn reserved_names() {
        for name in [
            "con",
            "CON.txt",
            "Con .tar.gz",
            "nul.",
            "aux",
            "PRN",
            "CONIN$",
            "conout$.x",
            "COM0",
            "com9",
            "COM¹",
            "lpt²",
            "LPT³.log",
            "lpt5 ",
        ] {
            assert!(is_reserved_name(name), "{name}");
        }
        for name in [
            "console", "com", "com10", "lpt⁴", "x.con", "conin", "nul-1", "comm1",
        ] {
            assert!(!is_reserved_name(name), "{name}");
        }
    }

    #[test]
    fn segment_rules() {
        use SegmentError::*;
        assert_eq!(validate_segment("ok name.txt"), Ok(()));
        assert_eq!(validate_segment(""), Err(Empty));
        assert_eq!(validate_segment("."), Err(Dot));
        assert_eq!(validate_segment(".."), Err(DotDot));
        assert_eq!(validate_segment(&"a".repeat(255)), Ok(()));
        assert_eq!(validate_segment(&"a".repeat(256)), Err(TooLong(256)));
        assert_eq!(validate_segment("a\u{7f}"), Err(ControlChar('\u{7f}')));
        assert_eq!(validate_segment("a\tb"), Err(ControlChar('\t')));
        for c in FORBIDDEN_CHARS {
            assert_eq!(validate_segment(&format!("a{c}b")), Err(ForbiddenChar(*c)));
        }
        assert_eq!(validate_segment("a."), Err(TrailingDotOrSpace));
        assert_eq!(validate_segment("a "), Err(TrailingDotOrSpace));
        assert_eq!(validate_segment("..."), Err(TrailingDotOrSpace));
        assert_eq!(validate_segment(".hidden"), Ok(()));
        assert_eq!(validate_segment("Aux.txt"), Err(Reserved));
    }

    #[test]
    fn path_rules() {
        assert_eq!(validate_path("a/b/c.txt"), Ok(()));
        assert_eq!(validate_path("/a"), Err(PathError::LeadingSlash));
        assert_eq!(validate_path("a/"), Err(PathError::TrailingSlash));
        assert!(matches!(
            validate_path("a//b"),
            Err(PathError::Segment {
                error: SegmentError::Empty,
                ..
            })
        ));
        assert!(matches!(
            validate_path("a/../b"),
            Err(PathError::Segment {
                error: SegmentError::DotDot,
                ..
            })
        ));
        // "e" followed by a combining acute accent is NFD.
        assert_eq!(validate_path("e\u{301}"), Err(PathError::NotNfc));
        assert_eq!(validate_path("\u{e9}"), Ok(()));
        let long = ["a"; 2048].join("/");
        assert_eq!(validate_path(&format!("{long}a")), Ok(()));
        assert_eq!(
            validate_path(&format!("{long}aa")),
            Err(PathError::TooLong(4097))
        );
    }

    #[test]
    fn names_are_single_segments() {
        assert_eq!(validate_name("Holiday Photos"), Ok(()));
        assert!(validate_name("a/b").is_err());
        assert!(validate_name("").is_err());
    }

    #[test]
    fn symlink_targets_may_contain_dot_dot() {
        assert_eq!(validate_symlink_target("../a.txt"), Ok(()));
        assert!(validate_symlink_target("./a.txt").is_err());
        assert_eq!(
            validate_symlink_target("/etc/passwd"),
            Err(PathError::LeadingSlash)
        );
    }

    #[test]
    fn symlink_resolution() {
        assert_eq!(
            symlink_steps("docs/link", "../a.txt"),
            Ok(vec!["".to_owned(), "a.txt".to_owned()])
        );
        assert_eq!(
            symlink_steps("link", "x/../y"),
            Ok(vec!["x".to_owned(), "".to_owned(), "y".to_owned()])
        );
        assert_eq!(symlink_steps("link", "../a"), Err(EscapesBundle));
        // Leaving and re-entering still leaves the top level on the way.
        assert_eq!(symlink_steps("d/link", "../../d/a"), Err(EscapesBundle));
    }

    #[test]
    fn ancestors_and_parent() {
        assert_eq!(ancestors("a/b/c").collect::<Vec<_>>(), ["a", "a/b"]);
        assert_eq!(ancestors("a").count(), 0);
        assert_eq!(parent("a/b/c"), "a/b");
        assert_eq!(parent("a"), "");
    }
}

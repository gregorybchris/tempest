//! Checks on the stylesheets that no compiler makes: every theme defines the same tokens, text
//! keeps 4.5:1 contrast and chart colors 3:1 (WCAG 2.2, 1.4.3 and 1.4.11), the fonts they name
//! exist, and every app links every sheet, in the same order.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use tempest_ui_kit::Theme;

const CSS: &str = include_str!("../../style/themes.css");

/// The kit's sheets.
fn style() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("style")
}

/// Colors that text is drawn in.
const TEXT: [&str; 7] = ["fg", "dim", "down", "up", "hot", "warn", "bad"];
/// Backgrounds that text sits on.
const BACKGROUNDS: [&str; 2] = ["bg", "raise"];

/// The custom properties set in the rule for `[data-theme="name"]`.
fn tokens(theme: Theme) -> BTreeMap<String, String> {
    let selector = format!("[data-theme=\"{}\"] {{", theme.name());
    let start = CSS
        .find(&selector)
        .unwrap_or_else(|| panic!("no rule for {}", theme.name()));
    let body = &CSS[start + selector.len()..];
    let body = &body[..body.find('}').expect("an unclosed rule")];
    body.lines()
        .filter_map(|line| {
            line.trim()
                .strip_prefix("--")?
                .strip_suffix(';')?
                .split_once(':')
        })
        .map(|(name, value)| (name.trim().to_owned(), value.trim().to_owned()))
        .collect()
}

/// A `#rrggbb` color's relative luminance.
fn luminance(hex: &str) -> f64 {
    let hex = hex
        .strip_prefix('#')
        .unwrap_or_else(|| panic!("{hex} isn't a hex color"));
    assert_eq!(hex.len(), 6, "{hex} isn't #rrggbb");
    let channel = |i: usize| {
        let c = f64::from(u8::from_str_radix(&hex[i..i + 2], 16).unwrap()) / 255.0;
        if c <= 0.040_45 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(0) + 0.7152 * channel(2) + 0.0722 * channel(4)
}

fn contrast(a: &str, b: &str) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

#[test]
fn every_theme_defines_the_same_tokens() {
    let names = |theme| tokens(theme).into_keys().collect::<Vec<_>>();
    let expected = names(Theme::Green);
    assert!(expected.len() > 20, "{expected:?}");
    for theme in Theme::ALL {
        assert_eq!(names(theme), expected, "{}", theme.name());
    }
}

#[test]
fn text_and_series_colors_keep_their_contrast() {
    let mut failures = Vec::new();
    for theme in Theme::ALL {
        let tokens = tokens(theme);
        let check = |fg: &str, bg: &str, min: f64, failures: &mut Vec<String>| {
            let ratio = contrast(&tokens[fg], &tokens[bg]);
            if ratio < min {
                failures.push(format!(
                    "{}: --{fg} on --{bg} is {ratio:.2}:1, under {min}:1",
                    theme.name()
                ));
            }
        };
        for fg in TEXT {
            for bg in BACKGROUNDS {
                check(fg, bg, 4.5, &mut failures);
            }
        }
        for i in 1..=6 {
            check(&format!("series-{i}"), "bg", 3.0, &mut failures);
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn the_fonts_exist() {
    let css = fs::read_to_string(style().join("fonts.css")).unwrap();
    let urls: Vec<_> = css
        .split("url(\"")
        .skip(1)
        .filter_map(|rest| rest.split_once('"'))
        .map(|(url, _)| url)
        .collect();
    assert!(!urls.is_empty());
    for url in urls {
        assert!(style().join(url).is_file(), "{url} is missing");
    }
}

/// The kit's sheets an app's page links, in order.
fn linked(page: &str) -> Vec<String> {
    page.split("href=\"../ui-kit/style/")
        .skip(1)
        .filter_map(|rest| rest.split_once('"'))
        .map(|(sheet, _)| sheet.to_owned())
        .filter(|sheet| sheet.ends_with(".css"))
        .collect()
}

#[test]
fn every_app_links_every_sheet_in_the_same_order() {
    let sheets: BTreeSet<String> = fs::read_dir(style())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter(|name| name.ends_with(".css"))
        .collect();
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let mut orders = BTreeMap::new();
    for entry in fs::read_dir(crates).unwrap() {
        let page = entry.unwrap().path().join("index.html");
        let Ok(text) = fs::read_to_string(&page) else {
            continue;
        };
        let order = linked(&text);
        if !order.is_empty() {
            orders.insert(page.display().to_string(), order);
        }
    }
    assert!(orders.len() >= 4, "{orders:?}");
    let (first, order) = orders.iter().next().unwrap();
    assert_eq!(
        order.iter().cloned().collect::<BTreeSet<_>>(),
        sheets,
        "{first} doesn't link every sheet"
    );
    // The themes' tokens come first and the effects, over everything, last.
    assert_eq!(order.first().map(String::as_str), Some("themes.css"));
    assert_eq!(order.last().map(String::as_str), Some("effects.css"));
    for (page, other) in &orders {
        assert_eq!(other, order, "{page} and {first} differ");
    }
}

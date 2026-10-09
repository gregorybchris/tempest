//! Paths on the engine's machine, as text: the picker's breadcrumbs, parents and joins. The
//! engine may run on another system than the browser, so a path is read by its shape: `/…` is
//! a Unix path, and `C:\…` or `\\server\share\…` a Windows one, which may also use `/`.

/// One step of a path, for a breadcrumb.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Crumb {
    /// What it shows: a folder's name, or the root.
    pub label: String,
    /// The path up to and including it.
    pub path: String,
}

/// An absolute path taken apart.
struct Parsed<'a> {
    /// The root as a path, ending with its separator: `/`, `C:\`, `\\server\share\`.
    root: String,
    /// The root as a breadcrumb shows it.
    label: String,
    /// The names after the root.
    names: Vec<&'a str>,
    /// The separator that joins them.
    separator: char,
}

/// The names in `rest`, split at either separator.
fn split(rest: &str) -> Vec<&str> {
    rest.split(['/', '\\']).filter(|n| !n.is_empty()).collect()
}

fn parse(path: &str) -> Option<Parsed<'_>> {
    let bytes = path.as_bytes();
    if let Some(rest) = path.strip_prefix("\\\\") {
        let mut names = split(rest);
        if names.is_empty() {
            return None;
        }
        let share: Vec<&str> = names.drain(..names.len().min(2)).collect();
        let label = format!("\\\\{}", share.join("\\"));
        return Some(Parsed {
            root: format!("{label}\\"),
            label,
            names,
            separator: '\\',
        });
    }
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        if !bytes.get(2).is_some_and(|b| matches!(b, b'\\' | b'/')) {
            return None;
        }
        let drive = path[..2].to_ascii_uppercase();
        return Some(Parsed {
            root: format!("{drive}\\"),
            label: format!("{drive}\\"),
            names: split(&path[2..]),
            separator: '\\',
        });
    }
    let rest = path.strip_prefix('/')?;
    Some(Parsed {
        root: "/".to_owned(),
        label: "/".to_owned(),
        names: rest.split('/').filter(|n| !n.is_empty()).collect(),
        separator: '/',
    })
}

impl Parsed<'_> {
    /// The path of the root and the first `count` names.
    fn path(&self, count: usize) -> String {
        let mut path = self.root.clone();
        let separator = self.separator.to_string();
        path.push_str(&self.names[..count].join(&separator));
        path
    }
}

/// The breadcrumbs of `path`, from its root; none for a relative path.
pub fn crumbs(path: &str) -> Vec<Crumb> {
    let Some(parsed) = parse(path) else {
        return Vec::new();
    };
    let mut crumbs = vec![Crumb {
        label: parsed.label.clone(),
        path: parsed.root.clone(),
    }];
    crumbs.extend(parsed.names.iter().enumerate().map(|(i, name)| Crumb {
        label: (*name).to_owned(),
        path: parsed.path(i + 1),
    }));
    crumbs
}

/// The folder `path` is in; none for a root or a relative path.
pub fn parent(path: &str) -> Option<String> {
    let parsed = parse(path)?;
    let count = parsed.names.len().checked_sub(1)?;
    Some(parsed.path(count))
}

/// `path`'s last name; none for a root or a relative path.
pub fn name(path: &str) -> Option<&str> {
    parse(path)?.names.last().copied()
}

/// `name` in `folder`, with the folder's separator.
pub fn join(folder: &str, name: &str) -> String {
    let separator = parse(folder).map_or('/', |p| p.separator);
    let mut path = folder.to_owned();
    if !path.ends_with(['/', '\\']) {
        path.push(separator);
    }
    path.push_str(name);
    path
}

/// `path` with a leading `~` standing for `home`.
pub fn expand(path: &str, home: Option<&str>) -> String {
    match (path.strip_prefix('~'), home) {
        (Some(rest), Some(home)) if rest.is_empty() || rest.starts_with(['/', '\\']) => {
            format!("{}{rest}", home.trim_end_matches(['/', '\\']))
        }
        _ => path.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(path: &str) -> Vec<(String, String)> {
        crumbs(path)
            .into_iter()
            .map(|c| (c.label, c.path))
            .collect()
    }

    fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter()
            .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
            .collect()
    }

    #[test]
    fn unix_paths_break_into_their_folders() {
        assert_eq!(
            labels("/Users/chris/Movies"),
            pairs(&[
                ("/", "/"),
                ("Users", "/Users"),
                ("chris", "/Users/chris"),
                ("Movies", "/Users/chris/Movies"),
            ])
        );
        assert_eq!(labels("/"), pairs(&[("/", "/")]));
        // Doubled and trailing slashes are just separators.
        assert_eq!(
            labels("//srv//data/"),
            pairs(&[("/", "/"), ("srv", "/srv"), ("data", "/srv/data")])
        );
        assert!(labels("relative/path").is_empty());
    }

    #[test]
    fn windows_paths_break_into_their_folders() {
        assert_eq!(
            labels(r"c:\Users\Chris Gregory"),
            pairs(&[
                (r"C:\", r"C:\"),
                ("Users", r"C:\Users"),
                ("Chris Gregory", r"C:\Users\Chris Gregory"),
            ])
        );
        assert_eq!(
            labels("D:/media/films"),
            pairs(&[
                (r"D:\", r"D:\"),
                ("media", r"D:\media"),
                ("films", r"D:\media\films")
            ])
        );
        assert_eq!(labels(r"E:\"), pairs(&[(r"E:\", r"E:\")]));
        assert_eq!(
            labels(r"\\nas\share\photos"),
            pairs(&[
                (r"\\nas\share", r"\\nas\share\"),
                ("photos", r"\\nas\share\photos"),
            ])
        );
        // A drive without a separator is relative to that drive's current folder.
        assert!(labels("C:Users").is_empty());
    }

    #[test]
    fn parents_names_and_joins_keep_the_paths_system() {
        assert_eq!(parent("/a/b").as_deref(), Some("/a"));
        assert_eq!(parent("/a").as_deref(), Some("/"));
        assert_eq!(parent("/"), None);
        assert_eq!(parent(r"C:\a\b").as_deref(), Some(r"C:\a"));
        assert_eq!(parent(r"C:\a").as_deref(), Some(r"C:\"));
        assert_eq!(parent(r"C:\"), None);
        assert_eq!(parent("x/y"), None);
        assert_eq!(name("/a/b"), Some("b"));
        assert_eq!(name(r"C:\a\Films"), Some("Films"));
        assert_eq!(name("/"), None);
        assert_eq!(join("/a", "b"), "/a/b");
        assert_eq!(join("/", "b"), "/b");
        assert_eq!(join(r"C:\a", "b"), r"C:\a\b");
        assert_eq!(join(r"C:\", "b"), r"C:\b");
    }

    #[test]
    fn only_absolute_paths_have_crumbs_and_home_expands() {
        for path in ["/", "/a", r"C:\", "c:/x", r"\\nas\share"] {
            assert!(!crumbs(path).is_empty(), "{path}");
        }
        for path in ["", "a/b", "C:", "C:x", "~/x", r"\\"] {
            assert!(crumbs(path).is_empty(), "{path}");
        }
        assert_eq!(expand("~/Movies", Some("/home/c/")), "/home/c/Movies");
        assert_eq!(expand("~", Some("/home/c")), "/home/c");
        assert_eq!(expand("~other", Some("/home/c")), "~other");
        assert_eq!(expand("~/x", None), "~/x");
        assert_eq!(expand(r"~\x", Some(r"C:\Users\c")), r"C:\Users\c\x");
    }
}

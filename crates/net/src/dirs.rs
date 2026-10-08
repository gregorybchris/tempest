//! Where Tempest keeps its files by default.

use std::path::PathBuf;

/// The current user's home directory, if it is known and absolute.
pub fn home_dir() -> Option<PathBuf> {
    std::env::home_dir().filter(|path| path.is_absolute())
}

/// Tempest's folder in the per-user data directory: the local application data folder on
/// Windows, Application Support on macOS, and the XDG data directory elsewhere (D-175).
pub fn default_data_dir() -> Option<PathBuf> {
    let absolute = |path: PathBuf| Some(path).filter(|path| path.is_absolute());
    let env = |name: &str| std::env::var_os(name).map(PathBuf::from).and_then(absolute);
    let data_dir = if cfg!(windows) {
        env("LOCALAPPDATA")
    } else if cfg!(target_os = "macos") {
        home_dir().map(|home| home.join("Library/Application Support"))
    } else {
        env("XDG_DATA_HOME").or_else(|| home_dir().map(|home| home.join(".local/share")))
    };
    let tempest = if cfg!(any(windows, target_os = "macos")) {
        "Tempest"
    } else {
        "tempest"
    };
    Some(data_dir?.join(tempest))
}

//! Showing a file or folder in the system's file manager, on the machine the engine runs on.
//!
//! The TUI and the web UI both ask the engine to do this (`POST /v1/transfers/{id}/reveal`), so
//! they reveal a download the same way.

use std::path::Path;
use std::process::{Command, Stdio};

/// Shows `path` in the file manager: macOS selects it in its folder, Windows does for a file and
/// opens a folder, and elsewhere a folder is opened (a file's parent).
pub fn reveal(path: &Path) -> Result<(), String> {
    let (program, args) = command(path);
    let status = Command::new(program)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| format!("cannot run {program}: {error}"))?;
    // Explorer exits with 1 even when it shows the path.
    if status.success() || cfg!(windows) {
        Ok(())
    } else {
        Err(format!("{program} failed ({status})"))
    }
}

/// The program and arguments that show `path`.
fn command(path: &Path) -> (&'static str, Vec<std::ffi::OsString>) {
    let file = path.is_file();
    if cfg!(target_os = "macos") {
        // Always select, never open: `open` on a folder named `X.app` launches it, and a peer
        // chooses the names in a download.
        ("open", vec!["-R".into(), path.into()])
    } else if cfg!(windows) {
        let arg = if file {
            let mut arg = std::ffi::OsString::from("/select,");
            arg.push(path);
            arg
        } else {
            path.into()
        };
        ("explorer", vec![arg])
    } else {
        let folder = match path.parent() {
            Some(parent) if file => parent,
            _ => path,
        };
        ("xdg-open", vec![folder.into()])
    }
}

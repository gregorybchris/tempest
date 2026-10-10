//! Making the operating system hand `tempest:` links to `tempest open` (D-263).
//!
//! Every command registers the running binary the first time it runs, unless `tempest
//! unregister` turned that off. `tempest register` and `tempest unregister` do it on request.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use tempest_engine::config::default_data_dir;

/// Present in the default data directory after `tempest unregister`, so that commands stop
/// registering.
const OFF_MARKER: &str = "link-handler-off";

/// Registers the running binary for `tempest:` links, unless the user turned that off or it is
/// a development build, and says so once. Never fails the command it runs before.
pub fn ensure() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    if is_development_build(&exe) || is_off() {
        return;
    }
    match register_binary(&exe) {
        Ok(true) => {
            eprintln!(
                "note: Tempest now opens tempest: links; `tempest unregister` turns that off"
            );
        }
        Ok(false) => {}
        Err(error) => tracing::debug!("cannot register tempest: links: {error:#}"),
    }
}

/// `tempest register`.
pub fn register() -> Result<()> {
    let exe = std::env::current_exe().context("cannot find tempest")?;
    if let Some(marker) = marker(OFF_MARKER)
        && marker.exists()
    {
        std::fs::remove_file(&marker)
            .with_context(|| format!("cannot remove {}", marker.display()))?;
    }
    match register_binary(&exe)? {
        true => println!("{} now opens tempest: links", exe.display()),
        false => println!("{} already opens tempest: links", exe.display()),
    }
    Ok(())
}

/// `tempest unregister`.
pub fn unregister() -> Result<()> {
    let marker = marker(OFF_MARKER).ok_or_else(|| anyhow!("cannot find the data directory"))?;
    if let Some(dir) = marker.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    }
    std::fs::write(&marker, "").with_context(|| format!("cannot write {}", marker.display()))?;
    os::unregister()?;
    println!("tempest: links no longer open Tempest; `tempest register` turns them on again");
    Ok(())
}

/// Registers `exe` unless it already is, and says whether anything changed.
fn register_binary(exe: &Path) -> Result<bool> {
    if os::is_registered(exe)? {
        return Ok(false);
    }
    os::register(exe)?;
    Ok(true)
}

fn is_off() -> bool {
    marker(OFF_MARKER).is_some_and(|m| m.exists())
}

/// A file in the default data directory, where per-user state that isn't an engine's lives.
fn marker(name: &str) -> Option<PathBuf> {
    Some(default_data_dir()?.join(name))
}

/// Whether `exe` is in a Cargo `target` directory, which should not take over the user's links.
fn is_development_build(exe: &Path) -> bool {
    exe.components().any(|c| c.as_os_str() == "target")
}

/// Quotes `arg` for a command line that a launcher splits into arguments, as the `Exec` key of
/// a desktop entry does.
#[cfg_attr(not(any(test, all(unix, not(target_os = "macos")))), allow(dead_code))]
fn desktop_quote(arg: &str) -> String {
    let mut quoted = String::from("\"");
    for c in arg.chars() {
        match c {
            '"' | '`' | '$' | '\\' => {
                quoted.push('\\');
                quoted.push('\\');
                quoted.push(c);
            }
            '%' => quoted.push_str("%%"),
            c => quoted.push(c),
        }
    }
    quoted.push('"');
    quoted
}

/// The desktop entry that makes `exe` the handler of `tempest:` links.
#[cfg_attr(not(any(test, all(unix, not(target_os = "macos")))), allow(dead_code))]
fn desktop_entry(exe: &Path) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Tempest\n\
         Comment=Download bundles from tempest: links\n\
         Exec={} open %u\n\
         Terminal=false\n\
         NoDisplay=true\n\
         MimeType=x-scheme-handler/tempest;\n",
        desktop_quote(&exe.to_string_lossy())
    )
}

/// The AppleScript of the application that macOS starts for a link.
#[cfg_attr(not(any(test, target_os = "macos")), allow(dead_code))]
fn applet_script(exe: &Path) -> String {
    let exe = exe
        .to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    format!(
        "on open location theURL\n\
         \tdo shell script (quoted form of \"{exe}\") & \" open \" & (quoted form of theURL) & \" >/dev/null 2>&1 &\"\n\
         end open location\n"
    )
}

#[cfg(all(unix, not(target_os = "macos")))]
mod os {
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use anyhow::{Context, Result, anyhow};

    const FILE: &str = "tempest.desktop";

    fn entry_path() -> Result<PathBuf> {
        let data = match std::env::var_os("XDG_DATA_HOME").filter(|d| !d.is_empty()) {
            Some(dir) => PathBuf::from(dir),
            None => {
                PathBuf::from(std::env::var_os("HOME").ok_or_else(|| anyhow!("HOME is not set"))?)
                    .join(".local/share")
            }
        };
        Ok(data.join("applications").join(FILE))
    }

    pub fn is_registered(exe: &Path) -> Result<bool> {
        let current = std::fs::read_to_string(entry_path()?).ok();
        Ok(current.as_deref() == Some(super::desktop_entry(exe).as_str()))
    }

    pub fn register(exe: &Path) -> Result<()> {
        let path = entry_path()?;
        std::fs::create_dir_all(path.parent().expect("a file has a parent"))
            .context("cannot create the applications folder")?;
        std::fs::write(&path, super::desktop_entry(exe))
            .with_context(|| format!("cannot write {}", path.display()))?;
        let status = Command::new("xdg-mime")
            .args(["default", FILE, "x-scheme-handler/tempest"])
            .status()
            .context("cannot run xdg-mime")?;
        match status.success() {
            true => Ok(()),
            false => Err(anyhow!("xdg-mime exited with {status}")),
        }
    }

    pub fn unregister() -> Result<()> {
        let path = entry_path()?;
        match std::fs::remove_file(&path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                Err(error).with_context(|| format!("cannot remove {}", path.display()))
            }
            _ => Ok(()),
        }
    }
}

#[cfg(target_os = "windows")]
mod os {
    use std::path::Path;
    use std::process::Command;

    use anyhow::{Context, Result, anyhow};

    const KEY: &str = r"HKCU\Software\Classes\tempest";

    fn command(exe: &Path) -> String {
        format!("\"{}\" open \"%1\"", exe.display())
    }

    fn reg(args: &[&str]) -> Result<std::process::Output> {
        Command::new("reg")
            .args(args)
            .output()
            .context("cannot run reg")
    }

    pub fn is_registered(exe: &Path) -> Result<bool> {
        let output = reg(&["query", &format!(r"{KEY}\shell\open\command"), "/ve"])?;
        Ok(output.status.success()
            && String::from_utf8_lossy(&output.stdout).contains(&command(exe)))
    }

    pub fn register(exe: &Path) -> Result<()> {
        let open = format!(r"{KEY}\shell\open\command");
        let command = command(exe);
        for args in [
            vec!["add", KEY, "/ve", "/d", "URL:Tempest", "/f"],
            vec!["add", KEY, "/v", "URL Protocol", "/d", "", "/f"],
            vec!["add", &open, "/ve", "/d", &command, "/f"],
        ] {
            let output = reg(&args)?;
            if !output.status.success() {
                return Err(anyhow!("reg exited with {}", output.status));
            }
        }
        Ok(())
    }

    pub fn unregister() -> Result<()> {
        // Fails when the key isn't there, which is the state asked for.
        reg(&["delete", KEY, "/f"]).map(|_| ())
    }
}

#[cfg(target_os = "macos")]
mod os {
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use anyhow::{Context, Result, anyhow};

    const URL_TYPES: &str = "<array><dict><key>CFBundleURLName</key><string>Tempest link</string>\
        <key>CFBundleURLSchemes</key><array><string>tempest</string></array></dict></array>";
    const LSREGISTER: &str = "/System/Library/Frameworks/CoreServices.framework/Frameworks/\
        LaunchServices.framework/Support/lsregister";

    fn app_path() -> Result<PathBuf> {
        let home = std::env::var_os("HOME").ok_or_else(|| anyhow!("HOME is not set"))?;
        Ok(PathBuf::from(home).join("Applications/Tempest Links.app"))
    }

    /// Where the binary the application runs is noted: the application holds it in compiled
    /// form only.
    fn exe_note() -> Result<PathBuf> {
        super::marker("link-handler-exe").ok_or_else(|| anyhow!("cannot find the data directory"))
    }

    pub fn is_registered(exe: &Path) -> Result<bool> {
        let note = std::fs::read_to_string(exe_note()?).ok();
        Ok(app_path()?.exists() && note.as_deref() == Some(&*exe.to_string_lossy()))
    }

    fn run(program: &str, args: &[&std::ffi::OsStr]) -> Result<()> {
        let output = Command::new(program)
            .args(args)
            .output()
            .with_context(|| format!("cannot run {program}"))?;
        match output.status.success() {
            true => Ok(()),
            false => Err(anyhow!(
                "{program} exited with {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            )),
        }
    }

    pub fn register(exe: &Path) -> Result<()> {
        let app = app_path()?;
        std::fs::create_dir_all(app.parent().expect("a file has a parent"))
            .context("cannot create ~/Applications")?;
        let _ = std::fs::remove_dir_all(&app);
        let script =
            std::env::temp_dir().join(format!("tempest-links-{}.applescript", std::process::id()));
        std::fs::write(&script, super::applet_script(exe)).context("cannot write the script")?;
        let compiled = run(
            "osacompile",
            &["-o".as_ref(), app.as_os_str(), script.as_os_str()],
        );
        let _ = std::fs::remove_file(&script);
        compiled?;
        let plist = app.join("Contents/Info.plist");
        run(
            "plutil",
            &[
                "-insert".as_ref(),
                "CFBundleURLTypes".as_ref(),
                "-xml".as_ref(),
                URL_TYPES.as_ref(),
                plist.as_os_str(),
            ],
        )?;
        // No Dock icon while it hands the link over.
        run(
            "plutil",
            &[
                "-replace".as_ref(),
                "LSUIElement".as_ref(),
                "-bool".as_ref(),
                "true".as_ref(),
                plist.as_os_str(),
            ],
        )?;
        run(
            "codesign",
            &[
                "--force".as_ref(),
                "--deep".as_ref(),
                "-s".as_ref(),
                "-".as_ref(),
                app.as_os_str(),
            ],
        )?;
        run(LSREGISTER, &["-f".as_ref(), app.as_os_str()])?;
        let note = exe_note()?;
        std::fs::create_dir_all(note.parent().expect("a file has a parent"))?;
        std::fs::write(&note, exe.to_string_lossy().as_bytes()).context("cannot write the note")
    }

    pub fn unregister() -> Result<()> {
        let app = app_path()?;
        if app.exists() {
            let _ = run(LSREGISTER, &["-u".as_ref(), app.as_os_str()]);
            std::fs::remove_dir_all(&app)
                .with_context(|| format!("cannot remove {}", app.display()))?;
        }
        let _ = std::fs::remove_file(exe_note()?);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_exec_quotes_the_path() {
        assert_eq!(desktop_quote("/home/a b/tempest"), "\"/home/a b/tempest\"");
        assert_eq!(desktop_quote("/a\"$%"), "\"/a\\\\\"\\\\$%%\"");
    }

    #[test]
    fn desktop_entry_names_the_scheme_and_passes_the_link() {
        let entry = desktop_entry(Path::new("/usr/bin/tempest"));
        assert!(entry.contains("Exec=\"/usr/bin/tempest\" open %u\n"));
        assert!(entry.contains("MimeType=x-scheme-handler/tempest;\n"));
    }

    #[test]
    fn the_applet_quotes_its_paths() {
        let script = applet_script(Path::new("/Users/a \"b\"/tempest"));
        assert!(script.contains("quoted form of \"/Users/a \\\"b\\\"/tempest\""));
    }

    #[test]
    fn cargo_builds_are_development_builds() {
        assert!(is_development_build(Path::new(
            "/src/tempest/target/debug/tempest"
        )));
        assert!(!is_development_build(Path::new(
            "/home/a/.cargo/bin/tempest"
        )));
    }
}

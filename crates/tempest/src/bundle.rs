//! Building a manifest from a file or folder on disk.

use std::fs::{self, File, Metadata};
use std::io::{ErrorKind, Read};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use tempest_protocol::tree::RootHasher;
use tempest_protocol::{Entry, FileInfo, Manifest};
use unicode_normalization::UnicodeNormalization;

/// The descriptive fields of a new bundle.
#[derive(Debug, Clone)]
pub struct BundleInfo {
    /// The bundle's name. Defaults to the shared file or folder's name.
    pub name: Option<String>,
    /// The creator's description.
    pub comment: String,
    /// When the bundle was created, in milliseconds since the Unix epoch.
    pub created: i64,
}

/// The `creator` field of bundles made by this program.
pub fn creator() -> String {
    format!("tempest {}", env!("CARGO_PKG_VERSION"))
}

/// Builds the manifest for the file or folder at `path`, hashing every file.
///
/// A folder's contents become the bundle's entries. A single file becomes a bundle with one
/// entry named after it. Symlinks inside a folder are kept as symlinks; `path` itself is followed.
pub fn build_manifest(path: &Path, info: BundleInfo) -> Result<Manifest> {
    let metadata = fs::metadata(path).with_context(|| format!("cannot read {}", path.display()))?;
    let own_name = || -> Result<String> {
        let absolute = fs::canonicalize(path)?;
        match absolute.file_name() {
            Some(name) => utf8_name(name, &absolute),
            None => bail!("{} has no name; pass --name", path.display()),
        }
    };
    let name = match info.name {
        Some(name) => name.nfc().collect(),
        None => own_name()?,
    };

    let mut entries = Vec::new();
    if metadata.is_dir() {
        add_dir_contents(path, "", &mut entries)?;
        if entries.is_empty() {
            bail!("{} is empty", path.display());
        }
    } else if metadata.is_file() {
        entries.push(file_entry(path, own_name()?, &metadata)?);
    } else {
        bail!("{} is not a file or folder", path.display());
    }

    Manifest::new(name, info.comment, info.created, creator(), entries)
        .context("cannot share this as a bundle")
}

/// Adds an entry for everything inside `dir`, whose path in the bundle is `prefix`.
fn add_dir_contents(dir: &Path, prefix: &str, entries: &mut Vec<Entry>) -> Result<()> {
    let read_dir = fs::read_dir(dir).with_context(|| format!("cannot read {}", dir.display()))?;
    for item in read_dir {
        let item = item.with_context(|| format!("cannot read {}", dir.display()))?;
        let fs_path = item.path();
        let name = utf8_name(&item.file_name(), &fs_path)?;
        let path = match prefix {
            "" => name,
            _ => format!("{prefix}/{name}"),
        };
        let metadata = fs::symlink_metadata(&fs_path)
            .with_context(|| format!("cannot read {}", fs_path.display()))?;
        let file_type = metadata.file_type();
        if file_type.is_symlink() {
            let target = fs::read_link(&fs_path)
                .with_context(|| format!("cannot read link {}", fs_path.display()))?;
            let target = target
                .to_str()
                .with_context(|| format!("link target of {} is not UTF-8", fs_path.display()))?
                .replace(std::path::MAIN_SEPARATOR, "/");
            entries.push(Entry::symlink(path, target.nfc().collect::<String>()));
        } else if file_type.is_dir() {
            let before = entries.len();
            add_dir_contents(&fs_path, &path, entries)?;
            if entries.len() == before {
                entries.push(Entry::directory(path));
            }
        } else if file_type.is_file() {
            entries.push(file_entry(&fs_path, path, &metadata)?);
        } else {
            bail!("{} is not a file, folder or symlink", fs_path.display());
        }
    }
    Ok(())
}

/// Hashes the file at `fs_path` into an entry at bundle path `path`.
fn file_entry(fs_path: &Path, path: String, metadata: &Metadata) -> Result<Entry> {
    let context = || format!("cannot read {}", fs_path.display());
    let mut file = File::open(fs_path).with_context(context)?;
    let mut hasher = RootHasher::new();
    let mut buffer = vec![0; 1 << 20];
    loop {
        match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => hasher.update(&buffer[..n]),
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) => return Err(e).with_context(context),
        }
    }
    let modified = metadata.modified().with_context(context)?;
    Ok(Entry::file(
        path,
        FileInfo {
            size: hasher.size(),
            root: hasher.finish(),
            executable: is_executable(metadata),
            modified: unix_millis(modified),
        },
    ))
}

/// A file name as NFC text, as manifests require.
fn utf8_name(name: &std::ffi::OsStr, fs_path: &Path) -> Result<String> {
    let name = name
        .to_str()
        .with_context(|| format!("the name of {} is not UTF-8", fs_path.display()))?;
    Ok(name.nfc().collect())
}

#[cfg(unix)]
fn is_executable(metadata: &Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o100 != 0
}

#[cfg(not(unix))]
fn is_executable(_metadata: &Metadata) -> bool {
    false
}

/// `time` as milliseconds since the Unix epoch, negative before it.
pub fn unix_millis(time: SystemTime) -> i64 {
    let clamp = |millis: u128| i64::try_from(millis).unwrap_or(i64::MAX);
    match time.duration_since(UNIX_EPOCH) {
        Ok(after) => clamp(after.as_millis()),
        Err(before) => -clamp(before.duration().as_millis()),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use tempest_protocol::EntryKind;

    fn info() -> BundleInfo {
        BundleInfo {
            name: None,
            comment: String::new(),
            created: 0,
        }
    }

    #[test]
    fn millis_before_and_after_the_epoch() {
        assert_eq!(unix_millis(UNIX_EPOCH + Duration::from_millis(1500)), 1500);
        assert_eq!(unix_millis(UNIX_EPOCH - Duration::from_millis(1500)), -1500);
    }

    #[test]
    fn a_single_file_is_one_entry_named_after_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.txt");
        fs::write(&path, vec![7; 20_000]).unwrap();
        let manifest = build_manifest(&path, info()).unwrap();
        assert_eq!(manifest.name(), "notes.txt");
        let [entry] = manifest.entries() else {
            panic!("one entry expected");
        };
        assert_eq!(entry.path, "notes.txt");
        let file = entry.as_file().unwrap();
        assert_eq!(file.size, 20_000);
        assert_eq!(file.root, tempest_protocol::tree::root(&[7; 20_000]));
    }

    #[test]
    fn empty_folders_and_invalid_names_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        assert!(build_manifest(dir.path(), info()).is_err());
        fs::write(dir.path().join("a:b"), "x").unwrap();
        let error = build_manifest(dir.path(), info()).unwrap_err();
        assert!(format!("{error:#}").contains("a:b"), "{error:#}");
    }

    #[test]
    fn names_are_normalized_to_nfc() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("e\u{301}"), "x").unwrap();
        let manifest = build_manifest(dir.path(), info()).unwrap();
        assert_eq!(manifest.entries()[0].path, "\u{e9}");
    }

    #[cfg(unix)]
    #[test]
    fn executable_bit_and_symlinks() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("run.sh");
        fs::write(&script, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        symlink("run.sh", dir.path().join("link")).unwrap();

        let manifest = build_manifest(dir.path(), info()).unwrap();
        let [link, run] = manifest.entries() else {
            panic!("two entries expected");
        };
        assert_eq!(
            link.kind,
            EntryKind::Symlink {
                target: "run.sh".into()
            }
        );
        assert!(run.as_file().unwrap().executable);
    }
}

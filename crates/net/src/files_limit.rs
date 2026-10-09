//! Raising the process's limit on open files.
//!
//! Running bundles keep some of their files open, and servers keep a descriptor per connection
//! or lent socket. The default soft limit is low (256 on macOS, 1,024 on most Linux systems),
//! while the hard limit a process may raise it to is usually far higher.

/// The most macOS lets the soft limit be set to, whatever the hard limit says.
#[cfg(target_os = "macos")]
const MACOS_MAX: u64 = 10_240;

/// Raises the soft limit on open files as far as the hard limit allows, and returns the new
/// limit. Does nothing where there is no such limit.
pub fn raise_open_file_limit() -> std::io::Result<Option<u64>> {
    #[cfg(unix)]
    {
        use rustix::process::{Resource, getrlimit, setrlimit};

        let limit = getrlimit(Resource::Nofile);
        let Some(current) = limit.current else {
            return Ok(None);
        };
        let target = limit.maximum.unwrap_or(u64::MAX);
        #[cfg(target_os = "macos")]
        let target = target.min(MACOS_MAX);
        if target <= current {
            return Ok(Some(current));
        }
        let raised = rustix::process::Rlimit {
            current: Some(target),
            maximum: limit.maximum,
        };
        setrlimit(Resource::Nofile, raised)?;
        Ok(Some(target))
    }
    #[cfg(not(unix))]
    Ok(None)
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_limit_only_rises() {
        let first = super::raise_open_file_limit().unwrap();
        let second = super::raise_open_file_limit().unwrap();
        assert_eq!(first, second);
        #[cfg(unix)]
        assert!(first.unwrap() >= 256);
    }
}

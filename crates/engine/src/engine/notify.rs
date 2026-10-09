//! The notification command, run when a transfer completes or fails.

use std::process::Stdio;

use crate::{Status, TransferInfo};

/// What happened to a transfer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Notice {
    /// Every chunk is stored.
    Completed,
    /// It stopped with an error.
    Errored,
}

impl Notice {
    fn name(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Errored => "errored",
        }
    }
}

/// The variables that describe `notice` about `transfer` to the command.
pub(super) fn environment(notice: Notice, transfer: &TransferInfo) -> Vec<(&'static str, String)> {
    let path = |path: &std::path::Path| path.display().to_string();
    let mut vars = vec![
        ("TEMPEST_EVENT", notice.name().to_owned()),
        ("TEMPEST_ID", transfer.id.to_string()),
        ("TEMPEST_NAME", transfer.name.clone()),
        ("TEMPEST_KIND", transfer.kind.as_str().to_owned()),
        ("TEMPEST_SAVE_FOLDER", path(&transfer.save_folder)),
        ("TEMPEST_LINK", transfer.link.clone()),
    ];
    if let Some(size) = transfer.size {
        vars.push(("TEMPEST_SIZE", size.to_string()));
    }
    if let Some(dir) = &transfer.content_dir {
        vars.push(("TEMPEST_CONTENT_DIR", path(dir)));
    }
    if let Status::Errored { reason } = &transfer.status {
        vars.push(("TEMPEST_ERROR", reason.clone()));
    }
    vars
}

/// Runs `command` for `notice` about `transfer`, without waiting for it. A command that can't
/// start is ignored: notifications never hold up transfers.
pub(super) fn run(command: &[String], notice: Notice, transfer: &TransferInfo) {
    let Some((program, args)) = command.split_first() else {
        return;
    };
    tracing::debug!(
        "running the {notice:?} notification for {}: {program}",
        transfer.id
    );
    let spawned = tokio::process::Command::new(program)
        .args(args)
        .envs(environment(notice, transfer))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    match spawned {
        Ok(mut child) => {
            // Waiting reaps the process once it exits.
            tokio::spawn(async move {
                let _ = child.wait().await;
            });
        }
        Err(error) => tracing::warn!("cannot run the notification command {program}: {error}"),
    }
}

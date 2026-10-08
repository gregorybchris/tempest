//! Handing an engine that runs only for a UI to a daemon when the UI closes (D-228): the
//! transfers stay wanted in the engine's state, so a daemon started on the same data directory
//! resumes them once this process's engine has shut down and released it.

use std::fs::OpenOptions;
use std::io::{BufRead, IsTerminal};
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::Value;
use tempest_engine::{Engine, EngineConfig};
use tokio::time::Instant;

use crate::engine::{EngineArgs, Target};

/// The daemon's log file in the data directory.
pub const LOG_FILE: &str = "daemon.log";

/// How long to wait for the daemon's API to answer.
const START_WAIT: Duration = Duration::from_secs(20);

/// What a daemon needs to run as the engine that is closing did.
#[derive(Debug, Clone)]
pub struct Daemon {
    /// Which engine: its config file and data directory.
    pub engine: EngineArgs,
    /// Settings overridden for the run, as `tempest daemon --set` takes them.
    pub set: Vec<(String, Value)>,
}

impl Daemon {
    /// Starts `tempest daemon` in the background on `config`'s data directory and waits for its
    /// API to answer, returning the API's URL. The engine here must have shut down already.
    pub async fn start(&self, config: &EngineConfig) -> Result<String> {
        let data_dir = config.data_dir()?;
        let log_path = data_dir.join(LOG_FILE);
        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .with_context(|| format!("cannot open {}", log_path.display()))?;
        let mut command = Command::new(std::env::current_exe().context("cannot find tempest")?);
        command.arg("daemon").arg("--data-dir").arg(&data_dir);
        if let Some(config_file) = &self.engine.config {
            command.arg("--config").arg(config_file);
        }
        for (key, value) in &self.set {
            command.arg("--set").arg(format!("{key}={value}"));
        }
        command
            .stdin(Stdio::null())
            .stdout(log.try_clone().context("cannot share the log file")?)
            .stderr(log);
        detach(&mut command);
        let mut child = command.spawn().context("cannot start the daemon")?;

        let deadline = Instant::now() + START_WAIT;
        loop {
            if let Some(client) = Target::daemon(config).await? {
                return Ok(client.url().to_owned());
            }
            if let Some(status) = child.try_wait()? {
                bail!(
                    "the daemon exited ({status}); {} says why",
                    log_path.display()
                );
            }
            if Instant::now() >= deadline {
                bail!(
                    "the daemon didn't answer within {} seconds; {} may say why",
                    START_WAIT.as_secs(),
                    log_path.display()
                );
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
}

/// Puts the child in a group of its own, so the terminal's Ctrl-C doesn't reach it.
fn detach(command: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP
        command.creation_flags(0x0000_0008 | 0x0000_0200);
    }
}

/// Says that `count` transfers carry on in the daemon at `url`.
pub fn report(count: usize, url: &str) {
    let what = match count {
        1 => "1 transfer carries".to_owned(),
        n => format!("{n} transfers carry"),
    };
    eprintln!("{what} on in a daemon at {url}");
}

/// What the user chose to do with the active transfers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    /// Carry on in a daemon.
    HandOff,
    /// Stop with the engine.
    Stop,
}

/// Stops every active transfer, as `tempest stop` does, so none is wanted when an engine opens
/// on the data directory again.
pub async fn stop_active(engine: &Engine) {
    for transfer in engine.list().await {
        if transfer.status.is_active() {
            let _ = engine.stop(transfer.id).await;
        }
    }
}

/// What an answer typed at the question means: handing over is the default, and anything
/// unclear stops the transfers.
pub fn parse_choice(answer: &str) -> Choice {
    match answer.trim().to_lowercase().as_str() {
        "" | "d" | "y" | "yes" | "daemon" => Choice::HandOff,
        _ => Choice::Stop,
    }
}

/// Asks on the terminal what to do with `active` transfers. Reads on a thread of its own, so a
/// question left unanswered never holds up exiting.
pub async fn ask(active: usize) -> Choice {
    if !std::io::stdin().is_terminal() {
        return Choice::Stop;
    }
    let what = match active {
        1 => "1 transfer is".to_owned(),
        n => format!("{n} transfers are"),
    };
    eprintln!(
        "{what} running here. Hand them to a daemon that carries on in the background, or stop \
         them with the engine?"
    );
    eprint!("[D]aemon / [s]top (Ctrl-C stops): ");
    let (sender, answer) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = std::io::stdin().lock().read_line(&mut line);
        let _ = sender.send(line);
    });
    tokio::select! {
        line = answer => line.map_or(Choice::Stop, |line| parse_choice(&line)),
        _ = tokio::signal::ctrl_c() => Choice::Stop,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_answer_defaults_to_handing_over_and_anything_else_stops() {
        for yes in ["", "d", "D", "y", "Yes", " daemon \n"] {
            assert_eq!(parse_choice(yes), Choice::HandOff, "{yes:?}");
        }
        for no in ["s", "stop", "n", "x"] {
            assert_eq!(parse_choice(no), Choice::Stop, "{no:?}");
        }
    }
}

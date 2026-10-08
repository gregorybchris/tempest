//! `tempest daemon` and the commands that talk to it, as separate processes on localhost.

#![cfg(unix)]

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use tempest_net::{TrackerClient, TrackerError};
use tempest_protocol::BundleId;

/// How long a process may take to start or finish.
const LIMIT: Duration = Duration::from_secs(60);

fn tempest(data_dir: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_tempest"));
    command
        .env_remove("TEMPEST_IDENTITY")
        .env_remove("TEMPEST_CONFIG")
        .env("TEMPEST_DATA_DIR", data_dir);
    command
}

/// Runs a command to completion, asserting that it succeeded.
fn run(command: &mut Command) -> Output {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{:?} failed: {}\n{}",
        command,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).unwrap()
}

/// A running `tempest daemon` on 127.0.0.1 with ports the system picks and LAN discovery off,
/// killed if still running when dropped.
struct Daemon {
    child: Child,
    stderr: mpsc::Receiver<String>,
}

impl Daemon {
    fn start(data_dir: &Path, extra: &[&str]) -> Self {
        let downloads = data_dir.join("downloads");
        let mut child = tempest(data_dir)
            .args(["daemon", "--set", "api.port=0", "--set", "peers.port=0"])
            .args([
                "--set",
                r#"peers.bind=["127.0.0.1"]"#,
                "--set",
                "lan.enabled=false",
            ])
            .arg("--set")
            .arg(format!("save_folder={}", downloads.display()))
            .args(extra)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stderr = lines(child.stderr.take().unwrap());
        let started = Instant::now();
        loop {
            let line = stderr
                .recv_timeout(LIMIT.saturating_sub(started.elapsed()))
                .expect("the daemon starts");
            if line.starts_with("running") {
                break;
            }
        }
        Self { child, stderr }
    }

    /// Interrupts the daemon as Ctrl-C would, and returns what it printed as it stopped.
    fn interrupt(mut self) -> String {
        let pid = self.child.id().to_string();
        run(Command::new("kill").args(["-INT", &pid]));
        let status = self.child.wait().unwrap();
        assert!(status.success(), "the daemon exits cleanly: {status}");
        self.stderr.try_iter().collect::<Vec<_>>().join("\n")
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The lines `stream` yields, read on another thread.
fn lines(stream: impl std::io::Read + Send + 'static) -> mpsc::Receiver<String> {
    let (lines, received) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stream).lines() {
            let Ok(line) = line else { break };
            if lines.send(line).is_err() {
                break;
            }
        }
    });
    received
}

/// The value of the stdout line starting with `name`.
fn field(output: &Output, name: &str) -> String {
    stdout(output)
        .lines()
        .find_map(|line| line.strip_prefix(name))
        .unwrap_or_else(|| panic!("no {name} line in {}", stdout(output)))
        .trim()
        .to_owned()
}

/// Runs `tempest list --json` and returns its transfers.
fn transfers(data_dir: &Path) -> Vec<serde_json::Value> {
    let output = run(tempest(data_dir).args(["list", "--json"]));
    serde_json::from_slice(&output.stdout).unwrap()
}

/// Waits until the transfer `id` in `data_dir`'s engine is in `state`.
fn wait_for_state(data_dir: &Path, id: &str, state: &str) {
    let started = Instant::now();
    loop {
        let found = transfers(data_dir);
        let transfer = found.iter().find(|t| t["id"] == id);
        if transfer.is_some_and(|t| t["status"]["state"] == state) {
            return;
        }
        assert!(started.elapsed() < LIMIT, "never {state}: {transfer:?}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Pseudo-random bytes, so that no two chunks are alike.
fn contents(size: usize, seed: u64) -> Vec<u8> {
    let mut state = seed;
    (0..size)
        .map(|_| {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (state >> 56) as u8
        })
        .collect()
}

/// A running `tempest tracker` on localhost, killed when dropped.
struct TrackerProcess {
    child: Child,
    url: String,
}

impl TrackerProcess {
    fn start(dir: &Path) -> Self {
        Self::start_with(dir, &[])
    }

    /// [`TrackerProcess::start`] with `extra` arguments.
    fn start_with(dir: &Path, extra: &[&str]) -> Self {
        let mut child = tempest(dir)
            .args(["tracker", "--bind", "127.0.0.1", "--port", "0"])
            .args(extra)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let line = lines(child.stderr.take().unwrap())
            .recv_timeout(LIMIT)
            .expect("the tracker printed nothing");
        let url = line
            .strip_prefix("tracker listening on ")
            .and_then(|rest| rest.split(';').next())
            .unwrap_or_else(|| panic!("unexpected line: {line}"))
            .to_owned();
        Self { child, url }
    }

    /// How many complete peers the tracker lists for `bundle`.
    fn seeders(&self, bundle: &str) -> u64 {
        let bundle: BundleId = bundle.parse().unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let client = TrackerClient::new(LIMIT).unwrap();
        let stats = runtime.block_on(client.stats(&self.url, &bundle)).unwrap();
        stats.complete
    }

    /// The status of `bundle`'s listing in the catalog, as anyone browsing sees it: 200 or 404.
    fn listing_status(&self, bundle: &str) -> u16 {
        let bundle: BundleId = bundle.parse().unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let client = TrackerClient::new(LIMIT).unwrap();
        match runtime.block_on(client.listing(&self.url, None, &bundle)) {
            Ok(_) => 200,
            Err(TrackerError::Refused { status, .. }) => status,
            Err(error) => panic!("{error}"),
        }
    }
}

impl Drop for TrackerProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn transfers_run_through_daemons_and_the_cli_controls_them() {
    let root = tempfile::tempdir().unwrap();
    let tracker = TrackerProcess::start(root.path());
    let source = root.path().join("album");
    fs::create_dir_all(&source).unwrap();
    let files = [
        ("one.flac", contents(3 << 20, 1)),
        ("two.flac", contents(1 << 20, 2)),
    ];
    for (name, data) in &files {
        fs::write(source.join(name), data).unwrap();
    }

    // `share` hands the folder to the sharing daemon, which announces it to its tracker.
    let a = root.path().join("a");
    let trackers = format!(r#"trackers=["{}"]"#, tracker.url);
    let sharer = Daemon::start(&a, &["--set", &trackers]);
    let shared = run(tempest(&a).arg("share").arg(&source));
    assert!(stderr(&shared).contains("daemon"), "{}", stderr(&shared));
    let link = field(&shared, "link");
    let id = field(&shared, "bundle");
    assert!(
        link.contains("tracker="),
        "the link names the tracker: {link}"
    );
    wait_for_state(&a, &id, "sharing");
    let started = Instant::now();
    while tracker.seeders(&id) == 0 {
        assert!(started.elapsed() < LIMIT, "the sharer never announced");
        std::thread::sleep(Duration::from_millis(100));
    }

    // `get` hands the link to the downloading daemon and follows it to completion.
    let b = root.path().join("b");
    let downloader = Daemon::start(&b, &[]);
    let output = root.path().join("out");
    let got = run(tempest(&b).args(["get", &link, "--output"]).arg(&output));
    let folder = PathBuf::from(stdout(&got).trim());
    assert_eq!(folder, output.join("album"));
    for (name, data) in &files {
        assert_eq!(&fs::read(folder.join(name)).unwrap(), data, "{name}");
    }

    // The list shows it, and the commands control it by name or ID prefix.
    let list = stdout(&run(tempest(&b).arg("list")));
    let header = list.lines().next().unwrap();
    for column in [
        "ID", "NAME", "STATUS", "DONE", "SIZE", "PEERS", "ETA", "RATIO",
    ] {
        assert!(header.contains(column), "{header}");
    }
    let row = list.lines().nth(1).unwrap();
    assert!(row.starts_with(&id[..8]) && row.contains("album"), "{list}");
    assert!(row.contains("100%"), "{list}");
    let stopped = run(tempest(&b).args(["stop", "album"]));
    assert!(stdout(&stopped).contains("stopped"), "{}", stdout(&stopped));
    wait_for_state(&b, &id, "stopped");
    run(tempest(&b).args(["start", &id[..6]]));
    wait_for_state(&b, &id, "sharing");
    let missing = tempest(&b).args(["start", "nothing"]).output().unwrap();
    assert!(!missing.status.success());
    assert!(
        stderr(&missing).contains("no transfer matches"),
        "{}",
        stderr(&missing)
    );

    run(tempest(&b).args(["remove", "--delete-files", "album"]));
    assert!(transfers(&b).is_empty());
    assert!(!folder.exists(), "removed with its files");

    // Ctrl-C stops the daemon cleanly: it tells the tracker and removes its API's address.
    let said = sharer.interrupt();
    assert!(said.contains("stopped"), "{said}");
    assert_eq!(tracker.seeders(&id), 0, "the tracker heard `stopped`");
    assert!(!a.join("api.json").exists());

    // With no daemon, the commands open the engine's state here and say so.
    let listed = run(tempest(&a).arg("list"));
    assert!(stdout(&listed).contains("album"), "{}", stdout(&listed));
    assert!(
        stderr(&listed).contains("no daemon is running"),
        "{}",
        stderr(&listed)
    );
    let stopped = run(tempest(&a).args(["stop", &id]));
    assert!(stdout(&stopped).contains("stopped"), "{}", stdout(&stopped));
    assert_eq!(transfers(&a)[0]["status"]["state"], "stopped");

    downloader.interrupt();
}

#[test]
fn the_cli_reaches_a_daemon_with_remote_access_through_its_data_directory() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("engine");
    let daemon = Daemon::start(&dir, &["--set", "api.remote=true"]);
    let api: serde_json::Value =
        serde_json::from_slice(&fs::read(dir.join("api.json")).unwrap()).unwrap();
    assert!(
        api["url"].as_str().unwrap().starts_with("https://"),
        "{api}"
    );

    let listed = run(tempest(&dir).arg("list"));
    assert_eq!(stdout(&listed).trim(), "no transfers");
    assert!(
        !stderr(&listed).contains("no daemon"),
        "{}",
        stderr(&listed)
    );

    // Another engine can't open the same state while the daemon runs.
    let second = tempest(&dir)
        .args(["daemon", "--set", "api.port=0"])
        .output()
        .unwrap();
    assert!(!second.status.success());
    assert!(
        stderr(&second).contains("already running"),
        "{}",
        stderr(&second)
    );
    daemon.interrupt();
}

/// `tempest share --publish` lists a share through the daemon, and `tempest catalog unpublish`
/// withdraws it, through the daemon or with none running.
#[test]
fn the_cli_publishes_and_unpublishes_through_the_daemon() {
    let root = tempfile::tempdir().unwrap();
    let catalog = root.path().join("catalog.db");
    let catalog = catalog.to_str().unwrap();
    let tracker = TrackerProcess::start_with(root.path(), &["--catalog", catalog]);
    let source = root.path().join("notes");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("a.txt"), contents(2000, 3)).unwrap();

    let dir = root.path().join("engine");
    let daemon = Daemon::start(&dir, &[]);
    let shared = run(tempest(&dir)
        .args(["share", "--publish", &tracker.url])
        .arg(&source));
    let id = field(&shared, "bundle");
    let started = Instant::now();
    while tracker.listing_status(&id) != 200 {
        assert!(started.elapsed() < LIMIT, "never listed");
        std::thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(transfers(&dir)[0]["published"][0], tracker.url.as_str());

    let withdrawn = run(tempest(&dir).args(["catalog", "unpublish", "notes", &tracker.url]));
    assert!(
        stdout(&withdrawn).contains("withdrawn"),
        "{}",
        stdout(&withdrawn)
    );
    assert_eq!(tracker.listing_status(&id), 404);
    assert_eq!(transfers(&dir)[0]["published"], serde_json::json!([]));

    // With no daemon, the command signs in with the engine's key itself.
    daemon.interrupt();
    let again = run(tempest(&dir).args(["catalog", "unpublish", &id, &tracker.url]));
    assert!(stdout(&again).contains("withdrawn"), "{}", stdout(&again));
    assert_eq!(tracker.listing_status(&id), 404);
}

#[test]
fn share_options_for_one_transfer_do_not_run_it_standalone() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("notes.txt");
    fs::write(&source, b"some notes").unwrap();
    let created = "1700000000000";
    // Nothing listens there, which announcing copes with.
    let tracker = "http://127.0.0.1:9/announce";

    let a = root.path().join("a");
    let daemon = Daemon::start(&a, &[]);
    let shared = run(tempest(&a)
        .args(["share", "--created", created, "--tracker", tracker])
        .arg(&source));
    assert!(stderr(&shared).contains("daemon"), "{}", stderr(&shared));
    let link = field(&shared, "link");
    assert!(link.contains("tracker=http"), "{link}");

    // The daemon made the same bundle a standalone dry run does, so it took the creation time.
    let dry = run(tempest(&a)
        .args(["share", "--dry-run", "--created", created])
        .arg(&source));
    assert_eq!(field(&shared, "bundle"), field(&dry, "bundle"));
    let listed = transfers(&a);
    assert_eq!(listed.len(), 1);
    assert!(listed[0]["link"].as_str().unwrap().contains("tracker=http"));

    daemon.interrupt();
}

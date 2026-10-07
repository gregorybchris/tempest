//! App state and actions. Nothing here draws; see `ui.rs`.

use proto_core::{format, Bundle, BundleState, World};

use crate::theme::Theme;

/// What the user has asked a transfer to do (qBittorrent-style controls).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    /// Runs when the queue allows.
    Running,
    /// Waiting for a free slot in the queue.
    Queued,
    /// Runs regardless of the queue.
    Forced,
    /// Stopped by the user.
    Stopped,
}

/// A transfer's status as the list shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    Downloading,
    Sharing,
    Stalled,
    Queued,
    Stopped,
    Completed,
    Errored,
}

impl Status {
    pub const ALL: [Status; 7] = [
        Status::Downloading,
        Status::Sharing,
        Status::Stalled,
        Status::Queued,
        Status::Stopped,
        Status::Completed,
        Status::Errored,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Status::Downloading => "downloading",
            Status::Sharing => "sharing",
            Status::Stalled => "stalled",
            Status::Queued => "queued",
            Status::Stopped => "stopped",
            Status::Completed => "completed",
            Status::Errored => "errored",
        }
    }
}

/// One row in the transfers list: a simulated bundle plus user settings.
pub struct Transfer {
    world: usize,
    index: usize,
    pub name: String,
    pub tags: Vec<String>,
    pub save_path: String,
    pub control: Control,
    pub error: Option<String>,
    /// The bundle as it was when stopped, so a stopped transfer stays still.
    frozen: Option<Bundle>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortKey {
    Added,
    Name,
    Size,
    Progress,
    Status,
    Down,
    Up,
    Peers,
    Eta,
    Ratio,
}

impl SortKey {
    pub const ALL: [SortKey; 10] = [
        SortKey::Added,
        SortKey::Name,
        SortKey::Size,
        SortKey::Progress,
        SortKey::Status,
        SortKey::Down,
        SortKey::Up,
        SortKey::Peers,
        SortKey::Eta,
        SortKey::Ratio,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SortKey::Added => "added",
            SortKey::Name => "name",
            SortKey::Size => "size",
            SortKey::Progress => "progress",
            SortKey::Status => "status",
            SortKey::Down => "download",
            SortKey::Up => "upload",
            SortKey::Peers => "peers",
            SortKey::Eta => "time left",
            SortKey::Ratio => "ratio",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Overview,
    Peers,
    Files,
    Decisions,
}

impl Tab {
    pub const ALL: [Tab; 4] = [Tab::Overview, Tab::Peers, Tab::Files, Tab::Decisions];

    pub fn label(self) -> &'static str {
        match self {
            Tab::Overview => "overview",
            Tab::Peers => "peers",
            Tab::Files => "files",
            Tab::Decisions => "decisions",
        }
    }
}

/// What the keyboard is currently talking to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Filter,
    Rename(String),
    Tags(String),
    Add(String),
    ConfirmRemove,
    Palette { query: String, selected: usize },
    Help,
}

/// A command, as the palette and help list them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Add,
    StartStop,
    ForceStart,
    Rename,
    Tags,
    OpenFolder,
    Remove,
    Filter,
    NextSort,
    ReverseSort,
    NextStatus,
    NextTab,
    NextTheme,
    Help,
    Quit,
}

impl Command {
    pub const ALL: [Command; 15] = [
        Command::Add,
        Command::StartStop,
        Command::ForceStart,
        Command::Rename,
        Command::Tags,
        Command::OpenFolder,
        Command::Remove,
        Command::Filter,
        Command::NextSort,
        Command::ReverseSort,
        Command::NextStatus,
        Command::NextTab,
        Command::NextTheme,
        Command::Help,
        Command::Quit,
    ];

    /// The key that runs this command in normal mode.
    pub fn key(self) -> &'static str {
        match self {
            Command::Add => "a",
            Command::StartStop => "space",
            Command::ForceStart => "F",
            Command::Rename => "r",
            Command::Tags => "t",
            Command::OpenFolder => "o",
            Command::Remove => "x",
            Command::Filter => "/",
            Command::NextSort => "s",
            Command::ReverseSort => "S",
            Command::NextStatus => "[ ]",
            Command::NextTab => "tab",
            Command::NextTheme => "T",
            Command::Help => "?",
            Command::Quit => "q",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Command::Add => "add a bundle from a link",
            Command::StartStop => "start or stop the selected transfer",
            Command::ForceStart => "force start (ignore the queue)",
            Command::Rename => "rename",
            Command::Tags => "edit tags",
            Command::OpenFolder => "open the download folder",
            Command::Remove => "remove",
            Command::Filter => "filter by name or tag",
            Command::NextSort => "change sort column",
            Command::ReverseSort => "reverse sort order",
            Command::NextStatus => "filter by status",
            Command::NextTab => "next detail tab",
            Command::NextTheme => "next theme",
            Command::Help => "help",
            Command::Quit => "quit",
        }
    }
}

pub struct App {
    worlds: Vec<World>,
    pub transfers: Vec<Transfer>,
    /// Transfer indexes in display order, after filtering and sorting.
    pub visible: Vec<usize>,
    /// Position in `visible`.
    pub cursor: usize,
    pub filter: String,
    /// `None` shows every status.
    pub status_filter: Option<Status>,
    pub sort: SortKey,
    pub descending: bool,
    pub tab: Tab,
    pub mode: Mode,
    pub theme: Theme,
    /// A short message at the bottom, and when it expires (sim ms).
    pub toast: Option<(String, u64)>,
    pub quit: bool,
}

impl App {
    pub fn new() -> Self {
        let mut worlds = vec![World::new(42), World::new(7), World::new(99)];
        let specs: [(&str, u64, &[&str], Control, Option<&str>); 12] = [
            ("ubuntu-26.04-desktop-amd64.iso", 6_412_820_480, &["linux"], Control::Running, None),
            ("openwebtext-shard-0412", 48_031_002_112, &["ml", "dataset"], Control::Running, None),
            ("holiday-photos-2026", 1_318_402_048, &["personal"], Control::Running, None),
            ("llama-4-scout-weights", 218_993_510_400, &["ml", "model"], Control::Running, None),
            ("debian-13.2.0-amd64-netinst.iso", 742_391_808, &["linux"], Control::Running, None),
            ("common-voice-24-en", 92_480_118_784, &["ml", "dataset"], Control::Running, None),
            ("blender-5.1-demo-files", 3_221_225_472, &["3d"], Control::Running, None),
            ("gutenberg-mirror-2026", 1_073_741_824_000, &["archive"], Control::Queued, None),
            ("fedora-kde-44-x86_64.iso", 2_791_309_312, &["linux"], Control::Stopped, None),
            ("mnist-and-friends", 1_181_116_006, &["ml", "dataset"], Control::Queued, None),
            ("family-videos-2025", 41_875_931_136, &["personal"], Control::Stopped, None),
            ("old-game-patch-1.4.2", 5_368_709_120, &["games"], Control::Running, Some("Not enough disk space: 2.1 GB more needed on /Volumes/Media")),
        ];
        let mut transfers = Vec::new();
        for (i, (name, size, tags, control, error)) in specs.into_iter().enumerate() {
            let (world, index) = (i / 4, i % 4);
            let bundle = &mut worlds[world].bundles[index];
            bundle.name = name.to_string();
            bundle.size_bytes = size;
            let frozen = (control != Control::Running || error.is_some()).then(|| freeze(bundle.clone()));
            transfers.push(Transfer {
                world,
                index,
                name: name.to_string(),
                tags: tags.iter().map(|t| t.to_string()).collect(),
                save_path: format!("~/Downloads/{name}"),
                control,
                error: error.map(str::to_string),
                frozen,
            });
        }
        let mut app = Self {
            worlds,
            transfers,
            visible: Vec::new(),
            cursor: 0,
            filter: String::new(),
            status_filter: None,
            sort: SortKey::Added,
            descending: false,
            tab: Tab::Overview,
            mode: Mode::Normal,
            theme: Theme::GREEN,
            toast: None,
            quit: false,
        };
        app.refresh();
        app
    }

    pub fn tick(&mut self, dt_ms: u64) {
        for world in &mut self.worlds {
            world.tick(dt_ms);
        }
        if self.toast.as_ref().is_some_and(|(_, until)| self.now() > *until) {
            self.toast = None;
        }
        self.refresh();
    }

    pub fn now(&self) -> u64 {
        self.worlds[0].t_ms
    }

    /// The bundle to display for a transfer.
    pub fn bundle<'a>(&'a self, t: &'a Transfer) -> &'a Bundle {
        t.frozen.as_ref().unwrap_or(&self.worlds[t.world].bundles[t.index])
    }

    pub fn status(&self, t: &Transfer) -> Status {
        let b = self.bundle(t);
        if t.error.is_some() {
            return Status::Errored;
        }
        match (t.control, b.state) {
            (Control::Stopped, BundleState::Sharing) => Status::Completed,
            (Control::Stopped, _) => Status::Stopped,
            (Control::Queued, _) => Status::Queued,
            (_, BundleState::Downloading) => Status::Downloading,
            (_, BundleState::Sharing) => Status::Sharing,
            (_, BundleState::WaitingForPeers) => Status::Stalled,
        }
    }

    /// Download rate as shown: zero unless the transfer is active.
    pub fn down_bps(&self, t: &Transfer) -> f64 {
        if t.frozen.is_some() { 0.0 } else { self.bundle(t).down_bps }
    }

    pub fn up_bps(&self, t: &Transfer) -> f64 {
        if t.frozen.is_some() { 0.0 } else { self.bundle(t).up_bps }
    }

    /// Peers currently sending us data.
    pub fn serving_us(&self, t: &Transfer) -> usize {
        if t.frozen.is_some() {
            return 0;
        }
        self.bundle(t).peers.iter().filter(|p| p.serves_us).count()
    }

    pub fn selected(&self) -> Option<&Transfer> {
        self.visible.get(self.cursor).map(|&i| &self.transfers[i])
    }

    pub fn count(&self, status: Option<Status>) -> usize {
        self.transfers.iter().filter(|t| status.is_none_or(|s| self.status(t) == s)).count()
    }

    pub fn total_down(&self) -> f64 {
        self.transfers.iter().map(|t| self.down_bps(t)).sum()
    }

    pub fn total_up(&self) -> f64 {
        self.transfers.iter().map(|t| self.up_bps(t)).sum()
    }

    /// Recomputes `visible` after any change to filters, sorting or data,
    /// keeping the same transfer selected when possible.
    pub fn refresh(&mut self) {
        let keep = self.visible.get(self.cursor).copied();
        let needle = self.filter.to_lowercase();
        let mut visible: Vec<usize> = (0..self.transfers.len())
            .filter(|&i| {
                let t = &self.transfers[i];
                let text = needle.is_empty()
                    || t.name.to_lowercase().contains(&needle)
                    || t.tags.iter().any(|tag| tag.contains(&needle));
                text && self.status_filter.is_none_or(|s| self.status(t) == s)
            })
            .collect();
        visible.sort_by(|&a, &b| {
            let (ta, tb) = (&self.transfers[a], &self.transfers[b]);
            let (ba, bb) = (self.bundle(ta), self.bundle(tb));
            let ord = match self.sort {
                SortKey::Added => a.cmp(&b),
                SortKey::Name => ta.name.to_lowercase().cmp(&tb.name.to_lowercase()),
                SortKey::Size => ba.size_bytes.cmp(&bb.size_bytes),
                SortKey::Progress => ba.progress.total_cmp(&bb.progress),
                SortKey::Status => self.status(ta).cmp(&self.status(tb)),
                SortKey::Down => self.down_bps(ta).total_cmp(&self.down_bps(tb)),
                SortKey::Up => self.up_bps(ta).total_cmp(&self.up_bps(tb)),
                SortKey::Peers => self.serving_us(ta).cmp(&self.serving_us(tb)),
                SortKey::Eta => eta_key(self, ta).cmp(&eta_key(self, tb)),
                SortKey::Ratio => ba.ratio().total_cmp(&bb.ratio()),
            };
            if self.descending { ord.reverse() } else { ord }
        });
        self.visible = visible;
        self.cursor = keep
            .and_then(|k| self.visible.iter().position(|&i| i == k))
            .unwrap_or(self.cursor)
            .min(self.visible.len().saturating_sub(1));
    }

    pub fn run(&mut self, command: Command) {
        match command {
            Command::Add => self.mode = Mode::Add(String::new()),
            Command::StartStop => self.start_stop(false),
            Command::ForceStart => self.start_stop(true),
            Command::Rename => {
                if let Some(t) = self.selected() {
                    self.mode = Mode::Rename(t.name.clone());
                }
            }
            Command::Tags => {
                if let Some(t) = self.selected() {
                    self.mode = Mode::Tags(t.tags.join(", "));
                }
            }
            Command::OpenFolder => {
                if let Some(t) = self.selected() {
                    let msg = format!("Would open {} in the file manager", t.save_path);
                    self.notify(msg);
                }
            }
            Command::Remove => {
                if self.selected().is_some() {
                    self.mode = Mode::ConfirmRemove;
                }
            }
            Command::Filter => self.mode = Mode::Filter,
            Command::NextSort => {
                let i = SortKey::ALL.iter().position(|k| *k == self.sort).unwrap_or(0);
                self.sort = SortKey::ALL[(i + 1) % SortKey::ALL.len()];
                self.refresh();
            }
            Command::ReverseSort => {
                self.descending = !self.descending;
                self.refresh();
            }
            Command::NextStatus => self.cycle_status(1),
            Command::NextTab => {
                let i = Tab::ALL.iter().position(|t| *t == self.tab).unwrap_or(0);
                self.tab = Tab::ALL[(i + 1) % Tab::ALL.len()];
            }
            Command::NextTheme => self.theme = self.theme.next(),
            Command::Help => self.mode = Mode::Help,
            Command::Quit => self.quit = true,
        }
    }

    pub fn move_cursor(&mut self, delta: isize) {
        let len = self.visible.len() as isize;
        if len > 0 {
            self.cursor = (self.cursor as isize + delta).clamp(0, len - 1) as usize;
        }
    }

    pub fn cycle_status(&mut self, delta: isize) {
        let mut options: Vec<Option<Status>> = vec![None];
        options.extend(Status::ALL.iter().copied().map(Some));
        let i = options.iter().position(|s| *s == self.status_filter).unwrap_or(0) as isize;
        let n = options.len() as isize;
        self.status_filter = options[((i + delta).rem_euclid(n)) as usize];
        self.cursor = 0;
        self.refresh();
    }

    pub fn set_status_filter(&mut self, status: Option<Status>) {
        self.status_filter = status;
        self.cursor = 0;
        self.refresh();
    }

    fn start_stop(&mut self, force: bool) {
        let Some(&i) = self.visible.get(self.cursor) else { return };
        let now = self.now();
        let live = self.worlds[self.transfers[i].world].bundles[self.transfers[i].index].clone();
        let t = &mut self.transfers[i];
        let msg = if t.error.is_some() {
            t.error = None;
            t.control = if force { Control::Forced } else { Control::Running };
            t.frozen = None;
            format!("Retrying {}", t.name)
        } else if force {
            t.control = Control::Forced;
            t.frozen = None;
            format!("Force-started {} (ignores the queue limit)", t.name)
        } else if matches!(t.control, Control::Stopped | Control::Queued) {
            t.control = Control::Running;
            t.frozen = None;
            format!("Started {}", t.name)
        } else {
            t.control = Control::Stopped;
            t.frozen = Some(freeze(live));
            format!("Stopped {}", t.name)
        };
        self.toast = Some((msg, now + 3000));
        self.refresh();
    }

    pub fn confirm_remove(&mut self, delete_files: bool) {
        if let Some(&i) = self.visible.get(self.cursor) {
            let t = self.transfers.remove(i);
            let msg = if delete_files {
                format!("Removed {} and deleted its files", t.name)
            } else {
                format!("Removed {} (files kept in {})", t.name, t.save_path)
            };
            self.notify(msg);
            self.visible.clear();
            self.refresh();
        }
        self.mode = Mode::Normal;
    }

    pub fn apply_rename(&mut self, name: String) {
        if let Some(&i) = self.visible.get(self.cursor) {
            if !name.trim().is_empty() {
                self.transfers[i].name = name.trim().to_string();
            }
        }
        self.mode = Mode::Normal;
        self.refresh();
    }

    pub fn apply_tags(&mut self, tags: String) {
        if let Some(&i) = self.visible.get(self.cursor) {
            self.transfers[i].tags = tags
                .split(',')
                .map(|t| t.trim().to_lowercase())
                .filter(|t| !t.is_empty())
                .collect();
        }
        self.mode = Mode::Normal;
        self.refresh();
    }

    pub fn apply_add(&mut self, link: String) {
        self.mode = Mode::Normal;
        if !link.trim().is_empty() {
            self.notify("This prototype can't add bundles yet; the link would be fetched here".into());
        }
    }

    pub fn notify(&mut self, msg: String) {
        let until = self.now() + 3500;
        self.toast = Some((msg, until));
    }

    /// Commands matching a palette query.
    pub fn palette_matches(query: &str) -> Vec<Command> {
        let q = query.to_lowercase();
        Command::ALL.iter().copied().filter(|c| q.is_empty() || c.label().contains(&q)).collect()
    }
}

fn eta_key(app: &App, t: &Transfer) -> u64 {
    match app.status(t) {
        Status::Downloading => app.bundle(t).eta_secs().unwrap_or(u64::MAX - 1),
        _ => u64::MAX,
    }
}

/// A stopped copy of a bundle: no rates, nothing in flight.
fn freeze(mut b: Bundle) -> Bundle {
    b.down_bps = 0.0;
    b.up_bps = 0.0;
    for cell in &mut b.cells {
        cell.in_flight = false;
    }
    for p in &mut b.peers {
        p.down_bps = 0.0;
        p.up_bps = 0.0;
    }
    b
}

/// Deterministic fake file names for a bundle's files tab.
pub fn fake_files(b: &Bundle, name: &str) -> Vec<(String, u64, f64)> {
    let n = (b.files as usize).clamp(1, 200);
    if n == 1 {
        return vec![(name.to_string(), b.size_bytes, b.progress)];
    }
    let per = b.size_bytes / n as u64;
    (0..n)
        .map(|i| {
            let p = ((b.progress * 1.9 - i as f64 / n as f64 * 0.9) + ((i * 37 % 11) as f64 / 40.0)).clamp(0.0, 1.0);
            let p = if b.progress >= 1.0 { 1.0 } else { p };
            (format!("{name}/part-{:04}.bin", i + 1), per, p)
        })
        .collect()
}

/// Short label for a transfer's rate column.
pub fn rate_or_dash(bps: f64) -> String {
    if bps < 1.0 { "·".to_string() } else { format::rate(bps) }
}

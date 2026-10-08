//! Where the user is: the areas, the views of Transfers, the detail's tabs and the screen they
//! make, and how the TUI was started and ends.

/// The app's areas (D-136), in navigation order. Transfers is home.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Area {
    /// The transfers list.
    Transfers,
    /// Creating a bundle and getting its link.
    Share,
    /// Limits, queue, categories and theme.
    Settings,
}

impl Area {
    /// Every area, in navigation order.
    pub const ALL: [Self; 3] = [Self::Transfers, Self::Share, Self::Settings];

    /// Its name in the navigation.
    pub fn title(self) -> &'static str {
        match self {
            Self::Transfers => "Transfers",
            Self::Share => "Share",
            Self::Settings => "Settings",
        }
    }

    pub(super) fn index(self) -> usize {
        Self::ALL.iter().position(|area| *area == self).unwrap_or(0)
    }

    pub(super) fn step(self, by: isize) -> Self {
        let count = Self::ALL.len() as isize;
        Self::ALL[(self.index() as isize + by).rem_euclid(count) as usize]
    }
}

/// How far into the selected transfer the Transfers area goes (D-137).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    /// The list, with the selected transfer's state below it.
    List,
    /// The list, with the selected transfer's detail below it.
    Detail,
    /// The selected transfer's chunk map, full screen.
    Chunks,
    /// The selected transfer's rates over time, full screen.
    Rates,
}

/// The detail's tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    /// Its state in plain language, and its numbers.
    Summary,
    /// Its files.
    Files,
    /// Its connections.
    Peers,
    /// Its decision log.
    Decisions,
}

impl Tab {
    /// Every tab, in order.
    pub const ALL: [Self; 4] = [Self::Summary, Self::Files, Self::Peers, Self::Decisions];

    /// Its name on the tab bar.
    pub fn title(self) -> &'static str {
        match self {
            Self::Summary => "Summary",
            Self::Files => "Files",
            Self::Peers => "Peers",
            Self::Decisions => "Decisions",
        }
    }

    pub(super) fn step(self, by: isize) -> Self {
        let index = Self::ALL.iter().position(|tab| *tab == self).unwrap_or(0);
        let count = Self::ALL.len() as isize;
        Self::ALL[(index as isize + by).rem_euclid(count) as usize]
    }
}

/// What has the keys: a level of the Transfers area, or another area.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    /// The transfers list.
    List,
    /// A transfer's detail, below the list.
    Detail,
    /// A transfer's chunk map.
    Chunks,
    /// A transfer's rates chart.
    Rates,
    /// The Share area.
    Share,
    /// The Settings area.
    Settings,
}

impl Screen {
    /// Every screen.
    pub const ALL: [Self; 6] = [
        Self::List,
        Self::Detail,
        Self::Chunks,
        Self::Rates,
        Self::Share,
        Self::Settings,
    ];
}

/// Where the engine the TUI talks to runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// A daemon that outlives the TUI.
    Daemon,
    /// An engine started by the TUI, which stops when it quits.
    Embedded,
}

/// What the engine should do once the TUI has quit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    /// Stop with the TUI.
    Stop,
    /// Carry on its active transfers in a daemon.
    HandOff,
}

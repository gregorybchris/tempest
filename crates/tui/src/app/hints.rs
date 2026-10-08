//! What the hint bar and help show: the commands that work on the screen shown (D-191).

use super::*;

impl App {
    /// Whether `command` would do anything now.
    pub fn works(&self, command: Command) -> bool {
        let screen = self.screen();
        if !command.works_in(screen) {
            return false;
        }
        let selected = self.selected.is_some();
        let share = self.selected_share().is_some();
        match command.section() {
            Section::Move => match screen {
                Screen::List => !self.visible().is_empty(),
                Screen::Share => share,
                _ => true,
            },
            Section::Transfer if screen == Screen::Share => share,
            Section::Transfer | Section::Queue => command == Command::Add || selected,
            Section::View => selected,
            Section::Find => command != Command::ClearFilters || self.finding.filtering(),
            Section::Share => command == Command::NewShare || share,
            Section::Change => self.settings.is_some(),
            Section::App => true,
        }
    }

    /// The keys that work right now, most useful first. The last always shows.
    pub fn hints(&self) -> Vec<Hint> {
        if let Some(modal) = &self.modal {
            return self.modal_hints(modal);
        }
        if self.screen() == Screen::Share && self.share.editing {
            return self.share_hints();
        }
        const ORDER: [Command; 25] = [
            Command::ClearFilters,
            Command::Back,
            Command::Down,
            Command::NextTab,
            Command::Detail,
            Command::Chunks,
            Command::Rates,
            Command::StartStop,
            Command::NewShare,
            Command::Change,
            Command::AddEntry,
            Command::RemoveEntry,
            Command::ShowShare,
            Command::Filter,
            Command::Add,
            Command::Palette,
            Command::Remove,
            Command::Sort,
            Command::GoTransfers,
            Command::Copy,
            Command::Views,
            Command::NextSection,
            Command::Theme,
            Command::Quit,
            Command::Help,
        ];
        let screen = self.screen();
        ORDER
            .into_iter()
            .filter(|command| self.works(*command))
            .filter_map(|command| command.hint(screen))
            .map(|(keys, action)| Hint { keys, action })
            .collect()
    }

    /// The current screen's commands by section, for the help overlay.
    pub fn help_sections(&self) -> Vec<(Section, Vec<Command>)> {
        let screen = self.screen();
        let mut sections: Vec<(Section, Vec<Command>)> = Vec::new();
        for command in Command::ALL {
            if !command.works_in(screen) {
                continue;
            }
            let section = command.section();
            match sections.last_mut() {
                Some((last, commands)) if *last == section => commands.push(command),
                _ => sections.push((section, vec![command])),
            }
        }
        sections
    }

    /// The current screen's name, as help titles it.
    pub fn screen_title(&self) -> &'static str {
        match self.screen() {
            Screen::List | Screen::Share | Screen::Settings => self.area.title(),
            Screen::Detail => "Detail",
            Screen::Chunks => "Chunk map",
            Screen::Rates => "Rates",
        }
    }
}

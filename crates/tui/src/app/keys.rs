//! What keys and commands do (D-191): each key is looked up in the binding table, and the
//! command it names changes the state or asks the engine for something.

use super::*;

impl App {
    /// Handles a key press, returning what to ask for, if anything.
    pub fn key(&mut self, key: KeyEvent) -> Option<Request> {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            // A second one, with the question open, stops everything.
            match self.modal {
                Some(Modal::Quit { .. }) => self.quit = true,
                _ => {
                    self.modal = None;
                    self.ask_to_quit();
                }
            }
            return None;
        }
        if let Some(modal) = self.modal.take() {
            return self.modal_key(modal, key);
        }
        // A visible notice is the topmost thing esc can close.
        if key.code == KeyCode::Esc && self.notice.take().is_some() {
            return None;
        }
        if self.screen() == Screen::Share && self.share.editing {
            return self.share_key(key);
        }
        self.run(Command::for_key(self.screen(), key)?)
    }

    /// Runs `command`, from its key or the palette.
    pub fn run(&mut self, command: Command) -> Option<Request> {
        if !command.works_in(self.screen()) {
            return None;
        }
        match command {
            Command::Down => self.move_by(1),
            Command::Up => self.move_by(-1),
            Command::First => self.move_by(isize::MIN),
            Command::Last => self.move_by(isize::MAX),
            Command::PageDown => self.move_by(PAGE),
            Command::PageUp => self.move_by(-PAGE),

            Command::Detail if self.selected.is_some() => {
                self.view = View::Detail;
                self.scroll = 0;
                return self.fetch();
            }
            Command::NextTab | Command::PreviousTab => {
                let by = if command == Command::NextTab { 1 } else { -1 };
                self.tab = self.tab.step(by);
                self.scroll = 0;
                return self.fetch();
            }
            Command::Chunks => {
                self.view = View::Chunks;
                return self.fetch();
            }
            Command::Rates => self.view = View::Rates,
            Command::Back => {
                self.view = match self.view {
                    View::List | View::Detail => View::List,
                    View::Chunks | View::Rates => View::Detail,
                };
                return self.fetch();
            }

            Command::Add => {
                self.modal = Some(Modal::Add(Box::default()));
                return Some(Request::Categories);
            }
            Command::Filter => {
                self.modal = Some(Modal::Filter(Input::new(&self.finding.text)));
            }
            Command::StatusFilter => {
                let selected = self.finding.status.map_or(0, |status| {
                    1 + StatusFilter::ALL
                        .iter()
                        .position(|s| *s == status)
                        .unwrap_or(0)
                });
                self.modal = Some(Modal::Pick {
                    kind: PickKind::Status,
                    selected,
                });
            }
            Command::Sort => {
                let key = self.finding.sort.key;
                let selected = SortKey::ALL.iter().position(|k| *k == key).unwrap_or(0);
                self.modal = Some(Modal::Pick {
                    kind: PickKind::Sort,
                    selected,
                });
            }
            Command::Reverse => {
                self.change_finding(|finding| finding.sort.reversed = !finding.sort.reversed);
            }
            Command::ClearFilters => self.change_finding(|finding| {
                finding.text.clear();
                finding.status = None;
            }),
            Command::Views => {
                let selected = self
                    .current_view()
                    .and_then(|name| self.views.iter().position(|v| v.name == name))
                    .map_or(0, |n| n + 1);
                self.modal = Some(Modal::Pick {
                    kind: PickKind::View,
                    selected,
                });
            }
            Command::SaveView => {
                let name = self.current_view().unwrap_or_default().to_owned();
                self.modal = Some(Modal::Value(Box::new(ValueEdit::new(
                    "Save this view".to_owned(),
                    ValueTarget::ViewName,
                    &name,
                    "a name, such as Music",
                ))));
            }
            Command::Columns => self.open_columns(),

            Command::NewShare => {
                self.share.editing = true;
                self.share.form.error = None;
            }
            Command::ShowShare => self.show_share(),
            Command::Copy if self.screen() == Screen::Share => {
                let share = self.selected_share()?;
                return Some(Request::Copy {
                    text: share.link.clone(),
                    what: format!("{}'s link", share.name),
                });
            }
            Command::Change => return self.change_row(),
            Command::AddEntry => return self.add_entry(),
            Command::RemoveEntry => return self.remove_row(),
            Command::NextSection => self.move_section(1),
            Command::PreviousSection => self.move_section(-1),

            Command::Palette => {
                self.modal = Some(Modal::Palette {
                    query: Input::default(),
                    selected: 0,
                });
            }
            Command::GoTransfers => return self.go(Area::Transfers),
            Command::GoShare => return self.go(Area::Share),
            Command::GoSettings => return self.go(Area::Settings),
            Command::NextArea => return self.go(self.area.step(1)),
            Command::PreviousArea => return self.go(self.area.step(-1)),
            Command::Theme => return self.next_theme(),
            Command::Help => self.modal = Some(Modal::Help { scroll: 0 }),
            Command::Quit => self.ask_to_quit(),

            transfer_command => return self.run_on_selected(transfer_command),
        }
        None
    }

    /// Runs a command that acts on the selected transfer.
    pub(super) fn run_on_selected(&mut self, command: Command) -> Option<Request> {
        let Some(transfer) = self.selected_transfer() else {
            self.notify("No transfer is selected.", true);
            return None;
        };
        let target = Target {
            id: transfer.id,
            name: transfer.name.clone(),
        };
        let text = |kind, text: &str| Modal::Text {
            kind,
            target: target.clone(),
            input: Input::new(text),
            error: None,
        };
        let action = match command {
            Command::StartStop => return self.start_or_stop(),
            Command::ForceStart => Action::ForceStart,
            Command::Recheck => Action::Recheck,
            Command::QueueUp => Action::Queue(QueueStep::Up),
            Command::QueueDown => Action::Queue(QueueStep::Down),
            Command::QueueTop => Action::Queue(QueueStep::Top),
            Command::QueueBottom => Action::Queue(QueueStep::Bottom),
            Command::Rename => {
                self.modal = Some(text(TextKind::Rename, &transfer.name));
                return None;
            }
            Command::Tags => {
                let tags: Vec<&str> = transfer.tags.iter().map(String::as_str).collect();
                self.modal = Some(text(TextKind::Tags, &tags.join(", ")));
                return None;
            }
            Command::MoveStorage => {
                let folder = transfer.save_folder.display().to_string();
                self.modal = Some(text(TextKind::Move, &folder));
                return None;
            }
            Command::Remove => {
                self.modal = Some(Modal::Remove {
                    target,
                    delete_files: false,
                });
                return None;
            }
            Command::Category => {
                let selected = transfer.category.as_ref().map_or(0, |current| {
                    self.categories
                        .iter()
                        .flatten()
                        .position(|(c, _)| c == current)
                        .map_or(0, |n| n + 1)
                });
                self.modal = Some(Modal::Pick {
                    kind: PickKind::Category(target),
                    selected,
                });
                return Some(Request::Categories);
            }
            Command::Copy => {
                return Some(Request::Copy {
                    text: transfer.link.clone(),
                    what: format!("{}'s link", transfer.name),
                });
            }
            Command::Unpublish => {
                let mut published = transfer.published.iter();
                self.modal = match (published.next(), published.next()) {
                    (None, _) => {
                        let text = format!("{} isn't published to any tracker.", target.name);
                        self.notify(text, false);
                        None
                    }
                    (Some(tracker), None) => Some(confirm_unpublish(target, tracker.clone())),
                    (Some(_), Some(_)) => Some(Modal::Pick {
                        kind: PickKind::Unpublish(target),
                        selected: 0,
                    }),
                };
                return None;
            }
            Command::Limits => {
                let transfer = transfer.clone();
                self.open_limits(&transfer);
                return None;
            }
            Command::Open => {
                let folder = transfer
                    .content_dir
                    .clone()
                    .unwrap_or_else(|| transfer.save_folder.clone());
                if self.local {
                    return Some(Request::Open(target.id, folder));
                }
                let text = format!(
                    "{} is in {} on the engine's machine.",
                    target.name,
                    folder.display()
                );
                self.notify(text, false);
                return None;
            }
            _ => return None,
        };
        Some(Request::Transfer(target, action))
    }

    /// Moves the cursor in the list, the shares or the rows, or scrolls the detail's tab.
    pub(super) fn move_by(&mut self, by: isize) {
        match self.screen() {
            Screen::Detail => {}
            Screen::Share => return self.move_share(by),
            Screen::Settings => return self.move_row(by),
            _ => return self.move_cursor(by),
        }
        // Decisions count from the newest, at the bottom, so down goes toward it.
        let by = if self.tab == Tab::Decisions { -by } else { by };
        let last = self.tab_rows().saturating_sub(1);
        self.scroll = self.scroll.saturating_add_signed(by).min(last);
    }

    /// How many rows the detail's tab has to scroll through.
    pub(super) fn tab_rows(&self) -> usize {
        let Some(id) = self.selected else { return 0 };
        let inside = self.inside();
        match self.tab {
            Tab::Summary => SUMMARY_ROWS,
            Tab::Files => inside.and_then(|i| loaded(&i.files)).map_or(0, Vec::len),
            Tab::Peers => inside.and_then(|i| loaded(&i.peers)).map_or(0, Vec::len),
            Tab::Decisions => self.model.decisions(&id).map_or(0, |d| d.events.len()),
        }
    }

    pub(super) fn move_cursor(&mut self, by: isize) {
        let visible = self.visible();
        let Some(last) = visible.len().checked_sub(1) else {
            return;
        };
        let target = match self.cursor() {
            Some(cursor) => cursor.saturating_add_signed(by).min(last),
            None => 0,
        };
        self.selected = Some(visible[target].id);
    }

    pub(super) fn start_or_stop(&mut self) -> Option<Request> {
        let transfer = self.selected_transfer()?;
        let target = Target {
            id: transfer.id,
            name: transfer.name.clone(),
        };
        let action = match transfer.status {
            Status::Stopped | Status::Completed | Status::Errored { .. } => Action::Start,
            Status::Moving => {
                let text = format!("{} can't stop while its files are moving.", transfer.name);
                self.notify(text, true);
                return None;
            }
            _ => Action::Stop,
        };
        Some(Request::Transfer(target, action))
    }
}

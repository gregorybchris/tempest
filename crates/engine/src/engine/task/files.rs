//! Moving, removing and deleting a transfer's files.

use super::*;

impl EngineTask {
    pub(super) fn remove(&mut self, id: BundleId, delete_files: bool, reply: Reply<()>) {
        if let Some(Transfer {
            live: Live::Moving { .. },
            ..
        }) = self.transfers.get(&id)
        {
            let _ = reply.send(Err(EngineError::Moving(id)));
            return;
        }
        let Some(t) = self.transfers.remove(&id) else {
            let _ = reply.send(Err(EngineError::NotFound(id)));
            return;
        };
        if let Err(error) = self.store.remove_transfer(&id) {
            self.transfers.insert(id, t);
            let _ = reply.send(Err(error.into()));
            return;
        }
        tracing::info!("{id}: removed");
        self.events.push(EngineEvent::Removed { id });
        let files = match (&t.row.content_dir, &t.manifest) {
            (Some(dir), Some(manifest)) if delete_files => Some(Files {
                dir: dir.clone(),
                manifest: manifest.clone(),
                own_dir: t.row.kind == Kind::Download,
            }),
            _ => None,
        };
        match t.live {
            Live::Running(run) | Live::Stopping(run) => {
                run.stop.send_replace(true);
                let burial = Burial {
                    generation: run.generation,
                    files,
                    reply,
                };
                self.burials.insert(id, burial);
            }
            Live::Opening { task, .. } => {
                task.abort();
                bury(files, reply);
            }
            Live::Idle | Live::Moving { .. } => bury(files, reply),
        }
    }

    /// Moves a transfer's files to `folder`: now if it is idle, or once it has stopped.
    pub(super) async fn move_storage(
        &mut self,
        id: BundleId,
        folder: PathBuf,
    ) -> Result<TransferInfo, EngineError> {
        let folder = checked_path(&folder)?;
        let t = self
            .transfers
            .get_mut(&id)
            .ok_or(EngineError::NotFound(id))?;
        if matches!(t.live, Live::Moving { .. }) || t.move_to.is_some() {
            return Err(EngineError::Moving(id));
        }
        t.move_to = Some(folder);
        if matches!(t.live, Live::Running(_) | Live::Opening { .. }) {
            self.halt(id);
        }
        self.after_stop(id).await;
        self.reschedule().await;
        self.emit_changes();
        Ok(self.info(&id).expect("the transfer exists"))
    }

    /// Starts moving an idle transfer's files to `folder`, once the move is recorded so that a
    /// crash during it can be undone.
    pub(super) fn start_move(&mut self, id: BundleId, folder: PathBuf) {
        let generation = self.generation();
        let t = self.transfers.get_mut(&id).expect("moving a transfer");
        let (Some(manifest), Some(dir)) = (&t.manifest, &t.row.content_dir) else {
            return;
        };
        let items = move_items(&t.row, manifest, dir, &folder, false);
        if items.iter().all(|item| item.from == item.to) {
            return;
        }
        tracing::info!("{id}: moving its files to {}", folder.display());
        if let Err(error) = self.store.begin_move(&id, &folder) {
            tracing::error!("couldn't record moving transfer {id}: {error}");
            t.row.error = Some(format!("Couldn't move the files: {error}"));
            return;
        }
        t.live = Live::Moving {
            generation,
            to: folder,
        };
        let internal = self.internal.clone();
        tokio::task::spawn_blocking(move || {
            let result = relocate::move_all(&items);
            let _ = internal.blocking_send(Internal::Moved {
                id,
                generation,
                result,
            });
        });
    }

    /// Records where a transfer's files ended up, and lets it run again.
    pub(super) async fn moved(
        &mut self,
        id: BundleId,
        generation: u64,
        result: std::io::Result<Vec<PathBuf>>,
    ) {
        let Some(t) = self.transfers.get_mut(&id) else {
            return;
        };
        let folder = match std::mem::replace(&mut t.live, Live::Idle) {
            Live::Moving { generation: g, to } if g == generation => to,
            other => {
                t.live = other;
                return;
            }
        };
        let manifest = t.manifest.clone().expect("a moved transfer has a manifest");
        let old_dir = t
            .row
            .content_dir
            .clone()
            .expect("a moved transfer has a content folder");
        let single = single_file_in_place(&t.row, &manifest);
        let new_dir = moved_dir(t.row.kind, &manifest, &old_dir, &folder, single);
        match result {
            Ok(originals) => {
                let old_folder = std::mem::replace(&mut t.row.save_folder, folder.clone());
                t.row.content_dir = Some(new_dir);
                match self.store.finish_move(&t.row) {
                    Ok(()) => {
                        tracing::info!("{id}: moved its files to {}", folder.display());
                        tokio::task::spawn_blocking(move || {
                            relocate::remove_originals(&originals);
                        });
                    }
                    // The next start finds the move unrecorded and undoes it, so the record
                    // and the files stay together.
                    Err(error) => {
                        tracing::error!("couldn't record moving transfer {id}: {error}");
                        t.row.save_folder = old_folder;
                        t.row.content_dir = Some(old_dir);
                        t.row.error = Some(format!(
                            "Moved the files to {}, but couldn't record it: {error}. They go \
                             back when the engine next starts.",
                            folder.display()
                        ));
                    }
                }
            }
            Err(error) => {
                tracing::warn!(
                    "{id}: moving its files to {} failed: {error}",
                    folder.display()
                );
                t.row.error = Some(format!(
                    "Couldn't move the files to {}: {error}. They are still in {}.",
                    folder.display(),
                    old_dir.display()
                ));
                if let Err(error) = self.store.end_move(&id) {
                    tracing::error!("couldn't forget transfer {id}'s move: {error}");
                }
                save_row(&mut self.store, &mut t.row);
            }
        }
        self.after_stop(id).await;
        self.reschedule().await;
    }
}

/// What moving a transfer's files from `dir` into `folder` moves. A download's own folder moves
/// whole; otherwise each top-level entry moves, since the folder may hold other things.
///
/// Entries are found under whatever Unicode normal form their names have on disk, and keep it.
/// With `undoing`, they are looked for at the new place, to put back a move a crash
/// interrupted.
pub(super) fn move_items(
    row: &TransferRow,
    manifest: &Manifest,
    dir: &Path,
    folder: &Path,
    undoing: bool,
) -> Vec<Item> {
    let single = single_file_in_place(row, manifest);
    let new_dir = moved_dir(row.kind, manifest, dir, folder, single);
    if let (Kind::Download, None) = (row.kind, single) {
        return vec![Item {
            from: dir.to_owned(),
            to: new_dir,
        }];
    }
    let tops: BTreeSet<&str> = (manifest.entries().iter())
        .filter_map(|entry| entry.path.split('/').next())
        .collect();
    let name = |path: &Path| path.file_name().unwrap_or_default().to_owned();
    tops.into_iter()
        .map(|top| match undoing {
            false => {
                let from = find_entry(dir, top);
                let to = new_dir.join(name(&from));
                Item { from, to }
            }
            true => {
                let to = find_entry(&new_dir, top);
                let from = dir.join(name(&to));
                Item { from, to }
            }
        })
        .collect()
}

/// The file of a transfer that is one file stored directly in its save folder (or shared from
/// its folder). A download kept in a folder of its own, as earlier versions made, is not.
pub(super) fn single_file_in_place<'a>(
    row: &TransferRow,
    manifest: &'a Manifest,
) -> Option<&'a str> {
    let single = manifest.single_file()?;
    (row.kind == Kind::Share || row.content_dir.as_ref() == Some(&row.save_folder))
        .then_some(single)
}

/// Where a transfer's content folder is once moved from `dir` into `folder`. A single file goes
/// straight into `folder`; anything else keeps its folder's name, as shares and downloads do alike.
pub(super) fn moved_dir(
    kind: Kind,
    manifest: &Manifest,
    dir: &Path,
    folder: &Path,
    single: Option<&str>,
) -> PathBuf {
    match (single, kind) {
        (Some(_), _) => folder.to_owned(),
        (None, Kind::Download) => folder.join(manifest.name()),
        (None, Kind::Share) => folder.join(dir.file_name().unwrap_or_default()),
    }
}

/// Deletes a removed transfer's files, if asked to, then answers the removal.
pub(super) fn bury(files: Option<Files>, reply: Reply<()>) {
    let Some(files) = files else {
        let _ = reply.send(Ok(()));
        return;
    };
    tokio::task::spawn_blocking(move || {
        let _ = reply.send(delete_files(&files));
    });
}

/// Deletes a bundle's files, symlinks and then directories, leaving anything else in them. A
/// file already gone is fine.
pub(super) fn delete_files(files: &Files) -> Result<(), EngineError> {
    use tempest_protocol::EntryKind;

    let fs_path = |path: &str| find_entry(&files.dir, path);
    let io_error = |path: &Path| {
        let path = path.to_owned();
        move |source| EngineError::Io { path, source }
    };
    let mut dirs = BTreeSet::new();
    for entry in files.manifest.entries() {
        let path = fs_path(&entry.path);
        for ancestor in tempest_protocol::path::ancestors(&entry.path) {
            dirs.insert(ancestor.to_owned());
        }
        match entry.kind {
            EntryKind::Directory => {
                dirs.insert(entry.path.clone());
            }
            EntryKind::File(_) | EntryKind::Symlink { .. } => match std::fs::remove_file(&path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                    return Err(io_error(&path)(e));
                }
                _ => {}
            },
        }
    }
    // Deepest first. A directory that still holds something else stays.
    let mut dirs: Vec<_> = dirs.into_iter().collect();
    dirs.sort_by_key(|dir| std::cmp::Reverse(dir.matches('/').count()));
    for dir in dirs {
        let _ = std::fs::remove_dir(fs_path(&dir));
    }
    if files.own_dir {
        let _ = std::fs::remove_dir(&files.dir);
    }
    Ok(())
}

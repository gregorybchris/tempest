//! A transfer's run: launching it, opening its storage, attaching its bundle, and stopping.

use super::*;

impl EngineTask {
    /// Starts the transfers the queue says should run, and stops those it says shouldn't.
    pub(super) async fn reschedule(&mut self) {
        if self.shutting_down.is_some() || self.hold {
            return;
        }
        let running = self.scheduled();
        let ids: Vec<_> = self.transfers.keys().copied().collect();
        for id in ids {
            let live = &self.transfers[&id].live;
            match (running.contains(&id), live) {
                (true, Live::Idle) => {
                    tracing::debug!("{id}: the queue starts it");
                    self.launch(id, false).await;
                }
                (false, Live::Running(_) | Live::Opening { recheck: false, .. }) => {
                    tracing::debug!("{id}: the queue stops it");
                    self.halt(id);
                }
                _ => {}
            }
        }
    }

    pub(super) fn generation(&mut self) -> u64 {
        self.next_generation += 1;
        self.next_generation
    }

    /// Starts a transfer: opens its storage if the manifest is known, or starts fetching it.
    /// With `recheck`, every file is hashed, ignoring the saved checkpoint, and the transfer
    /// runs afterwards only if the queue says it should. A share's files are only ever read, so
    /// a recheck that finds one changed fails it (D-244).
    pub(super) async fn launch(&mut self, id: BundleId, recheck: bool) {
        let generation = self.generation();
        let t = self.transfers.get_mut(&id).expect("launching a transfer");
        t.recheck = false;
        let Some(manifest) = t.manifest.clone() else {
            tracing::debug!("{id}: starting; fetching its manifest from the swarm");
            let session = match self.config.transfer(t.row.kind, &t.row.settings) {
                Ok(transfer) => transfer.session,
                Err(error) => return self.fail(id, error.to_string()),
            };
            let mut peers = load_peers(&self.store, &id);
            peers.extend(link_record(&t.link));
            let destination = Destination::NewFolderIn(t.row.save_folder.clone());
            let started = self
                .node
                .download_with(id, destination, peers, Some(session))
                .await;
            return match started {
                Ok(handle) => self.attach(id, generation, handle).await,
                Err(error) => self.fail(id, error.to_string()),
            };
        };
        let dir = t
            .row
            .content_dir
            .clone()
            .expect("a transfer with a manifest has a content folder");
        let checkpoint = match recheck {
            true => None,
            false => load_checkpoint(&self.store, &id),
        };
        let kind = t.row.kind;
        tracing::debug!(
            "{id}: starting; opening its {kind:?} storage in {} ({})",
            dir.display(),
            match (recheck, checkpoint.is_some()) {
                (true, _) => "checking every file",
                (false, true) => "resuming from a checkpoint",
                (false, false) => "no checkpoint, checking every file",
            }
        );
        let internal = self.internal.clone();
        let task = tokio::spawn(async move {
            let config = StorageConfig::default();
            let result = match (kind, checkpoint) {
                (Kind::Share, Some(c)) => Storage::resume_shared(dir, manifest, config, c).await,
                (Kind::Share, None) => Storage::open_shared(dir, manifest, config).await,
                (Kind::Download, Some(c)) => Storage::resume_download(dir, manifest, config, c)
                    .await
                    .map(|(storage, _)| storage),
                (Kind::Download, None) => Storage::create_download(dir, manifest, config)
                    .await
                    .map(|(storage, _)| storage),
            };
            let _ = internal
                .send(Internal::Opened {
                    id,
                    generation,
                    result,
                })
                .await;
        });
        t.live = Live::Opening {
            generation,
            recheck,
            task: task.abort_handle(),
        };
    }

    /// Whether the queue would run a transfer now.
    pub(super) fn should_run(&self, id: BundleId) -> bool {
        self.shutting_down.is_none() && !self.hold && self.scheduled().contains(&id)
    }

    /// The transfers the queue says should run.
    pub(super) fn scheduled(&self) -> std::collections::HashSet<BundleId> {
        let candidates: Vec<_> = self
            .transfers
            .values()
            .filter(|t| t.row.wanted && !t.row.done && t.row.error.is_none())
            .map(|t| Candidate {
                id: t.row.id,
                position: t.row.queue_position,
                complete: t.row.is_complete(),
                forced: t.row.forced,
            })
            .collect();
        schedule(&candidates, self.config.queue)
            .into_iter()
            .collect()
    }

    /// Runs a transfer whose storage just opened, or, after a recheck the queue doesn't want
    /// running, records what storage found and closes it.
    pub(super) async fn opened(&mut self, id: BundleId, generation: u64, storage: Storage) {
        let Some(t) = self.transfers.get_mut(&id) else {
            return;
        };
        if !matches!(t.live, Live::Opening { generation: g, .. } if g == generation) {
            return;
        }
        t.live = Live::Idle;
        // What storage found may differ from the record, if files changed while stopped.
        match storage.checkpoint().await {
            Ok(checkpoint) => {
                t.row.have = checkpoint.chunks_stored();
                if t.row.have < t.row.chunks {
                    t.row.completed = None;
                    t.row.done = false;
                    t.row.stopped_by = None;
                } else {
                    t.row.completed.get_or_insert_with(now_millis);
                }
                tracing::debug!(
                    "{id}: storage opened with {} of {} chunks",
                    t.row.have,
                    t.row.chunks
                );
                save_checkpoint(&mut self.store, &id, &checkpoint);
                save_row(&mut self.store, &mut t.row);
            }
            Err(error) => {
                tracing::warn!("{id}: cannot read its storage: {error}");
                t.row.error = Some(error.to_string());
                save_row(&mut self.store, &mut t.row);
            }
        }
        if !self.should_run(id) {
            tracing::debug!("{id}: checked; the queue doesn't want it running");
            let _ = storage.close().await;
            return;
        }
        let t = self.transfers.get_mut(&id).expect("checked above");
        let session = match self.config.transfer(t.row.kind, &t.row.settings) {
            Ok(transfer) => transfer.session,
            Err(error) => return self.fail(id, error.to_string()),
        };
        match self.node.share_with(storage, Some(session)).await {
            Ok(handle) => {
                let peers: Vec<_> = load_peers(&self.store, &id)
                    .into_iter()
                    .chain(link_record(&t.link))
                    .collect();
                if !peers.is_empty() {
                    handle.add_peers(peers).await;
                }
                self.attach(id, generation, handle).await;
            }
            Err(error) => self.fail(id, error.to_string()),
        }
    }

    /// Starts discovery for a running bundle and hands it to a supervisor.
    pub(super) async fn attach(&mut self, id: BundleId, generation: u64, handle: BundleHandle) {
        let t = self.transfers.get_mut(&id).expect("attaching a transfer");
        let mut trackers = t.link.trackers.clone();
        for tracker in self.config.trackers.iter().chain(&t.row.publish) {
            if !trackers.contains(tracker) {
                trackers.push(tracker.clone());
            }
        }
        // It depends on the bundle alone, so one signature serves every tracker and announce.
        let listing = self.identity.listing(&id);
        tracing::info!(
            "{id}: running, announcing to {} trackers ({} listing)",
            trackers.len(),
            t.row.publish.len()
        );
        let announcers = trackers
            .into_iter()
            .map(|url| {
                let listed = t.row.publish.contains(&url).then_some(listing);
                let config = self.announce.clone();
                Announcer::spawn(self.tracker.clone(), url, config, &handle, listed)
            })
            .collect();
        if let Some(lan) = &self.lan {
            lan.watch(&handle).await;
        }
        let limits = handle.limits().clone();
        let sharing = match self.config.transfer(t.row.kind, &t.row.settings) {
            Ok(transfer) => {
                limits.download.set_rate(transfer.limits.download);
                limits.upload.set_rate(transfer.limits.upload);
                transfer.sharing
            }
            Err(error) => {
                tracing::warn!("{id}: its limits don't apply, running without: {error}");
                SharingLimits::NONE
            }
        };
        let (stop, stop_receiver) = watch::channel(false);
        let (checkpoint, checkpoint_receiver) = mpsc::channel(1);
        let (listing_changes, listing_receiver) = mpsc::unbounded_channel();
        let now = Instant::now();
        t.live = Live::Running(Run {
            generation,
            status: handle.status.clone(),
            stop,
            checkpoint,
            listing: listing_changes,
            base_uploaded: t.row.uploaded,
            base_downloaded: t.row.downloaded,
            last_progress: now,
            progress_mark: (0, 0, t.manifest.is_some()),
            sample: (now, 0, 0),
            download_rate: 0,
            upload_rate: 0,
            last_checkpoint: now,
            last_tick: now,
            limits,
            sharing,
            inspector: handle.inspector(),
        });
        let supervisor = Supervisor {
            id,
            generation,
            handle,
            announcers,
            tracker: self.tracker.clone(),
            announce: self.announce.clone(),
            listing,
            internal: self.internal.clone(),
        };
        tokio::spawn(supervisor.run(stop_receiver, checkpoint_receiver, listing_receiver));
    }

    /// Checks a transfer's data from scratch: now if it is idle, or once it has stopped.
    pub(super) async fn recheck(&mut self, id: BundleId) -> Result<TransferInfo, EngineError> {
        let t = self
            .transfers
            .get_mut(&id)
            .ok_or(EngineError::NotFound(id))?;
        if matches!(t.live, Live::Moving { .. }) || t.move_to.is_some() {
            return Err(EngineError::Moving(id));
        }
        if t.manifest.is_none() {
            return Err(EngineError::NoData(id));
        }
        self.store.clear_checkpoint(&id)?;
        tracing::info!("{id}: recheck requested");
        t.recheck = true;
        // A failure that sent the user here may be fixed by what the check finds.
        t.row.error = None;
        match t.live {
            Live::Running(_) | Live::Opening { .. } => self.halt(id),
            Live::Stopping(_) | Live::Idle | Live::Moving { .. } => {}
        }
        self.after_stop(id).await;
        self.emit_changes();
        Ok(self.info(&id).expect("the transfer exists"))
    }

    /// Applies each running transfer's own speed and sharing limits, after settings change.
    pub(super) fn refresh_runs(&mut self) {
        for t in self.transfers.values_mut() {
            let (Live::Running(run) | Live::Stopping(run)) = &mut t.live else {
                continue;
            };
            if let Ok(transfer) = self.config.transfer(t.row.kind, &t.row.settings) {
                run.limits.download.set_rate(transfer.limits.download);
                run.limits.upload.set_rate(transfer.limits.upload);
                run.sharing = transfer.sharing;
            }
        }
    }

    /// Records that a transfer failed to start.
    pub(super) fn fail(&mut self, id: BundleId, reason: String) {
        let Some(t) = self.transfers.get_mut(&id) else {
            return;
        };
        tracing::warn!("{id}: failed to start: {reason}");
        t.live = Live::Idle;
        t.row.error = Some(reason);
        save_row(&mut self.store, &mut t.row);
    }

    /// Asks a running transfer to stop, or stops a storage still opening.
    pub(super) fn halt(&mut self, id: BundleId) {
        let t = self.transfers.get_mut(&id).expect("halting a transfer");
        t.live = match std::mem::replace(&mut t.live, Live::Idle) {
            Live::Running(run) => {
                tracing::debug!("{id}: stopping");
                run.stop.send_replace(true);
                if learn_manifest(&mut t.manifest, &mut t.row, &run.status.borrow()) {
                    save_row(&mut self.store, &mut t.row);
                }
                Live::Stopping(run)
            }
            Live::Opening { task, .. } => {
                task.abort();
                Live::Idle
            }
            Live::Idle => Live::Idle,
            unchanged @ (Live::Stopping(_) | Live::Moving { .. }) => unchanged,
        };
    }

    /// Records how a bundle ended.
    pub(super) async fn finished(
        &mut self,
        id: BundleId,
        generation: u64,
        result: Result<Summary, DriverError>,
    ) {
        if self
            .burials
            .get(&id)
            .is_some_and(|burial| burial.generation == generation)
        {
            let burial = self.burials.remove(&id).expect("checked above");
            bury(burial.files, burial.reply);
            return;
        }
        let Some(t) = self.transfers.get_mut(&id) else {
            return;
        };
        let (run, asked) = match std::mem::replace(&mut t.live, Live::Idle) {
            Live::Running(run) if run.generation == generation => (run, false),
            Live::Stopping(run) if run.generation == generation => (run, true),
            other => {
                t.live = other;
                return;
            }
        };
        // The manifest may have arrived since the last tick.
        learn_manifest(&mut t.manifest, &mut t.row, &run.status.borrow());
        let mut removed = false;
        match result {
            Ok(summary) => {
                t.row.uploaded = run.base_uploaded + summary.uploaded;
                t.row.downloaded = run.base_downloaded + summary.downloaded;
                if let Some(checkpoint) = summary.checkpoint.filter(|_| !t.recheck) {
                    t.row.have = checkpoint.chunks_stored();
                    save_checkpoint(&mut self.store, &id, &checkpoint);
                }
                tracing::debug!(
                    "{id}: stopped; complete {}, {} bytes down and {} up this run, asked {asked}",
                    summary.complete,
                    summary.downloaded,
                    summary.uploaded
                );
                if summary.complete {
                    if t.row.completed.is_none() {
                        tracing::info!("{id}: complete");
                    }
                    t.row.completed.get_or_insert_with(now_millis);
                    // It stopped sharing by itself: the idle timeout passed (D-065), and the
                    // action for a sharing limit applies (D-224).
                    if !asked {
                        t.row.done = true;
                        t.row.stopped_by = run.sharing.idle_stop();
                    }
                    removed = !asked && run.sharing.action == SharingAction::Remove;
                }
            }
            Err(error) => {
                tracing::warn!("{id}: stopped with an error: {error}");
                t.row.error = Some(error.to_string());
            }
        }
        save_row(&mut self.store, &mut t.row);
        if removed {
            let (reply, _) = oneshot::channel();
            self.remove(id, false, reply);
        } else {
            self.after_stop(id).await;
        }
        self.reschedule().await;
    }

    /// Starts what waited for a transfer to stop: a move, then a recheck.
    pub(super) async fn after_stop(&mut self, id: BundleId) {
        let Some(t) = self.transfers.get_mut(&id) else {
            return;
        };
        if !matches!(t.live, Live::Idle) {
            return;
        }
        if let Some(folder) = t.move_to.take() {
            match t.manifest {
                Some(_) => return self.start_move(id, folder),
                // Nothing is on disk yet: the download starts again in the new folder.
                None => {
                    t.row.save_folder = folder;
                    save_row(&mut self.store, &mut t.row);
                }
            }
        }
        if t.recheck {
            self.launch(id, true).await;
        }
    }

    /// Stops or removes a transfer that reached the sharing limit `stop`.
    pub(super) fn sharing_limit_reached(
        &mut self,
        id: BundleId,
        action: SharingAction,
        stop: SharingStop,
    ) {
        tracing::info!("{id}: reached its sharing limit {stop:?}; action {action:?}");
        match action {
            SharingAction::Stop => {
                let t = self.transfers.get_mut(&id).expect("a running transfer");
                // Stopped by a rule, not by the user: completed (D-180).
                t.row.done = true;
                t.row.stopped_by = Some(stop);
                save_row(&mut self.store, &mut t.row);
            }
            SharingAction::Remove => {
                let (reply, _) = oneshot::channel();
                self.remove(id, false, reply);
            }
        }
    }
}

/// Records a download's manifest once its status shows it. Returns whether it was new.
pub(super) fn learn_manifest(
    known: &mut Option<Arc<Manifest>>,
    row: &mut TransferRow,
    status: &tempest_net::Status,
) -> bool {
    let (None, Some(manifest)) = (&known, &status.manifest) else {
        return false;
    };
    row.manifest = Some(manifest.to_bytes());
    row.chunks = u64::from(ChunkLayout::new(manifest).chunk_count());
    row.content_dir = Some(match manifest.single_file() {
        Some(_) => row.save_folder.clone(),
        None => row.save_folder.join(manifest.name()),
    });
    *known = Some(manifest.clone());
    true
}

/// The peer a link names, if it gives an address to reach it at.
pub(super) fn link_record(link: &Link) -> Option<PeerRecord> {
    let peer_id = link.peer?;
    (!link.addrs.is_empty() || link.relay.is_some()).then(|| PeerRecord {
        peer_id,
        addresses: link.addrs.clone(),
        relay: link.relay,
    })
}

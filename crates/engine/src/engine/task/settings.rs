//! Settings: showing, changing and applying them.

use super::*;

impl EngineTask {
    pub(super) fn settings_view(&self) -> SettingsView {
        SettingsView {
            settings: self.config.clone(),
            changed: self.changes.clone(),
            restart_required: (self.config.restart_required(&self.running).iter())
                .map(|group| (*group).to_owned())
                .collect(),
            alternative_limits_active: self.config.limits.alternative_active(local_now()),
        }
    }

    pub(super) async fn change_settings(
        &mut self,
        changes: BTreeMap<String, Value>,
    ) -> Result<SettingsView, EngineError> {
        if let Some(key) = changes.keys().find(|key| key.as_str() == "data_dir") {
            return Err(EngineError::Fixed(key.clone()));
        }
        let mut all = self.changes.clone();
        all.extend(changes.clone());
        self.apply_settings(all).await?;
        for (key, value) in &changes {
            self.store.set_setting(key, value)?;
        }
        tracing::info!(
            "settings changed: {}",
            changes.keys().cloned().collect::<Vec<_>>().join(", ")
        );
        Ok(self.settings_view())
    }

    pub(super) async fn reset_setting(&mut self, key: String) -> Result<SettingsView, EngineError> {
        let mut all = self.changes.clone();
        if all.remove(&key).is_some() {
            self.apply_settings(all).await?;
            self.store.remove_setting(&key)?;
            tracing::info!("setting reset: {key}");
        }
        Ok(self.settings_view())
    }

    /// Makes `changes` over the config file the settings in effect, if they are valid.
    pub(super) async fn apply_settings(
        &mut self,
        changes: BTreeMap<String, Value>,
    ) -> Result<(), EngineError> {
        let config = self.base.with_changes(&changes)?;
        config.validate()?;
        self.save_folder = config.save_folder()?;
        self.config = config;
        self.changes = changes;
        // New queue and speed limits, relays and port mapping apply now, new trackers to shares'
        // links.
        self.apply_speed_limits();
        self.apply_relays();
        self.apply_port_mapping();
        self.refresh_runs();
        self.reschedule().await;
        self.emit_changes();
        Ok(())
    }

    /// Picks a home relay from the configured relays. A held engine runs no transfers, so
    /// nobody needs to reach it.
    pub(super) fn apply_relays(&self) {
        if !self.hold {
            let endpoint = self.node.endpoint();
            endpoint.set_relays(self.config.relays.choice());
        }
    }

    /// Asks the router to forward the peer port, or stops. A held engine needs no port.
    pub(super) fn apply_port_mapping(&self) {
        if !self.hold {
            let endpoint = self.node.endpoint();
            endpoint.set_port_mapping(self.config.port_mapping.config());
        }
    }

    /// Applies the speed limits over every transfer in effect now: the usual ones, or the
    /// alternative ones while switched on or scheduled.
    pub(super) fn apply_speed_limits(&self) {
        let limits = self.config.limits.in_effect(local_now());
        let node = self.node.limits();
        node.download.set_rate(limits.download);
        node.upload.set_rate(limits.upload);
    }
}

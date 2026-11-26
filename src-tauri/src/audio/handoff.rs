use super::*;

// `filter` is always the audible/last verified child. Before commit `other` is
// the candidate; after commit it is the retired child, never a fallback route.
pub(super) struct Handoff {
    other: Option<ActiveFilter>,
    phase: Phase,
}

#[derive(Clone, Copy)]
enum Phase {
    Staging { restore_old: bool },
    Retiring,
    Stopping,
}

impl Handoff {
    pub(super) fn staging(restore_old: bool) -> Self {
        Self {
            other: None,
            phase: Phase::Staging { restore_old },
        }
    }
}

impl AudioController {
    pub(super) fn filter_slot(&mut self, other: bool) -> &mut Option<ActiveFilter> {
        if other {
            &mut self.handoff.as_mut().unwrap().other
        } else {
            &mut self.filter
        }
    }

    pub(super) async fn verify_prepared_filter(
        &mut self,
        staged: bool,
        record: &mut RestoreRecord,
        expected_effective: &Option<String>,
        expected_configured: &Option<String>,
    ) -> AppResult<()> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(6);
        loop {
            let filter = self.filter_slot(staged).as_mut().unwrap();
            if filter.process.has_exited()? {
                return Err(AppError::new(
                    ErrorCode::AudioUnavailable,
                    "Il filtro audio non si e' avviato. Verificare i moduli PipeWire.",
                )
                .detail(filter.process.stderr()));
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(AppError::new(
                    ErrorCode::Timeout,
                    "Nodi, porte e link stereo del filtro non osservati in tempo.",
                )
                .detail(filter.process.stderr()));
            }
            self.read_registry().await?;
            let (effective, configured, _) = self.registry.defaults();
            if effective != *expected_effective || configured != *expected_configured {
                return Err(Self::target_mismatch());
            }
            if verify_graph(&self.registry, record)? {
                record.virtual_object_serial =
                    self.registry.node(&record.virtual_node_name)?.object_serial;
                record.playback_object_serial = self
                    .registry
                    .node(&record.playback_node_name)?
                    .object_serial;
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    pub(super) fn prepared_level_matches(
        &self,
        record: &RestoreRecord,
        volume: f64,
        mute: bool,
    ) -> bool {
        self.registry
            .node(&record.virtual_node_name)
            .is_ok_and(|node| {
                node.object_serial == record.virtual_object_serial
                    && node.muted == Some(mute)
                    && node
                        .volume
                        .is_some_and(|actual| (actual - volume).abs() <= 0.002)
            })
    }

    pub(super) async fn prepare_virtual_level(
        &mut self,
        record: &RestoreRecord,
        volume: f64,
        mute: bool,
    ) -> AppResult<()> {
        // Write to the verified virtual ID/serial, not a new name lookup that
        // could resolve to a replacement object between observation and write.
        let node = self.registry.node(&record.virtual_node_name)?;
        self.mutate_level(&node, Some(volume), Some(mute)).await?;
        self.read_registry().await?;
        if !self.prepared_level_matches(record, volume, mute) {
            return Err(AppError::new(
                ErrorCode::NotConfirmed,
                "Livello e mute non confermati sul nuovo filtro.",
            ));
        }
        Ok(())
    }

    pub(super) fn current_filter_level(&self) -> AppResult<(f64, bool)> {
        let old = &self.filter.as_ref().unwrap().record;
        let node = self.registry.node(&old.virtual_node_name)?;
        node.volume
            .zip(node.muted)
            .filter(|_| node.object_serial == old.virtual_object_serial)
            .ok_or_else(|| {
                AppError::new(
                    ErrorCode::NotConfirmed,
                    "Livello del filtro precedente non verificabile.",
                )
            })
    }

    pub(super) fn is_filter_default(&self, record: &RestoreRecord) -> bool {
        let (effective, configured, _) = self.registry.defaults();
        let name = Some(record.virtual_node_name.as_str());
        effective.as_deref() == name && configured.as_deref() == name
    }

    pub(super) fn commit_handoff(&mut self) {
        let handoff = self.handoff.as_mut().unwrap();
        std::mem::swap(&mut self.filter, &mut handoff.other);
        handoff.phase = Phase::Retiring;
    }

    pub(super) async fn recover_handoff(&mut self) -> AppResult<()> {
        let staged = surround::handoff_paths(&self.paths);
        let handoff = self.handoff.as_ref().unwrap();
        let phase = handoff.phase;
        if matches!(phase, Phase::Stopping) {
            return self.stop_handoff().await;
        }
        let current = self.filter.as_ref().unwrap().record.clone();
        if let Some(other) = handoff.other.as_ref().map(|f| f.record.clone()) {
            if let Phase::Staging { restore_old } = phase {
                self.rollback_handoff(&current, &other, restore_old).await?;
            }
            // Retiring a verified handoff never writes default metadata.
            self.terminate_filter(true).await?;
            if matches!(phase, Phase::Retiring) {
                surround::promote_handoff(&self.paths, &other, &current)?;
            } else {
                surround::clear_restore(&staged, &other.session_id)?;
            }
        } else {
            // A spawn failure can leave its pre-spawn journal. Do not touch the
            // working filter or signal a persisted PID while reconciling this slot.
            self.recover_journal(&staged).await?;
        }
        self.handoff = None;
        self.recovery_pending = false;
        self.selected_output = Some(current.selected_physical_target);
        self.read_registry().await?;
        Ok(())
    }

    async fn rollback_handoff(
        &mut self,
        old: &RestoreRecord,
        candidate: &RestoreRecord,
        restore_old: bool,
    ) -> AppResult<()> {
        self.read_registry().await?;
        let (effective, configured, _) = self.registry.defaults();
        let new_name = Some(candidate.virtual_node_name.as_str());
        if configured.as_deref() != new_name {
            return Ok(()); // unpublished or superseded; no metadata write
        }
        // WirePlumber may not yet have converged on the configured write. No
        // other effective name is ours to replace, even with our configured name.
        let original_effective = if restore_old {
            Some(&old.virtual_node_name)
        } else {
            candidate.previous_effective_default.as_ref()
        };
        if effective.as_deref() != new_name && effective.as_ref() != original_effective {
            return Err(Self::target_mismatch());
        }
        if restore_old {
            self.write_default_observed(
                &old.virtual_node_name,
                Some(&configured),
                Some(&effective),
                Some(old),
            )
            .await
        } else if let Some(name) = &candidate.previous_configured_default {
            // The old child was pinned but not the observed default of this profile.
            self.write_default_observed(name, Some(&configured), Some(&effective), None)
                .await
        } else {
            self.mutate_default(None, configured.as_deref()).await?;
            self.read_registry().await?;
            if self.registry.defaults().1 == configured {
                return Err(Self::target_mismatch());
            }
            Ok(())
        }
    }

    pub(super) async fn stop_handoff(&mut self) -> AppResult<()> {
        // On explicit shutdown (including disconnect), stop both owned clients
        // before exposing any fallback. If a child can't be reaped, keep both
        // handles and journals; never choose a physical route speculatively.
        self.handoff.as_mut().unwrap().phase = Phase::Stopping;
        self.recovery_pending = true;
        self.state = SurroundState::Stopping;
        let first = self.terminate_filter(true).await;
        let second = self.terminate_filter(false).await;
        first?;
        second?;
        self.handoff = None;
        self.filter = None;
        self.recover_stale().await
    }
}

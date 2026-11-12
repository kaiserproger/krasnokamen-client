//! Replay uses the same resources, packet dispatch, tick and scene owners as play.
use super::*;
use sha2::{Digest, Sha256};

impl Session {
    pub(crate) fn set_recording(&mut self, options: Option<crate::replay::format::RecordingOptions>) {
        self.recording = options;
        self.recording_tick = None;
    }
    pub(crate) fn recording_ready(&self, required_entities: &BTreeSet<String>) -> bool {
        self.phase == "play" && self.player.pose_known && self.recording_tick.is_some()
            && required_entities.iter().all(|uuid| self.entities.uuid_entities.contains_key(uuid))
    }
    pub(crate) fn recording_tick(&self) -> Option<u64> { self.recording_tick }
    pub(crate) fn finish_recording(&mut self,take_complete:bool) -> Result<()> {
        self.network.take().ok_or("recorder network is not active")?.finish_recording(take_complete)
    }
    #[cfg(test)]
    pub(crate) fn replay_fixture() -> Self {
        let mut session = Self::new_mode("replay-test".into(), false);
        session.replaying = true;
        session.phase = "login".into();
        session.content = Some(Content::test_cube());
        session
    }
    pub(super) fn activate_resources(&mut self, loaded: Loaded) -> Result<()> {
        eprintln!("session startup: activating content and UI");
        self.entities.activate_models(loaded.models.clone());
        self.player.set_visual_models(Arc::clone(&loaded.models));
        let mut ui = if self.presentation { UiState::new(&loaded.content)? } else { UiState::new_state(&loaded.content)? };
        self.entities.set_text_renderer(ui.text_renderer());
        ui.set_entity_models(loaded.models);
        if let Some((width, height, scale)) = self.ui_viewport { ui.resize(width, height, scale); }
        self.resource_epoch += 1;
        if self.presentation { self.install_atlas(&loaded.content); }
        self.content = Some(loaded.content);
        ui.singleplayer_active = self.local_server.is_some();
        self.ui = Some(ui);
        // A render must not depend on settings on the render-farm account.
        if !self.replaying {
            self.apply_persisted_settings(crate::local_config::load_client_settings());
            self.check_language_override()?;
        }
        self.network = loaded.network;
        if let Some(network) = self.network.as_ref() {
            network.set_view_distance(self.render_distance.clamp(UiState::MIN_RENDER_DISTANCE, UiState::MAX_RENDER_DISTANCE) as u8);
            network.set_main_arm_right(self.main_arm_right);
            network.set_language(&self.language);
        }
        if self.presentation && self.terrain.is_none() { self.terrain = Some(Terrain::new(self.wake_callback())?); }
        self.phase = "status".into();
        Ok(())
    }
    pub(crate) fn from_recording(presentation: bool) -> Result<Self> {
        // No audio device, sockets, auth, title screen, or persisted user options.
        let mut session = Self::new_mode("replay".into(), false);
        session.replaying = true;
        session.presentation = presentation;
        session.camera_mode = 1;
        session.scene = Scene::default();
        let content = if presentation { Content::load()? } else { Content::load_state()? };
        let models = Arc::new(if presentation { crate::entities::Models::load(&content)? } else { crate::entities::Models::load_state(&content)? });
        session.activate_resources(Loaded { content, models, network: None })?;
        session.phase = "login".into();
        session.apply_client_setting("hide_hud", &json!(true))?;
        Ok(session)
    }
    pub(crate) fn receive_packet(&mut self, phase: Phase, id: i32, body: &[u8]) -> Result<()> {
        if self.recording.is_some() && let Some((tick,_))=crate::replay::format::tick_marker(phase,id,body)? {
            self.recording_tick = Some(tick);
        }
        self.packet_sequence += 1;
        *self.packet_counts.entry(id).or_default() += 1;
        self.apply(phase, id, body).map_err(|error| format!("{phase:?} packet 0x{id:02x} sequence {}: {error}", self.packet_sequence))?;
        self.changed_state();
        Ok(())
    }
    pub(crate) fn replay_error(&self) -> Result<()> {
        self.error.as_ref().map_or(Ok(()), |error| Err(format!("replay session: {error}")))
    }
    pub(crate) fn replay_camera(&mut self, path: &crate::capture::camera::CameraPath, time: f64, partial: f32) -> Result<()> {
        let attached = path.attach_entity.map(|id| {
            let entity = self.entities.get(id).ok_or_else(|| format!("camera entity {id} is not present at {time}"))?;
            Ok::<_, String>(self.entities.presented_transform(entity, self.world_tick, partial)?.position)
        }).transpose()?;
        self.cinematic_camera = Some(path.sample(time, attached)?);
        self.camera();
        Ok(())
    }
    pub(crate) fn replay_pose(&self) -> Option<crate::capture::camera::Pose> { self.cinematic_camera }
    /// Finish exactly the mesh work selected by the current camera, with the
    /// virtual clock stopped. Live play never enters the blocking hook.
    pub(crate) fn settle_replay_geometry(&mut self) -> Result<()> {
        if !self.replaying { return Err("blocking replay geometry requested during live play".into()); }
        if !self.presentation || self.phase != "play" { return Ok(()); }
        if !self.play_world_presentation_ready() { return Err("replay camera range starts before received spawn terrain/light is ready".into()); }
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            self.refresh_scene()?;
            let terrain = self.terrain.as_mut().ok_or("replay terrain unavailable")?;
            let timeout = deadline.checked_duration_since(Instant::now()).ok_or("replay terrain completion timed out")?;
            if !terrain.wait_replay_completion(self.replaying, timeout)? { break; }
        }
        self.refresh_scene_mode(true)?;
        // Cinematic output is world-only, not UI or first-person overlays.
        self.scene.overlay = Default::default();
        // Mesh upload latency must not turn a cut-in into a terrain fade-in.
        for chunk in self.scene.chunks.values_mut() { chunk.fade_duration_ms = None; }
        Ok(())
    }
    /// CPU-side presentation fingerprint, not a promise of cross-GPU bit identity.
    pub(crate) fn replay_state_hash(&self) -> [u8;32] {
        let mut hash = Sha256::new();
        hash.update(self.phase.as_bytes());
        for value in [self.client_tick, self.world_tick, self.packet_sequence] { hash.update(value.to_be_bytes()); }
        for value in self.player.position { hash.update(value.to_bits().to_be_bytes()); }
        hash.update(self.player.yaw.to_bits().to_be_bytes());
        hash.update(self.player.pitch.to_bits().to_be_bytes());
        for value in [self.player.health,self.player.saturation] {
            hash.update([u8::from(value.is_some())]);
            hash.update(value.unwrap_or(0.).to_bits().to_be_bytes());
        }
        hash.update([u8::from(self.player.food.is_some())]);
        hash.update(self.player.food.unwrap_or(0).to_be_bytes());
        for value in self.scene.camera_position { hash.update(value.to_bits().to_be_bytes()); }
        hash.update(self.scene.yaw.to_bits().to_be_bytes());
        hash.update(self.scene.pitch.to_bits().to_be_bytes());
        hash.update(self.scene.fov_deg.to_bits().to_be_bytes());
        // Slot insertion order is deterministic, unlike the ID lookup map.
        for entity in self.entities.received_entities() {
            hash.update(entity.handle.network_id.to_be_bytes());
            hash.update(entity.type_name.as_bytes());
            let pose = entity.presented_transform(self.world_tick, self.presentation_partial);
            for value in pose.position { hash.update(value.to_bits().to_be_bytes()); }
            hash.update(pose.yaw.to_bits().to_be_bytes());
            hash.update(pose.pitch.to_bits().to_be_bytes());
            hash.update(entity.revision.to_be_bytes());
        }
        hash.update(bytemuck::cast_slice(&self.scene.dynamic.vertices));
        for indices in &self.scene.dynamic.indices { hash.update(bytemuck::cast_slice(indices)); }
        hash.update(self.scene.crowd.bones.to_be_bytes());
        hash.update(bytemuck::cast_slice(&self.scene.crowd.instances));
        hash.update(bytemuck::cast_slice(&self.scene.crowd.counts));
        hash.finalize().into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn live_session_cannot_take_blocking_replay_hook() {
        let mut session = Session::new_mode("live-no-block".into(), false);
        assert!(session.settle_replay_geometry().unwrap_err().contains("live play"));
        let mut terrain = Terrain::new(Arc::new(|| {})).unwrap();
        assert!(terrain.wait_replay_completion(false, Duration::ZERO).unwrap_err().contains("forbidden"));
    }
    #[test]
    fn recorder_ready_requires_every_current_uuid_and_session_prerequisite() {
        let mut session = Session::new_mode("recorder-roster".into(), false);
        let required = BTreeSet::from(["first".into(),"second".into()]);
        session.phase = "play".into();
        session.player.pose_known = true;
        session.recording_tick = Some(9000);
        session.entities.uuid_entities.insert("unrelated".into(),BTreeSet::from([1]));
        session.entities.uuid_entities.insert("first".into(),BTreeSet::from([2]));
        assert!(!session.recording_ready(&required), "a count is not an exact UUID barrier");
        session.entities.uuid_entities.insert("second".into(),BTreeSet::from([3]));
        assert!(session.recording_ready(&required));
        session.entities.uuid_entities.remove("first");
        assert!(!session.recording_ready(&required), "a despawned UUID no longer satisfies readiness");
        session.entities.uuid_entities.insert("first".into(),BTreeSet::from([4]));
        session.recording_tick = None;
        assert!(!session.recording_ready(&required));
        session.recording_tick = Some(9000);
        session.player.pose_known = false;
        assert!(!session.recording_ready(&required));
        session.player.pose_known = true;
        session.phase = "configuration".into();
        assert!(!session.recording_ready(&required));
    }
}

//! Replay uses the same resources, packet dispatch, tick and scene owners as play.
use super::*;
use sha2::{Digest, Sha256};
use crate::replay::SeekPhase;

pub(super) struct SeekState {
    phase: SeekPhase,
    atlas_tick: u64,
}

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
        if !self.replaying { self.apply_persisted_settings(crate::local_config::load_client_settings()); }
        self.network = loaded.network;
        if let Some(network) = self.network.as_ref() {
            network.set_view_distance(self.render_distance.clamp(UiState::MIN_RENDER_DISTANCE, UiState::MAX_RENDER_DISTANCE) as u8);
            network.set_main_arm_right(self.main_arm_right);
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
        session.replay_render_distance(None)?;
        let content = if presentation { Content::load()? } else { Content::load_state()? };
        let models = Arc::new(if presentation { crate::entities::Models::load(&content)? } else { crate::entities::Models::load_state(&content)? });
        session.activate_resources(Loaded { content, models, network: None })?;
        session.phase = "login".into();
        session.apply_client_setting("hide_hud", &json!(true))?;
        Ok(session)
    }
    /// Reuse the normal radius bounds/world budget without writing the render
    /// host's persisted settings. With no override, follow the recorded server.
    pub(crate) fn replay_render_distance(&mut self, requested: Option<u32>) -> Result<()> {
        if !self.replaying { return Err("replay radius requested during live play".into()); }
        let bounds = UiState::MIN_RENDER_DISTANCE..=UiState::MAX_RENDER_DISTANCE;
        if requested.is_some_and(|distance| !bounds.contains(&distance)) {
            return Err(format!("replay render-distance must be {}..{}", bounds.start(), bounds.end()));
        }
        let distance = requested.unwrap_or_else(|| self.server_view_distance
            .filter(|radius| *radius > 0).unwrap_or(UiState::DEFAULT_RENDER_DISTANCE)
            .clamp(UiState::MIN_RENDER_DISTANCE, UiState::MAX_RENDER_DISTANCE));
        if self.render_distance != distance || self.scene.render_distance != distance {
            self.render_distance = distance;
            self.world.set_view_distance(distance);
            self.scene.render_distance = distance;
        }
        Ok(())
    }
    /// Vanilla «Entity Distance» of a replay render (`--entity-distance`), not persisted.
    pub(crate) fn replay_entity_distance(&mut self, scaling: f64) -> Result<()> {
        use crate::entities::view_range::{MAX_SCALING, MIN_SCALING};
        if !self.replaying { return Err("replay entity distance requested during live play".into()); }
        if !(MIN_SCALING..=MAX_SCALING).contains(&scaling) {
            return Err(format!("replay entity-distance must be {MIN_SCALING}..{MAX_SCALING}"));
        }
        self.entity_distance_scaling = scaling;
        Ok(())
    }
    pub(crate) fn replay_seek_phase(&mut self, phase: Option<SeekPhase>) -> Result<()> {
        if !self.replaying { return Err("replay seek requested during live play".into()); }
        if phase.is_some() { self.entities.forget_frames(); }
        self.replay_seek = phase.map(|phase| SeekState {
            phase,
            atlas_tick: self.replay_seek.as_ref().map_or(self.world_tick, |state| state.atlas_tick),
        });
        Ok(())
    }
    /// Packet state is authoritative during a cut. Only the bounded warm-up
    /// runs the normal entity presentation owner; never predict recorder input.
    pub(super) fn tick_replay_seek(&mut self, world_advances: bool) -> Result<()> {
        let Some(SeekState { phase: SeekPhase::Warmup(ticks), .. }) = &mut self.replay_seek else { return Ok(()); };
        if !world_advances { return Ok(()); }
        *ticks += 1;
        let content = self.content.as_ref().ok_or("content unavailable")?;
        self.entities.set_look_target(
            (self.player.pose_known && self.player.game_mode != 3).then(|| self.player.eye_position()));
        let start = Instant::now();
        self.entities.tick_presentation(self.world_tick, false, &self.world,
            content, self.environment.game_time())?;
        self.tick_detail_ms[6] += start.elapsed().as_secs_f64() * 1000.;
        let start = Instant::now();
        self.entities.tick_block_sources(&self.world, content,
            self.environment.game_time(), &[self.player.position], false)?;
        self.tick_detail_ms[7] += start.elapsed().as_secs_f64() * 1000.;
        Ok(())
    }
    pub(crate) fn finish_replay_seek(&mut self) -> Result<u32> {
        let state = self.replay_seek.take().ok_or("replay seek has no warm-up phase")?;
        let SeekPhase::Warmup(ticks) = state.phase else {
            return Err("replay seek has no warm-up phase".into());
        };
        if self.phase != "play" || !self.player.pose_known { return Ok(ticks); }
        self.inventory.experience_level = self.player.experience_level;
        if let Some(ui) = &mut self.ui {
            ui.synchronize(&self.inventory, self.content.as_ref().ok_or("content unavailable")?)?;
            ui.tick(self.client_tick, 0.);
        }
        self.consume_player_effects()?;
        self.flush_commands()?;
        self.animate_atlas(state.atlas_tick)?;
        self.revision += 1;
        self.refresh_scene()?;
        Ok(ticks)
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
    /// virtual clock stopped. Live play never enters the blocking hook. False:
    /// CPU mesh residency is full; upload this scene (an uncaptured frame),
    /// release its acknowledgements and settle again.
    pub(crate) fn settle_replay_geometry(&mut self) -> Result<bool> {
        if !self.replaying { return Err("blocking replay geometry requested during live play".into()); }
        if !self.presentation || self.phase != "play" { return Ok(true); }
        if !self.play_world_presentation_ready() { return Err("replay camera range starts before received spawn terrain/light is ready".into()); }
        let deadline = Instant::now() + Duration::from_secs(120);
        let settled = loop {
            self.refresh_scene()?;
            let terrain = self.terrain.as_mut().ok_or("replay terrain unavailable")?;
            let timeout = deadline.checked_duration_since(Instant::now()).ok_or("replay terrain completion timed out")?;
            match terrain.wait_replay_completion(self.replaying, timeout)? {
                crate::terrain::ReplayWait::Completion => {}
                crate::terrain::ReplayWait::Upload => break false,
                crate::terrain::ReplayWait::Settled => break true,
            }
        };
        self.refresh_scene_mode(true)?;
        // Cinematic output is world-only, not UI or first-person overlays.
        self.scene.overlay = Default::default();
        // Zero admits the uploaded mesh immediately; None is an unknown policy
        // that the renderer must reject, not a request to disable fading.
        for chunk in self.scene.chunks.values_mut() { chunk.fade_duration_ms = Some(0); }
        Ok(settled)
    }
    pub(crate) fn replay_terrain_metrics(&self) -> Value {
        self.terrain.as_ref().map_or(Value::Null, Terrain::metrics)
    }
    #[cfg(test)]
    pub(crate) fn replay_seek_clock_state(&self) -> [u64; 3] {
        [self.client_tick, self.world_tick, self.packet_sequence]
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

    fn moving_entity_session() -> Session {
        let mut session = super::super::mining_start_tests::session();
        session.replaying = true;
        session.player.game_mode = 3;
        let models = Arc::get_mut(session.entities.models.as_mut().unwrap()).unwrap();
        models.schemas.insert("minecraft:pig".into(), serde_json::from_value(json!({
            "protocol_id":1,"width":1.0,"height":1.0,"eye_height":0.8,
            "fire_immune":false,"class":"authored.fixture","dimension_constants":{},
            "enum_ids":{},"enum_numbers":{},"hierarchy":["net.minecraft.world.entity.LivingEntity"],
            "method_owners":{},"accessors":{},
            "dimensions":{"width":1.0,"height":1.0,"eye_height":0.8,"fixed":false,"attachments":{}},
            "default_attributes":{"minecraft:scale":{"id":1,"value":1.0}}
        })).unwrap());
        models.motion.insert("minecraft:pig".into(), serde_json::from_value(json!({
            "class":"authored.fixture","is_mob":true,"walk_uses_y":"never","methods":{},
            "body_rule":"standard","walk_rule":"standard",
            "head_rule":{"kind":"fixed","value":75.0},"body_head_rule":{"kind":"fixed","value":75.0}
        })).unwrap());
        session.content.as_mut().unwrap().entity_names.insert(1,"minecraft:pig".into());
        let mut spawn = Writer::default();
        spawn.var(41);spawn.0.extend([0;16]);spawn.var(1);
        for value in [4.5,4.,4.5] {spawn.f64(value);}
        spawn.0.extend([0;5]); // zero LP velocity, pitch/yaw/head, spawn data
        session.receive_packet(Phase::Play,crate::protocol::packet_ids::play::clientbound::ADD_ENTITY,&spawn.0).unwrap();
        session
    }

    #[test]
    fn cut_seek_preserves_received_state_and_warms_normal_entity_owner() {
        let run = |seek: bool| {
            let mut session = moving_entity_session();
            if seek { session.replay_seek_phase(Some(SeekPhase::Skip)).unwrap(); }
            for tick in 1..=400 {
                if seek && tick == 321 {
                    session.replay_seek_phase(Some(SeekPhase::Warmup(0))).unwrap();
                }
                let mut movement = Writer::default();
                movement.var(41);movement.var(1);
                movement.0.extend(16i16.to_be_bytes());
                movement.0.extend([0;4]);
                session.receive_packet(Phase::Play,
                    crate::protocol::packet_ids::play::clientbound::MOVE_ENTITY_POS,&movement.0).unwrap();
                session.tick_after_events(true);
                session.replay_error().unwrap();
            }
            if seek { assert_eq!(session.finish_replay_seek().unwrap(),80); }
            session
        };
        let full = run(false);
        let cut = run(true);
        assert_eq!(cut.client_tick,full.client_tick);
        assert_eq!(cut.world_tick,full.world_tick);
        assert_eq!(cut.packet_sequence,full.packet_sequence);
        let expected = full.entities.get(41).unwrap();
        let actual = cut.entities.get(41).unwrap();
        assert_eq!(actual.position,expected.position);
        assert_eq!(actual.revision,expected.revision);
        assert_eq!(actual.on_ground,expected.on_ground);
        assert_eq!(actual.type_name,expected.type_name);
        for (actual,expected) in actual.presented_transform(cut.world_tick,0.5).position
            .into_iter().zip(expected.presented_transform(full.world_tick,0.5).position) {
            assert!((actual-expected).abs()<1e-12,"warm interpolation must converge");
        }
        let a = actual.source.as_ref().unwrap();
        let b = expected.source.as_ref().unwrap();
        assert_eq!(a.age,80,"only the bounded warm-up runs presentation");
        assert_eq!(b.age,400);
        let a = a.living.as_ref().unwrap();
        let b = b.living.as_ref().unwrap();
        assert!((a.body(0.5)-b.body(0.5)).abs()<1e-4);
        assert!((a.walk.speed(0.5)-b.walk.speed(0.5)).abs()<1e-6);
        assert!(cut.replay_seek.is_none());
    }

    #[test]
    fn live_ticks_still_predict_present_entities_and_refresh_scene() {
        let mut session = moving_entity_session();
        session.replaying = false;
        let _peer = super::super::mining_start_tests::connect(&mut session);
        assert!(session.replay_seek_phase(Some(SeekPhase::Skip)).unwrap_err().contains("live play"));
        let pose_revision = session.player.pose_revision;
        session.tick_after_events(true);
        assert!(session.error.is_none(),"{:?}",session.error);
        assert!(session.player.pose_revision>pose_revision,"normal player.tick must run");
        assert_eq!(session.entities.presentation_tick,session.world_tick);
        assert_eq!(session.entities.get(41).unwrap().source.as_ref().unwrap().age,1);
        assert!(session.tick_detail_ms[10]>0.,"normal refresh_scene must run");
    }
}

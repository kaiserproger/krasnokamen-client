//! Packet replay, in receive order, on an explicitly stepped owner-thread clock.
pub(crate) mod clock;
pub mod format;
pub(crate) mod record;
mod roster;
pub(crate) mod scan;
#[cfg(test)]
mod seek_tests;
#[cfg(test)]
use crate::protocol::packet_ids::{configuration, login, play};
use crate::{protocol::network::Phase, session::Session};
use clock::VirtualClock;
use format::{Packet, RecordingReader};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::BufReader,
    path::{Path, PathBuf},
    time::Duration,
};

/// Wall time preserves the recording's receive cadence; tick time plays server
/// marker groups at 20 Hz, including gaps, independent of the server's TPS.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Timebase {
    #[default]
    Wall,
    Tick,
}
impl std::str::FromStr for Timebase {
    type Err = String;
    fn from_str(value: &str) -> Result<Self, String> {
        match value {
            "wall" => Ok(Self::Wall),
            "tick" => Ok(Self::Tick),
            _ => Err("replay timebase must be wall or tick".into()),
        }
    }
}

// Four seconds of normal entity presentation before a shot starts. The fast
// prefix never becomes a captured frame; seeks are cut boundaries, not playback.
pub(crate) const SEEK_WARMUP_TICKS: u32 = 80;
#[derive(Clone, Copy)]
pub(crate) enum SeekPhase {
    Skip,
    Warmup(u32),
}

pub struct Player {
    pub(crate) session: Session,
    input: RecordingReader<BufReader<File>>,
    next: Option<Packet>,
    path: PathBuf,
    presentation: bool,
    phase: Phase,
    time: Duration,
    next_tick: Duration,
    last_packet: Duration,
    pub last_tick: Option<u64>,
    timebase: Timebase,
    tick_origin: Option<u64>,
    render_distance: Option<u32>,
    pub(crate) seek_warmup_ticks: u32,
    pub last_wall_ns: Option<u64>,
    transcript: Sha256,
    crowd_instancing: bool,
    crowd_mesh_fingerprint: std::cell::RefCell<Option<(u64, [u8; 32])>>,
    // Drop the session's worker owners before restoring the enclosing clock.
    clock: VirtualClock,
}
impl Player {
    pub fn open(path: &Path, presentation: bool, timebase: Timebase) -> Result<Self, String> {
        let clock = VirtualClock::new();
        let mut input = RecordingReader::open(path)?;
        let next = input.next_packet()?;
        if next.is_none() {
            return Err("replay recording contains no packets".into());
        }
        let session = Session::from_recording(presentation)?;
        Ok(Self {
            session,
            input,
            next,
            path: path.into(),
            presentation,
            phase: Phase::Login,
            time: Duration::ZERO,
            next_tick: Duration::ZERO,
            last_packet: Duration::ZERO,
            last_tick: None,
            timebase,
            tick_origin: None,
            render_distance: None,
            seek_warmup_ticks: 0,
            last_wall_ns: None,
            transcript: Sha256::new(),
            crowd_instancing: false,
            crowd_mesh_fingerprint: Default::default(),
            clock,
        })
    }
    pub(crate) fn set_crowd_instancing(&mut self, enabled: bool) {
        self.crowd_instancing = enabled;
        self.session.set_crowd_instancing(enabled);
    }
    pub(crate) fn set_render_distance(&mut self, distance: Option<u32>) -> Result<(), String> {
        self.session.replay_render_distance(distance)?;
        self.render_distance = distance;
        Ok(())
    }
    /// Cut-boundary seek: preserve every packet/world tick, discard uncaptured
    /// presentation history, then warm the normal entity owner before capture.
    /// During a shot use advance_to/step instead, so animation history continues.
    pub fn seek(&mut self, time: Duration) -> Result<(), String> {
        if time < self.time {
            self.clock.set(Duration::ZERO);
            let profiling = self.session.take_scene_profile().is_some();
            self.session = Session::from_recording(self.presentation)?;
            self.session.enable_scene_profile(profiling);
            self.session.set_crowd_instancing(self.crowd_instancing);
            self.session.replay_render_distance(self.render_distance)?;
            *self.crowd_mesh_fingerprint.borrow_mut() = None;
            self.input = RecordingReader::open(&self.path)?;
            self.next = self.input.next_packet()?;
            self.phase = Phase::Login;
            self.time = Duration::ZERO;
            self.next_tick = Duration::ZERO;
            self.last_packet = Duration::ZERO;
            self.last_tick = None;
            self.tick_origin = None;
            self.last_wall_ns = None;
            self.transcript = Sha256::new();
        }
        let warmup = self.tick_interval().saturating_mul(SEEK_WARMUP_TICKS);
        let result = (|| {
            if time.saturating_sub(self.time) > warmup {
                self.session.replay_seek_phase(Some(SeekPhase::Skip))?;
                self.advance_to(time - warmup)?;
            }
            self.session.replay_seek_phase(Some(SeekPhase::Warmup(0)))?;
            self.advance_to(time)?;
            self.seek_warmup_ticks = self.session.finish_replay_seek()?;
            Ok(())
        })();
        // A failed range or packet must not leave subsequent playback in seek.
        self.session.replay_seek_phase(None)?;
        result
    }
    pub fn step(&mut self, frame: u64) -> Result<(), String> {
        self.advance_to(clock::frame_time(frame, 60))
    }
    pub fn advance_to(&mut self, target: Duration) -> Result<(), String> {
        if target < self.time {
            return Err("replay clock reversal requires seek".into());
        }
        if target.as_millis() > u128::from(u32::MAX) {
            return Err("replay time exceeds solrec u32 millisecond range".into());
        }
        loop {
            if self.next.is_none() && target > self.last_packet {
                return Err(format!(
                    "replay range exceeds last packet at {:.3}s",
                    self.last_packet.as_secs_f64()
                ));
            }
            let packet_time = self.packet_time()?;
            let tick_due = self.next_tick <= target;
            if let Some(time) =
                packet_time.filter(|time| *time <= target && (!tick_due || *time <= self.next_tick))
            {
                self.clock.set(time);
                self.time = time;
                let packet = self.next.take().unwrap();
                self.apply_packet(&packet)?;
                self.last_packet = time;
                self.next = self.input.next_packet()?;
            } else if tick_due {
                self.clock.set(self.next_tick);
                self.time = self.next_tick;
                self.session.tick_after_events(true);
                self.session.replay_error()?;
                self.next_tick += self.tick_interval();
            } else {
                break;
            }
        }
        self.clock.set(target);
        self.time = target;
        Ok(())
    }
    fn tick_interval(&self) -> Duration {
        match self.timebase {
            Timebase::Wall => self.session.tick_interval(),
            Timebase::Tick => Duration::from_millis(50),
        }
    }
    fn packet_time(&mut self) -> Result<Option<Duration>, String> {
        let Some(packet) = self.next.as_ref() else {
            return Ok(None);
        };
        if self.timebase == Timebase::Wall {
            return Ok(Some(Duration::from_millis(u64::from(packet.ms))));
        }
        let (id, body) = packet.split()?;
        let marker = format::tick_marker(self.phase, id, body)?.map(|(tick, _)| tick);
        if let Some(tick) = marker {
            if self.last_tick.is_some_and(|previous| tick <= previous) {
                return Err("replay krasnokamen:tick markers are not strictly increasing".into());
            }
            self.tick_origin.get_or_insert(tick);
        }
        // Configuration before the first actual marker is time zero. The
        // header's start_tick may be an unfinalized recorder estimate.
        let elapsed = match (marker.or(self.last_tick), self.tick_origin) {
            (Some(tick), Some(origin)) => tick
                .checked_sub(origin)
                .and_then(|ticks| ticks.checked_mul(50))
                .ok_or("replay tick timeline overflow")?,
            _ => 0,
        };
        if elapsed > u64::from(u32::MAX) {
            return Err("replay time exceeds solrec u32 millisecond range".into());
        }
        Ok(Some(Duration::from_millis(elapsed)))
    }
    fn apply_packet(&mut self, packet: &Packet) -> Result<(), String> {
        let (id, body) = packet.split()?;
        // Tick numbers are server-owned labels, never inferred by incrementing
        // a local counter: skipped edge-pump ticks remain visible as gaps.
        if let Some((tick, wall)) = format::tick_marker(self.phase, id, body)? {
            if self.last_tick.is_some_and(|previous| tick <= previous) {
                return Err("replay krasnokamen:tick markers are not strictly increasing".into());
            }
            self.last_tick = Some(tick);
            self.last_wall_ns = Some(wall);
        }
        self.session.receive_packet(self.phase, id, body)?;
        self.session.replay_render_distance(self.render_distance)?;
        self.transcript.update(packet.ms.to_be_bytes());
        self.transcript
            .update((packet.bytes.len() as u32).to_be_bytes());
        self.transcript.update(&packet.bytes);
        self.phase = format::phase_after(self.phase, id);
        Ok(())
    }
    pub(crate) fn partial(&self) -> f32 {
        let interval = self.tick_interval();
        (1. - self.next_tick.saturating_sub(self.time).as_secs_f32() / interval.as_secs_f32())
            .clamp(0., 1.)
    }
    pub fn frame_hash(&self) -> String {
        let mut hash = self.transcript.clone();
        hash.update(self.time.as_nanos().to_be_bytes());
        hash.update(self.session.replay_state_hash());
        // A process-global mesh revision is an invalidation key, not replay state.
        let meshes = &self.session.scene().crowd.meshes;
        let mut cached = self.crowd_mesh_fingerprint.borrow_mut();
        if cached
            .as_ref()
            .is_none_or(|(revision, _)| *revision != meshes.revision)
        {
            *cached = Some((meshes.revision, crowd_mesh_hash(meshes)));
        }
        hash.update(cached.as_ref().unwrap().1);
        format!("{:x}", hash.finalize())
    }
}

fn crowd_mesh_hash(meshes: &crate::entities::CrowdMeshes) -> [u8; 32] {
    let mut hash = Sha256::new();
    for bytes in [
        bytemuck::cast_slice(meshes.vertices.as_slice()),
        bytemuck::cast_slice(meshes.indices.as_slice()),
        bytemuck::cast_slice(meshes.bones.as_slice()),
    ] {
        hash.update((bytes.len() as u64).to_be_bytes());
        hash.update(bytes);
    }
    hash.update((meshes.groups.len() as u64).to_be_bytes());
    for group in &meshes.groups {
        hash.update(group.base_vertex.to_be_bytes());
        hash.update(bytemuck::bytes_of(&group.first_index));
        hash.update(bytemuck::bytes_of(&group.counts));
        hash.update(group.bone_first.to_be_bytes());
        hash.update(group.bone_count.to_be_bytes());
        hash.update(bytemuck::bytes_of(&group.hip));
        hash.update(group.radius.to_bits().to_be_bytes());
    }
    hash.finalize().into()
}

#[cfg(test)]
mod tests;

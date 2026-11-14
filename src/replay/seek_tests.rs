//! Opt-in measurement on a real take. No recording, assets or fixture pixels
//! are embedded here. The full-presentation reference suppresses local input
//! prediction/scene extraction just like a cut, but ticks every entity owner.
use super::*;
use crate::{
    capture::camera::CameraPath,
    render::{GraphicsPreset, Renderer},
};
use serde_json::json;
use std::{collections::BTreeMap, time::Instant};

// Candidate warm-up windows in client ticks; 0 is the unwarmed cut.
const WINDOWS: [u32; 7] = [0, 3, 8, 20, 40, 80, 160];

struct EntityState {
    received: [u8; 32],
    position: [f64; 3],
    body: Option<[f32; 2]>,
    walk_speed: Option<[f32; 2]>,
    passenger: bool,
}
struct Snapshot {
    entities: BTreeMap<i32, EntityState>,
    clocks: [u64; 3],
    transcript: [u8; 32],
    tick: Option<u64>,
    wall: Option<u64>,
    next_tick: Duration,
}
fn snapshot(replay: &Player) -> Snapshot {
    let mut entities = BTreeMap::new();
    for entity in replay.session.entities.received_entities() {
        // Only received fields: source age/gait phase are intentionally allowed
        // to restart at a cut. Never compare them as authoritative packet state.
        let received = format!(
            "{:?}",
            (
                (
                    &entity.uuid,
                    &entity.type_name,
                    entity.position,
                    entity.velocity,
                    entity.on_ground,
                    entity.spawn_data,
                    entity.observed_tick,
                    entity.revision
                ),
                (
                    &entity.metadata,
                    &entity.equipment,
                    &entity.effects,
                    &entity.attributes
                ),
                (
                    &entity.passenger_ids,
                    entity.vehicle,
                    entity.leash_holder_id,
                    entity.animation,
                    entity.damage_tick,
                    entity.death_tick,
                    entity.hurt_direction
                ),
            )
        );
        let living = entity
            .source
            .as_ref()
            .and_then(|source| source.living.as_ref());
        entities.insert(
            entity.handle.network_id,
            EntityState {
                received: Sha256::digest(received.as_bytes()).into(),
                position: replay
                    .session
                    .entities
                    .presented_transform(
                        entity,
                        replay.session.replay_seek_clock_state()[1],
                        replay.partial(),
                    )
                    .unwrap()
                    .position,
                body: living.map(|state| [state.body(0.), state.body(1.)]),
                walk_speed: living.map(|state| [state.walk.speed(0.), state.walk.speed(1.)]),
                passenger: entity.vehicle.is_some(),
            },
        );
    }
    Snapshot {
        entities,
        clocks: replay.session.replay_seek_clock_state(),
        transcript: replay.transcript.clone().finalize().into(),
        tick: replay.last_tick,
        wall: replay.last_wall_ns,
        next_tick: replay.next_tick,
    }
}
/// Body yaw accumulates without wrapping; 360 degrees apart draws identically.
fn yaw_delta(a: f32, b: f32) -> f64 {
    f64::from((a - b + 180.).rem_euclid(360.) - 180.).abs()
}
fn distribution(mut deltas: Vec<f64>, visible: f64) -> serde_json::Value {
    deltas.sort_by(f64::total_cmp);
    let quantile = |q: f64| {
        deltas
            .get(((deltas.len() as f64 * q).ceil() as usize).saturating_sub(1))
            .copied()
            .unwrap_or(0.)
    };
    json!({"samples":deltas.len(),"max":quantile(1.),"p99":quantile(0.99),"p50":quantile(0.5),
        "threshold":visible,"over_threshold":deltas.iter().filter(|delta| **delta > visible).count()})
}
fn compare(reference: &Snapshot, actual: &Snapshot) -> serde_json::Value {
    assert_eq!(reference.clocks, actual.clocks);
    assert_eq!(reference.transcript, actual.transcript);
    assert_eq!(reference.tick, actual.tick);
    assert_eq!(reference.wall, actual.wall);
    assert_eq!(reference.next_tick, actual.next_tick);
    assert_eq!(reference.entities.len(), actual.entities.len());
    let (mut position, mut seat, mut body, mut walk) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for (id, a) in &actual.entities {
        let b = &reference.entities[id];
        assert_eq!(a.received, b.received, "received state changed for {id}");
        let distance = a
            .position
            .into_iter()
            .zip(b.position)
            .map(|(a, b)| (a - b).abs())
            .fold(0f64, f64::max);
        position.push(distance);
        if a.passenger {
            seat.push(distance);
        }
        if let (Some(a), Some(b)) = (a.body, b.body) {
            body.push(
                a.into_iter()
                    .zip(b)
                    .map(|(a, b)| yaw_delta(a, b))
                    .fold(0f64, f64::max),
            );
        }
        if let (Some(a), Some(b)) = (a.walk_speed, b.walk_speed) {
            walk.push(
                a.into_iter()
                    .zip(b)
                    .map(|(a, b)| f64::from((a - b).abs()))
                    .fold(0f64, f64::max),
            );
        }
    }
    json!({"entities":actual.entities.len(),"received_state_exact":true,"packet_and_clock_state_exact":true,
        "presented_position_blocks":distribution(position,1e-3),"seat_position_blocks":distribution(seat,1e-3),
        "body_yaw_degrees":distribution(body,0.5),"walk_speed":distribution(walk,1e-2)})
}
fn render_cut(replay: &mut Player, camera: &CameraPath, directory: &Path, label: &str) -> Vec<u8> {
    let partial = replay.partial();
    replay
        .session
        .replay_camera(camera, replay.time.as_secs_f64(), partial)
        .unwrap();
    replay.session.prepare_frame(partial).unwrap();
    let mut renderer =
        pollster::block_on(Renderer::new_offscreen(480, 270, directory.into())).unwrap();
    assert!(renderer.crowd_supported());
    renderer.set_graphics_preset(GraphicsPreset::Medium);
    let readback = renderer.replay_readback().unwrap();
    let mut pixels = Vec::new();
    let upload_frames =
        crate::capture::settle_terrain(replay, &mut renderer, |renderer, scene, observation| {
            renderer.replay_rgba(scene, observation, &readback, &mut pixels)
        })
        .unwrap();
    renderer
        .replay_rgba(
            replay.session.scene(),
            &replay.session.execution_observation(),
            &readback,
            &mut pixels,
        )
        .unwrap();
    std::fs::write(directory.join(format!("{label}-480x270.rgba")), &pixels).unwrap();
    std::fs::write(directory.join(format!("{label}.frame.json")), serde_json::to_vec_pretty(&json!({
        "cpu_state_sha256":replay.frame_hash(),"rgba_sha256":format!("{:x}",Sha256::digest(&pixels)),
        "last_applied_tick":replay.last_tick,"replay_time":replay.time.as_secs_f64(),
        "presentation":replay.session.scene().entity_presentation,
        "terrain_upload_frames":upload_frames,"terrain":replay.session.capture_terrain_metrics(),
        "note":"native 480x270 diagnostic cut-in, no SSAA; not the final cinematic export"
    })).unwrap()).unwrap();
    pixels
}
fn pixel_difference(reference: &[u8], actual: &[u8]) -> serde_json::Value {
    assert_eq!(reference.len(), actual.len());
    let (mut differing, mut total, mut max) = (0usize, 0u64, 0u8);
    for (a, b) in reference.chunks_exact(4).zip(actual.chunks_exact(4)) {
        let delta = a.iter().zip(b).map(|(a, b)| a.abs_diff(*b)).max().unwrap();
        differing += usize::from(delta > 0);
        total += u64::from(delta);
        max = max.max(delta);
    }
    json!({"pixels":reference.len()/4,"differing_pixels":differing,"max_channel_delta":max,
        "mean_max_channel_delta":total as f64/(reference.len()/4) as f64})
}
fn open(recording: &Path, timebase: Timebase, radius: u32) -> Player {
    let mut replay = Player::open(recording, true, timebase).unwrap();
    replay.set_crowd_instancing(true);
    replay.set_render_distance(Some(radius)).unwrap();
    replay
}
fn measure(
    recording: &Path,
    directory: &Path,
    timebase: Timebase,
    camera: &CameraPath,
    radius: u32,
) {
    std::fs::create_dir_all(directory).unwrap();
    let target = Duration::from_secs_f64(camera.range()[0]);
    let (reference, reference_pixels, full_seconds, full_ticks) = {
        let mut replay = open(recording, timebase, radius);
        let start = Instant::now();
        replay
            .session
            .replay_seek_phase(Some(SeekPhase::Warmup(0)))
            .unwrap();
        replay.advance_to(target).unwrap();
        let full_ticks = replay.session.finish_replay_seek().unwrap();
        let seconds = start.elapsed().as_secs_f64();
        let state = snapshot(&replay);
        let pixels = render_cut(&mut replay, camera, directory, "full-presentation");
        eprintln!("{timebase:?} full presentation: {full_ticks} ticks, {seconds:.3}s");
        (state, pixels, seconds, full_ticks)
    };
    let mut windows = Vec::new();
    for ticks in WINDOWS {
        let mut replay = open(recording, timebase, radius);
        let warmup = replay.tick_interval().saturating_mul(ticks);
        let start = Instant::now();
        replay
            .session
            .replay_seek_phase(Some(SeekPhase::Skip))
            .unwrap();
        replay.advance_to(target.saturating_sub(warmup)).unwrap();
        let skip_seconds = start.elapsed().as_secs_f64();
        replay
            .session
            .replay_seek_phase(Some(SeekPhase::Warmup(0)))
            .unwrap();
        replay.advance_to(target).unwrap();
        let warmed = replay.session.finish_replay_seek().unwrap();
        let seconds = start.elapsed().as_secs_f64();
        let comparison = compare(&reference, &snapshot(&replay));
        let pixels = render_cut(&mut replay, camera, directory, &format!("warm-{ticks}"));
        let image = pixel_difference(&reference_pixels, &pixels);
        eprintln!("{timebase:?} warm-up {ticks}: {seconds:.3}s {comparison} {image}");
        windows.push(
            json!({"requested_ticks":ticks,"actual_ticks":warmed,"skip_seconds":skip_seconds,
            "seek_seconds":seconds,"comparison":comparison,"image_vs_full_presentation":image}),
        );
        // Publish each completed window; a later failure retains measured data.
        std::fs::write(directory.join("convergence.json"),serde_json::to_vec_pretty(&json!({
            "timebase":timebase,"target_seconds":target.as_secs_f64(),"render_distance":radius,
            "full_presentation_seconds":full_seconds,"full_presentation_ticks":full_ticks,
            "production_warmup_ticks":SEEK_WARMUP_TICKS,"windows":windows,
            "scope":"received-state exactness and lerp/body/seat/walk convergence against a cut-free \
                presentation reference; cumulative source age/gait phase may reset at a cut"
        })).unwrap()).unwrap();
    }
}

#[test]
#[ignore = "requires a real take, client content and offscreen Vulkan; run under scene-slot/mem-run"]
fn captured_take_warmup_convergence() {
    let variable = |name: &str| {
        PathBuf::from(std::env::var_os(name).unwrap_or_else(|| panic!("{name} unset")))
    };
    let recording = variable("KRASNOKAMEN_CINEMA_RECORDING");
    let directory = variable("KRASNOKAMEN_CINEMA_RECEIPTS");
    crate::content_paths::apply_defaults(&variable("KRASNOKAMEN_CINEMA_DATA"));
    crate::content_paths::validate(true).unwrap();
    let radius = variable("KRASNOKAMEN_CINEMA_RENDER_DISTANCE")
        .to_str()
        .unwrap()
        .parse::<u32>()
        .unwrap();
    // Each camera's first key is the cut target on its own timeline.
    for (label, timebase, camera) in [
        ("tick", Timebase::Tick, "KRASNOKAMEN_CINEMA_TICK_CAMERA"),
        ("wall", Timebase::Wall, "KRASNOKAMEN_CINEMA_WALL_CAMERA"),
    ] {
        let camera = CameraPath::parse(&std::fs::read(variable(camera)).unwrap()).unwrap();
        measure(
            &recording,
            &directory.join(label),
            timebase,
            &camera,
            radius,
        );
    }
}
